use crate::client::McpClient;
use crate::protocol::McpToolDescriptor;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;
use vitna_tools::{Tool, ToolContext, ToolDefinition, ToolResult};

/// Sanitizes an MCP server or tool identifier into a safe ASCII slug.
fn sanitize_slug(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

/// Builds a collision-free, non-shadowing registry name for an external MCP tool.
///
/// The `mcp__` prefix guarantees an untrusted MCP server can never overwrite a
/// built-in tool (`read_file`, `write_file`, `run_command`, etc.), and including
/// the server name keeps two servers exposing the same tool name distinct.
pub fn namespaced_tool_name(server_name: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_slug(server_name),
        sanitize_slug(tool_name)
    )
}

pub struct McpToolBridge {
    pub server_name: String,
    pub registered_name: String,
    pub descriptor: McpToolDescriptor,
    pub client: Arc<Mutex<McpClient>>,
    pub is_mutating: bool,
}

impl McpToolBridge {
    pub fn new(
        server_name: impl Into<String>,
        descriptor: McpToolDescriptor,
        client: Arc<Mutex<McpClient>>,
        is_mutating: bool,
    ) -> Self {
        let server_name = server_name.into();
        let registered_name = descriptor.name.clone();
        Self {
            server_name,
            registered_name,
            descriptor,
            client,
            is_mutating,
        }
    }

    /// Creates a bridge whose registry name is namespaced as `mcp__<server>__<tool>`
    /// so untrusted external servers can never shadow built-in workspace tools.
    pub fn namespaced(
        server_name: impl Into<String>,
        descriptor: McpToolDescriptor,
        client: Arc<Mutex<McpClient>>,
        is_mutating: bool,
    ) -> Self {
        let server_name = server_name.into();
        let registered_name = namespaced_tool_name(&server_name, &descriptor.name);
        Self {
            server_name,
            registered_name,
            descriptor,
            client,
            is_mutating,
        }
    }
}

/// Initializes an MCP client if needed, discovers all paginated tools via
/// `tools/list`, and wraps each tool in a namespaced [`McpToolBridge`].
///
/// Every bridged tool strictly requires exact-action approval and preserves
/// the server's real tool name for JSON-RPC `tools/call` and action digests.
pub async fn discover_and_bridge_tools(
    server_name: impl Into<String>,
    client: Arc<Mutex<McpClient>>,
) -> Result<Vec<Arc<dyn Tool>>, String> {
    let server_name = server_name.into();
    if server_name.trim().is_empty() {
        return Err("MCP server name must be non-empty".to_string());
    }

    let descriptors = {
        let mut guard = client.lock().await;
        if !guard.is_initialized() {
            guard.initialize().await?;
        }
        guard.list_tools().await?
    };

    let mut seen = HashSet::new();
    let mut bridges: Vec<Arc<dyn Tool>> = Vec::with_capacity(descriptors.len());
    for desc in descriptors {
        if desc.name.trim().is_empty() {
            return Err(format!(
                "MCP server '{}' advertised a tool with an empty name",
                server_name
            ));
        }
        let bridge = McpToolBridge::namespaced(&server_name, desc, Arc::clone(&client), true);
        if !seen.insert(bridge.registered_name.clone()) {
            return Err(format!(
                "MCP server '{}' produced duplicate namespaced tool '{}'",
                server_name, bridge.registered_name
            ));
        }
        bridges.push(Arc::new(bridge));
    }

    Ok(bridges)
}

#[async_trait]
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.registered_name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.registered_name.clone(),
            description: format!(
                "[MCP: {}] {}",
                self.server_name,
                self.descriptor.description.as_deref().unwrap_or("External MCP tool")
            ),
            parameters_schema: self.descriptor.input_schema.clone(),
            is_mutating: self.is_mutating,
            requires_approval: true, // Untrusted external MCP tools strictly require approval
        }
    }

    /// An MCP tool's identity includes the server that serves it, so this
    /// overrides the default, which knows only the tool's name. Two servers can
    /// expose a tool of the same name, and approving one must not approve the
    /// other.
    ///
    /// The server travels as its own field. It was once spliced into the name
    /// with an unescaped `:`, so server `a:b` exposing `c` and server `a`
    /// exposing `b:c` were one action, and both names come from the server,
    /// which this tool's own definition calls untrusted. The `mcp_server` key
    /// also keeps an MCP tool from ever sharing a digest with a built-in tool
    /// of the same name, since the built-in identity has no such key.
    fn compute_action_digest(&self, args: &serde_json::Value) -> String {
        vitna_tools::canonical_digest(&serde_json::json!({
            "mcp_server": self.server_name,
            "tool": self.descriptor.name,
            "args": args,
        }))
    }

    async fn execute(&self, args: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult, String> {
        let mut client = self.client.lock().await;
        let res = client.call_tool(&self.descriptor.name, args).await?;

        let mut output = String::new();
        for item in res.content {
            if let Some(t) = item.text {
                output.push_str(&t);
                output.push('\n');
            }
        }

        let is_success = !res.is_error.unwrap_or(false);

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: self.registered_name.clone(),
            success: is_success,
            output,
            preimage_hash: None,
            postimage_hash: None,
            diff: None,
            exit_code: None,
            ..Default::default()
        })
    }
}
