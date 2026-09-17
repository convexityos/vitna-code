use crate::path_safety::resolve_workspace_path;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};

pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "run_command".to_string(),
            description: "Execute a command in the workspace through the guarded runner. Subject to policy and approval gating.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command line to execute."
                    },
                    "working_directory": {
                        "type": "string",
                        "description": "Optional relative directory within workspace. Defaults to '.' (workspace root)."
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Optional execution timeout in milliseconds. Defaults to 30000 (30 seconds)."
                    }
                },
                "required": ["command"]
            }),
            is_mutating: true,
            requires_approval: true,
        }
    }

    fn compute_action_digest(&self, args: &serde_json::Value) -> String {
        let cmd = args.get("command").and_then(|c| c.as_str()).unwrap_or_default();
        let wd = args.get("working_directory").and_then(|w| w.as_str()).unwrap_or(".");
        let canonical = format!("run_command:{}:{}", cmd, wd);
        let hash = Sha256::digest(canonical.as_bytes());
        hex::encode(hash)
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let command = args
            .get("command")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required 'command' parameter".to_string())?;

        let wd_str = args
            .get("working_directory")
            .and_then(|w| w.as_str())
            .unwrap_or(".");

        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|t| t.as_u64())
            .unwrap_or(30_000);

        let resolved_wd = resolve_workspace_path(&ctx.workspace_root, wd_str)?;

        let output = ctx
            .runner
            .run_command(command, &resolved_wd, timeout_ms)
            .await?;

        let mut formatted_output = format!(
            "Command executed: {}\nExit code: {}\nDuration: {} ms\n",
            command, output.exit_code, output.duration_ms
        );

        if !output.stdout.is_empty() {
            formatted_output.push_str(&format!("--- Stdout ---\n{}\n", output.stdout));
        }
        if !output.stderr.is_empty() {
            formatted_output.push_str(&format!("--- Stderr ---\n{}\n", output.stderr));
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "run_command".to_string(),
            success: output.exit_code == 0,
            output: formatted_output,
            preimage_hash: None,
            postimage_hash: None,
            diff: None,
            exit_code: Some(output.exit_code),
        })
    }
}
