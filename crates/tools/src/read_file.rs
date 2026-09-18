use crate::path_safety::resolve_workspace_path;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read file contents from the workspace with line numbering and SHA-256 verification.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path to the file within the workspace."
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "Optional 1-indexed start line number to view."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Optional 1-indexed end line number to view."
                    }
                },
                "required": ["path"]
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
            .ok_or_else(|| "Missing required 'path' parameter".to_string())?;

        let resolved_path = resolve_workspace_path(&ctx.workspace_root, path_str)?;

        if !resolved_path.exists() {
            return Err(format!("File does not exist: {}", path_str));
        }

        let raw_bytes = fs::read(&resolved_path)
            .map_err(|e| format!("Failed to read file {}: {}", path_str, e))?;
        let content = String::from_utf8_lossy(&raw_bytes).to_string();
        let content_hash = hex::encode(Sha256::digest(&raw_bytes));

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start_line = args
            .get("start_line")
            .and_then(|l| l.as_u64())
            .map(|l| l.max(1) as usize)
            .unwrap_or(1);

        let end_line = args
            .get("end_line")
            .and_then(|l| l.as_u64())
            .map(|l| (l as usize).min(total_lines))
            .unwrap_or(total_lines);

        let mut output = String::new();
        output.push_str(&format!("File: {} (total lines: {})\n", path_str, total_lines));
        output.push_str(&format!("SHA-256: {}\n---\n", content_hash));

        if total_lines == 0 {
            output.push_str("(file is empty)\n");
        } else if start_line > total_lines {
            output.push_str(&format!("(start_line {} exceeds total lines {})\n", start_line, total_lines));
        } else {
            for (idx, line) in lines[start_line - 1..end_line].iter().enumerate() {
                output.push_str(&format!("{:4} | {}\n", start_line + idx, line));
            }
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "read_file".to_string(),
            success: true,
            output,
            preimage_hash: Some(content_hash.clone()),
            postimage_hash: Some(content_hash),
            diff: None,
            exit_code: None,
            ..Default::default()
        })
    }
}
