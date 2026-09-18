use crate::path_safety::resolve_workspace_path;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub struct SearchCodeTool;

impl SearchCodeTool {
    fn search_file(
        path: &Path,
        rel_path: &str,
        query: &str,
        case_sensitive: bool,
        results: &mut Vec<String>,
        max_results: usize,
    ) {
        if results.len() >= max_results {
            return;
        }

        let raw_bytes = match fs::read(path) {
            Ok(b) => b,
            Err(_) => return,
        };

        // Skip non-text binary files (heuristic: zero bytes in first 512 bytes)
        if raw_bytes.iter().take(512).any(|&b| b == 0) {
            return;
        }

        let text = String::from_utf8_lossy(&raw_bytes);
        let q_lower = query.to_lowercase();

        for (idx, line) in text.lines().enumerate() {
            if results.len() >= max_results {
                break;
            }

            let matches = if case_sensitive {
                line.contains(query)
            } else {
                line.to_lowercase().contains(&q_lower)
            };

            if matches {
                results.push(format!("{}:{}: {}", rel_path, idx + 1, line.trim()));
            }
        }
    }

    fn search_dir(
        base_root: &Path,
        current_dir: &Path,
        query: &str,
        case_sensitive: bool,
        results: &mut Vec<String>,
        max_results: usize,
    ) {
        if results.len() >= max_results || !current_dir.is_dir() {
            return;
        }

        let entries = match fs::read_dir(current_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            if results.len() >= max_results {
                break;
            }

            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            if name_str == ".git" || name_str == ".vitna" || name_str == "target" || name_str == "node_modules" {
                continue;
            }

            if path.is_dir() {
                Self::search_dir(base_root, &path, query, case_sensitive, results, max_results);
            } else if path.is_file() {
                if let Ok(rel) = path.strip_prefix(base_root) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    Self::search_file(&path, &rel_str, query, case_sensitive, results, max_results);
                }
            }
        }
    }
}

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_code".to_string(),
            description: "Fast workspace code search across files with line numbers and matched content.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Text substring or keyword to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional relative subpath to restrict search. Defaults to '.' (entire workspace)."
                    },
                    "case_sensitive": {
                        "type": "boolean",
                        "description": "Optional flag for case sensitivity. Defaults to false."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Optional limit on number of returned results. Defaults to 100."
                    }
                },
                "required": ["query"]
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
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .ok_or_else(|| "Missing required 'query' parameter".to_string())?;

        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or(".");

        let case_sensitive = args
            .get("case_sensitive")
            .and_then(|c| c.as_bool())
            .unwrap_or(false);

        let max_results = args
            .get("max_results")
            .and_then(|m| m.as_u64())
            .map(|m| m as usize)
            .unwrap_or(100);

        let target_dir = resolve_workspace_path(&ctx.workspace_root, path_str)?;

        let mut results = Vec::new();
        if target_dir.is_file() {
            if let Ok(rel) = target_dir.strip_prefix(&ctx.workspace_root) {
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                Self::search_file(&target_dir, &rel_str, query, case_sensitive, &mut results, max_results);
            }
        } else {
            Self::search_dir(&ctx.workspace_root, &target_dir, query, case_sensitive, &mut results, max_results);
        }

        let mut output = format!("Search results for '{}' ({} matches):\n", query, results.len());
        if results.is_empty() {
            output.push_str("No matches found.\n");
        } else {
            for res in results {
                output.push_str(&format!("{}\n", res));
            }
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "search_code".to_string(),
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
