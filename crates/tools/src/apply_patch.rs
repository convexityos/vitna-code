use crate::path_safety::resolve_workspace_path;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;

pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "apply_patch".to_string(),
            description: "Apply a patch to a workspace file with preimage hash validation to guarantee conflict-free modification.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the target file within the workspace."
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete new content for the file after patch application."
                    },
                    "expected_preimage_hash": {
                        "type": "string",
                        "description": "Optional SHA-256 hash of the expected file content before patching."
                    }
                },
                "required": ["path", "content"]
            }),
            is_mutating: true,
            requires_approval: true,
        }
    }

    fn compute_action_digest(&self, args: &serde_json::Value) -> String {
        let path = args.get("path").and_then(|p| p.as_str()).unwrap_or_default();
        let content = args.get("content").and_then(|c| c.as_str()).unwrap_or_default();
        let canonical = format!("apply_patch:{}:{}", path, content);
        let hash = Sha256::digest(canonical.as_bytes());
        hex::encode(hash)
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required 'path' parameter".to_string())?;

        let new_content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required 'content' parameter".to_string())?;

        let resolved_path = resolve_workspace_path(&ctx.workspace_root, path_str)?;

        let (old_content, current_preimage) = if resolved_path.exists() {
            let raw = fs::read(&resolved_path)
                .map_err(|e| format!("Failed to read existing file: {}", e))?;
            let hash = hex::encode(Sha256::digest(&raw));
            (String::from_utf8_lossy(&raw).to_string(), hash)
        } else {
            (
                String::new(),
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            )
        };

        // Preimage validation: abort if file changed under the agent
        if let Some(expected) = args.get("expected_preimage_hash").and_then(|e| e.as_str()) {
            if expected != current_preimage {
                return Err(format!(
                    "Patch conflict: expected preimage '{}' does not match current file hash '{}'",
                    expected, current_preimage
                ));
            }
        }

        let postimage_hash = hex::encode(Sha256::digest(new_content.as_bytes()));
        let diff = crate::write_file::WriteFileTool::generate_unified_diff(path_str, &old_content, new_content);

        if let Some(parent) = resolved_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        fs::write(&resolved_path, new_content)
            .map_err(|e| format!("Failed to write patched file: {}", e))?;

        let output = format!(
            "Patch applied cleanly to {}\nPreimage:  {}\nPostimage: {}\n",
            path_str, current_preimage, postimage_hash
        );

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "apply_patch".to_string(),
            success: true,
            output,
            preimage_hash: Some(current_preimage),
            postimage_hash: Some(postimage_hash),
            diff: Some(diff),
            exit_code: None,
        })
    }
}
