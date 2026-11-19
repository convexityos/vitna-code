use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;

use crate::protocol::{
    JsonRpcNotification, JsonRpcRequest, McpToolCallResult, McpToolDescriptor, McpToolsListResult,
};
use crate::transport::{HttpMcpTransport, McpTransport, StdioMcpTransport, DEFAULT_TIMEOUT_MS};

/// Maximum `tools/list` pages followed from a single server to guard against
/// hostile infinite-cursor loops.
const MAX_TOOL_LIST_PAGES: usize = 32;

/// Maximum tools accepted from a single server.
const MAX_TOOLS_PER_SERVER: usize = 512;

pub struct McpClient {
    pub server_name: String,
    next_id: u64,
    transport: Option<Box<dyn McpTransport>>,
    initialized: bool,
    server_capabilities: Option<serde_json::Value>,
    // In-memory mock tool registry for tests and headless verification
    mock_tools: HashMap<String, McpToolDescriptor>,
    mock_handlers:
        HashMap<String, Box<dyn Fn(serde_json::Value) -> McpToolCallResult + Send + Sync>>,
}

impl McpClient {
    /// Creates an in-memory mock MCP client for offline tests.
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            next_id: 1,
            transport: None,
            initialized: false,
            server_capabilities: None,
            mock_tools: HashMap::new(),
            mock_handlers: HashMap::new(),
        }
    }

    /// Creates an MCP client backed by any real [`McpTransport`].
    pub fn with_transport(
        server_name: impl Into<String>,
        transport: Box<dyn McpTransport>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            next_id: 1,
            transport: Some(transport),
            initialized: false,
            server_capabilities: None,
            mock_tools: HashMap::new(),
            mock_handlers: HashMap::new(),
        }
    }

    /// Spawns a local stdio MCP server process with an allowlisted environment.
    pub fn connect_stdio(
        server_name: impl Into<String>,
        program: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, String> {
        let transport =
            StdioMcpTransport::spawn(program, args, env, Duration::from_millis(DEFAULT_TIMEOUT_MS))?;
        Ok(Self::with_transport(server_name, Box::new(transport)))
    }

    /// Connects to a Streamable HTTP MCP server endpoint.
    pub fn connect_http(
        server_name: impl Into<String>,
        endpoint: impl Into<String>,
    ) -> Result<Self, String> {
        let transport =
            HttpMcpTransport::new(endpoint, Duration::from_millis(DEFAULT_TIMEOUT_MS))?;
        Ok(Self::with_transport(server_name, Box::new(transport)))
    }

    /// Registers a tool handler for testing and offline integration.
    pub fn register_mock_tool<F>(
        &mut self,
        name: &str,
        description: &str,
        input_schema: serde_json::Value,
        handler: F,
    ) where
        F: Fn(serde_json::Value) -> McpToolCallResult + Send + Sync + 'static,
    {
        let descriptor = McpToolDescriptor {
            name: name.to_string(),
            description: Some(description.to_string()),
            input_schema,
        };
        self.mock_tools.insert(name.to_string(), descriptor);
        self.mock_handlers.insert(name.to_string(), Box::new(handler));
    }

    pub fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn server_capabilities(&self) -> Option<&serde_json::Value> {
        self.server_capabilities.as_ref()
    }

    /// Performs the MCP initialization handshake (`initialize` + `notifications/initialized`).
    pub async fn initialize(&mut self) -> Result<serde_json::Value, String> {
        let id = self.next_request_id();
        let req = JsonRpcRequest::new(
            id,
            "initialize",
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "clientInfo": {
                    "name": "vitna-code",
                    "version": env!("CARGO_PKG_VERSION")
                }
            })),
        );

        if let Some(transport) = self.transport.as_mut() {
            let resp = transport.request(&req).await?;
            if let Some(err) = resp.error {
                return Err(format!(
                    "MCP server '{}' rejected initialize ({}): {}",
                    self.server_name, err.code, err.message
                ));
            }
            let result = resp.result.ok_or_else(|| {
                format!(
                    "MCP server '{}' initialize response missing result",
                    self.server_name
                )
            })?;
            transport
                .notify(&JsonRpcNotification::new("notifications/initialized", None))
                .await?;
            self.initialized = true;
            self.server_capabilities = result.get("capabilities").cloned();
            return Ok(result);
        }

        self.initialized = true;
        Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": self.server_name,
                "version": "1.0.0"
            }
        }))
    }

    /// Lists all tools exposed by the MCP server, following pagination cursors
    /// up to [`MAX_TOOL_LIST_PAGES`] pages.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDescriptor>, String> {
        if self.transport.is_none() {
            let _ = self.next_request_id();
            return Ok(self.mock_tools.values().cloned().collect());
        }

        let mut all_tools = Vec::new();
        let mut cursor: Option<String> = None;

        for _page in 0..MAX_TOOL_LIST_PAGES {
            let id = self.next_request_id();
            let params = cursor.as_ref().map(|c| json!({ "cursor": c }));
            let req = JsonRpcRequest::new(id, "tools/list", params);

            let resp = self
                .transport
                .as_mut()
                .expect("transport checked")
                .request(&req)
                .await?;
            if let Some(err) = resp.error {
                return Err(format!(
                    "MCP server '{}' tools/list failed ({}): {}",
                    self.server_name, err.code, err.message
                ));
            }
            let result_val = resp.result.ok_or_else(|| {
                format!(
                    "MCP server '{}' tools/list response missing result",
                    self.server_name
                )
            })?;
            let page: McpToolsListResult = serde_json::from_value(result_val).map_err(|e| {
                format!(
                    "MCP server '{}' returned invalid tools/list result: {e}",
                    self.server_name
                )
            })?;
            all_tools.extend(page.tools);
            if all_tools.len() > MAX_TOOLS_PER_SERVER {
                return Err(format!(
                    "MCP server '{}' exceeded maximum of {MAX_TOOLS_PER_SERVER} tools",
                    self.server_name
                ));
            }

            match page.next_cursor {
                Some(next) if !next.is_empty() => {
                    if cursor.as_deref() == Some(next.as_str()) {
                        return Err(format!(
                            "MCP server '{}' repeated tools/list pagination cursor",
                            self.server_name
                        ));
                    }
                    cursor = Some(next);
                }
                _ => return Ok(all_tools),
            }
        }

        Err(format!(
            "MCP server '{}' exceeded maximum of {MAX_TOOL_LIST_PAGES} tools/list pages",
            self.server_name
        ))
    }

    /// Invokes an MCP tool on the server.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolCallResult, String> {
        let id = self.next_request_id();
        let req = JsonRpcRequest::new(
            id,
            "tools/call",
            Some(json!({
                "name": name,
                "arguments": arguments
            })),
        );

        if let Some(transport) = self.transport.as_mut() {
            let resp = transport.request(&req).await?;
            if let Some(err) = resp.error {
                return Err(format!(
                    "MCP server '{}' tool '{}' failed ({}): {}",
                    self.server_name, name, err.code, err.message
                ));
            }
            let result_val = resp.result.ok_or_else(|| {
                format!(
                    "MCP server '{}' tool '{}' response missing result",
                    self.server_name, name
                )
            })?;
            return serde_json::from_value(result_val).map_err(|e| {
                format!(
                    "MCP server '{}' tool '{}' returned invalid result: {e}",
                    self.server_name, name
                )
            });
        }

        if let Some(handler) = self.mock_handlers.get(name) {
            Ok(handler(arguments))
        } else {
            Err(format!(
                "MCP tool '{}' not found on server '{}'",
                name, self.server_name
            ))
        }
    }
}
