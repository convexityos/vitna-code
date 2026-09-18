use crate::path_safety::resolve_workspace_path;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;

pub struct WriteFileTool;

impl WriteFileTool {
    /// Generates a unified diff comparing old_content with new_content.
    pub fn generate_unified_diff(path: &str, old_content: &str, new_content: &str) -> String {
        let mut diff = format!("--- a/{}\n+++ b/{}\n", path, path);

        let old_lines: Vec<&str> = if old_content.is_empty() {
            Vec::new()
        } else {
            old_content.lines().collect()
        };
        let new_lines: Vec<&str> = if new_content.is_empty() {
            Vec::new()
        } else {
            new_content.lines().collect()
        };

        diff.push_str(&format!(
            "@@ -1,{} +1,{} @@\n",
            old_lines.len(),
            new_lines.len()
        ));

        // Simple line diff representation
        for line in &old_lines {
            diff.push_str(&format!("-{}\n", line));
        }
        for line in &new_lines {
            diff.push_str(&format!("+{}\n", line));
        }

        diff
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".to_string(),
            description: "Write or update file content within the workspace. Records cryptographic preimage and postimage hashes and generates unified diffs.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the destination file within the workspace."
                    },
                    "content": {
                        "type": "string",
                        "description": "Complete new content to write to the file."
                    },
                    "description": {
                        "type": "string",
                        "description": "Optional human-readable explanation of why this change is being made."
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
        let canonical = format!("write_file:{}:{}", path, content);
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

        let (old_content, preimage_hash) = if resolved_path.exists() {
            let raw = fs::read(&resolved_path)
                .map_err(|e| format!("Failed to read existing file {}: {}", path_str, e))?;
            let hash = hex::encode(Sha256::digest(&raw));
            (String::from_utf8_lossy(&raw).to_string(), hash)
        } else {
            (
                String::new(),
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
            )
        };

        let postimage_hash = hex::encode(Sha256::digest(new_content.as_bytes()));
        let diff = Self::generate_unified_diff(path_str, &old_content, new_content);

        // Ensure parent directory exists
        if let Some(parent) = resolved_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent directory for {}: {}", path_str, e))?;
        }

        // Write content
        fs::write(&resolved_path, new_content)
            .map_err(|e| format!("Failed to write file {}: {}", path_str, e))?;

        let output = format!(
            "Successfully wrote {} bytes to {}\nPreimage: {}\nPostimage: {}\nDiff:\n{}",
            new_content.len(),
            path_str,
            preimage_hash,
            postimage_hash,
            diff
        );

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "write_file".to_string(),
            success: true,
            output,
            preimage_hash: Some(preimage_hash),
            postimage_hash: Some(postimage_hash),
            diff: Some(diff),
            exit_code: None,
            ..Default::default()
        })
    }
}
