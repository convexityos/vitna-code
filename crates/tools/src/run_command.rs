use crate::path_safety::resolve_workspace_path;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use vitna_runner::CommandRequest;

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

        // The runner decides isolation and refuses when it cannot get it. This
        // tool passes the workspace root because that, not the working
        // directory, is what the sandbox makes writable.
        let mut request = CommandRequest::new(
            command,
            &resolved_wd,
            &ctx.workspace_root,
            timeout_ms,
        );
        request.allow_unsandboxed = ctx.allow_unsandboxed;
        request.allow_network = ctx.allow_network;

        let output = ctx.runner.run_command(request).await?;

        let mut formatted_output = format!(
            "Command executed: {}\nExit code: {}\nDuration: {} ms\nIsolation: {} ({})\n",
            command,
            output.exit_code,
            output.duration_ms,
            output.sandbox_backend,
            output.sandbox_enforcement
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
            sandbox_backend: Some(output.sandbox_backend),
            sandbox_enforcement: Some(output.sandbox_enforcement),
        })
    }
}
