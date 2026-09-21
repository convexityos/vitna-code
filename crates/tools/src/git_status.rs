use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use vitna_git_workspaces::host_git;

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

    async fn execute(&self, _args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        // The workspace is untrusted input and git executes commands its own
        // configuration names, so both invocations below are built by
        // `host_git::command`. This tool runs on the host, outside any sandbox,
        // and asks for no approval, which is exactly why it must not be able to
        // run anything the repository chose. See `host_git` for the three keys
        // that reach a shell from `git status`.
        let branch_out = host_git::command(&ctx.workspace_root)?
            .arg("branch")
            .arg("--show-current")
            .output();

        let branch = match branch_out {
            Ok(out) if out.status.success() => {
                let b = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if b.is_empty() { "(detached HEAD)".to_string() } else { b }
            }
            _ => "(not a git repository or git unavailable)".to_string(),
        };

        // `--ignore-submodules=dirty` keeps status from scanning submodule
        // working trees. Filter drivers are neutralized per repository, and a
        // submodule carries its own config in `.git/modules/<name>/config`,
        // which this enumeration does not reach. Submodule commit changes are
        // still reported; only their working-tree contents are skipped.
        let status_out = host_git::command(&ctx.workspace_root)?
            .arg("status")
            .arg("--porcelain=v1")
            .arg("--ignore-submodules=dirty")
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
            ..Default::default()
        })
    }
}
