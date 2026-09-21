use crate::client::McpClient;
use crate::protocol::McpToolDescriptor;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;
use vitna_tools::{Tool, ToolContext, ToolDefinition, ToolResult};

pub struct McpToolBridge {
    pub server_name: String,
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
        Self {
            server_name: server_name.into(),
            descriptor,
            client,
            is_mutating,
        }
    }
}

#[async_trait]
impl Tool for McpToolBridge {
    fn name(&self) -> &str {
        &self.descriptor.name
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.descriptor.name.clone(),
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
            tool_name: self.descriptor.name.clone(),
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
