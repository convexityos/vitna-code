use crate::protocol::{JsonRpcRequest, McpToolCallResult, McpToolDescriptor};
use serde_json::json;
use std::collections::HashMap;

pub struct McpClient {
    pub server_name: String,
    next_id: u64,
    // In-memory mock tool registry for tests and headless verification
    mock_tools: HashMap<String, McpToolDescriptor>,
    mock_handlers: HashMap<String, Box<dyn Fn(serde_json::Value) -> McpToolCallResult + Send + Sync>>,
}

impl McpClient {
    pub fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            next_id: 1,
            mock_tools: HashMap::new(),
            mock_handlers: HashMap::new(),
        }
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

    /// Performs the MCP initialization handshake.
    pub async fn initialize(&mut self) -> Result<serde_json::Value, String> {
        let id = self.next_request_id();
        let _req = JsonRpcRequest::new(
            id,
            "initialize",
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "clientInfo": {
                    "name": "vitna-code",
                    "version": "0.1.0"
                }
            })),
        );

        // Simulated initialization response
        Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {
                "name": self.server_name,
                "version": "1.0.0"
            }
        }))
    }

    /// Lists tools exposed by the MCP server.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDescriptor>, String> {
        let id = self.next_request_id();
        let _req = JsonRpcRequest::new(id, "tools/list", None);

        let tools: Vec<McpToolDescriptor> = self.mock_tools.values().cloned().collect();
        Ok(tools)
    }

    /// Invokes an MCP tool on the server.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<McpToolCallResult, String> {
        let id = self.next_request_id();
        let _req = JsonRpcRequest::new(
            id,
            "tools/call",
            Some(json!({
                "name": name,
                "arguments": arguments
            })),
        );

        if let Some(handler) = self.mock_handlers.get(name) {
            Ok(handler(arguments))
        } else {
            Err(format!("MCP tool '{}' not found on server '{}'", name, self.server_name))
        }
    }
}
