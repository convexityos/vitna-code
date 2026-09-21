use crate::workspace_fs::{Resolved, Workspace};
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str {
        "list_dir"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_dir".to_string(),
            description: "List files and subdirectories within a workspace directory with metadata.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional relative path to directory within workspace. Defaults to '.' (root)."
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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or(".");

        // Resolved on the real filesystem, so a directory link cannot list
        // what lies outside the workspace.
        let workspace = Workspace::new(&ctx.workspace_root)?;
        let resolved_path = match workspace.resolve(path_str)? {
            Resolved::Existing(real_path) => real_path,
            Resolved::Missing(_) => return Err(format!("Directory does not exist: {}", path_str)),
        };

        if !resolved_path.is_dir() {
            return Err(format!("Path is not a directory: {}", path_str));
        }

        let mut entries = Vec::new();
        let read_dir = fs::read_dir(&resolved_path)
            .map_err(|e| format!("Failed to read directory {}: {}", path_str, e))?;

        for entry in read_dir {
            let entry = entry.map_err(|e| format!("Directory read error: {}", e))?;
            let file_name = entry.file_name().to_string_lossy().to_string();
            let file_type = entry.file_type().map_err(|e| format!("Metadata error: {}", e))?;
            let is_dir = file_type.is_dir();
            let size = if is_dir {
                0
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };

            entries.push((file_name, is_dir, size));
        }

        entries.sort_by(|a, b| {
            // Directories first, then alphabetical
            match (a.1, b.1) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.0.cmp(&b.0),
            }
        });

        let mut output = format!("Directory listing for '{}' ({} entries):\n", path_str, entries.len());
        for (name, is_dir, size) in entries {
            if is_dir {
                output.push_str(&format!("[DIR]  {}/\n", name));
            } else {
                output.push_str(&format!("[FILE] {:<30} ({} bytes)\n", name, size));
            }
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "list_dir".to_string(),
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
