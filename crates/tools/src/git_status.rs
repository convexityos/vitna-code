use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::process::Command;

pub struct GitStatusTool;

#[async_trait]
impl Tool for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_status".to_string(),
            description: "Inspect workspace Git status including current branch, modified, staged, and untracked files.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "detailed": {
                        "type": "boolean",
                        "description": "Optional flag for verbose output."
                    }
                }
            }),
            is_mutating: false,
            requires_approval: false,
        }
    }

    fn compute_action_digest(&self, args: &serde_json::Value) -> String {
        let canonical = serde_json::to_string(args).unwrap_or_default();
        let hash = Sha256::digest(canonical.as_bytes());
        hex::encode(hash)
    }

    async fn execute(&self, _args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let branch_out = Command::new("git")
            .arg("branch")
            .arg("--show-current")
            .current_dir(&ctx.workspace_root)
            .output();

        let branch = match branch_out {
            Ok(out) if out.status.success() => {
                let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if b.is_empty() { "(detached HEAD)".to_string() } else { b }
            }
            _ => "(not a git repository or git unavailable)".to_string(),
        };

        let status_out = Command::new("git")
            .arg("status")
            .arg("--porcelain=v1")
            .current_dir(&ctx.workspace_root)
            .output();

        let status_text = match status_out {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
            _ => String::new(),
        };

        let mut output = format!("Branch: {}\n", branch);
        if status_text.trim().is_empty() {
            output.push_str("Working tree clean (zero uncommitted changes).\n");
        } else {
            output.push_str("Changes:\n");
            for line in status_text.lines() {
                output.push_str(&format!("  {}\n", line));
            }
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "git_status".to_string(),
            success: true,
            output,
            preimage_hash: None,
            postimage_hash: None,
            diff: None,
            exit_code: None,
        })
    }
}
