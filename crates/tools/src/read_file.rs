use crate::workspace_fs::{read_up_to, sha256_hex, Resolved, Workspace, MAX_FILE_BYTES};
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};

/// The most bytes of numbered file content one call returns.
const MAX_OUTPUT_BYTES: usize = 256 * 1024;

pub struct ReadFileTool;

impl ReadFileTool {
    /// Renders lines `start..=end` (1-indexed, both within `lines`) with line
    /// numbers, stopping at `budget` bytes with a notice saying where to
    /// resume. A single line longer than the budget is shown cut, so a call
    /// always makes progress.
    fn render_lines(lines: &[&str], start: usize, end: usize, budget: usize) -> String {
        let mut out = String::new();
        for (idx, line) in lines[start - 1..end].iter().enumerate() {
            let number = start + idx;
            let rendered = format!("{:4} | {}\n", number, line);
            if out.len() + rendered.len() <= budget {
                out.push_str(&rendered);
                continue;
            }
            let resume_at = if out.is_empty() {
                let mut cut = budget.min(rendered.len());
                while !rendered.is_char_boundary(cut) {
                    cut -= 1;
                }
                out.push_str(&rendered[..cut]);
                out.push('\n');
                out.push_str(&format!(
                    "--- truncated: line {} is longer than the {} bytes read_file returns per call, so only its start is shown.\n",
                    number, budget
                ));
                number + 1
            } else {
                out.push_str(&format!(
                    "--- truncated: read_file returns at most {} bytes per call.\n",
                    budget
                ));
                number
            };
            if resume_at <= end {
                out.push_str(&format!(
                    "Lines {} to {} were not shown. Call read_file again with start_line={} to continue.\n",
                    resume_at, end, resume_at
                ));
            }
            break;
        }
        out
    }
}

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

        // This tool needs no approval, so it must not be able to read
        // anything the workspace does not contain: the path is resolved on
        // the real filesystem, links included, and only a regular file is
        // opened. A FIFO would block here forever.
        let workspace = Workspace::new(&ctx.workspace_root)?;
        let real_path = match workspace.resolve(path_str)? {
            Resolved::Existing(real_path) => real_path,
            Resolved::Missing(_) => return Err(format!("File does not exist: {}", path_str)),
        };
        let mut file = workspace.open_regular(&real_path, path_str)?;
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);

        let (mut raw_bytes, truncated) = read_up_to(&mut file, MAX_FILE_BYTES)
            .map_err(|e| format!("Failed to read file {}: {}", path_str, e))?;

        // Hash only a complete read. A prefix's hash would read as the
        // file's, and apply_patch would accept it as the preimage.
        let content_hash = if truncated {
            // Drop the line the bound cut in half.
            if let Some(last_newline) = raw_bytes.iter().rposition(|&b| b == b'\n') {
                raw_bytes.truncate(last_newline + 1);
            }
            None
        } else {
            Some(sha256_hex(&raw_bytes))
        };

        let content = String::from_utf8_lossy(&raw_bytes);
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
        match &content_hash {
            Some(hash) => {
                output.push_str(&format!("File: {} (total lines: {})\n", path_str, total_lines));
                output.push_str(&format!("SHA-256: {}\n---\n", hash));
            }
            None => {
                output.push_str(&format!(
                    "File: {} ({} bytes, larger than the {} bytes read_file reads; lines readable: {})\n",
                    path_str, size, MAX_FILE_BYTES, total_lines
                ));
                output.push_str("SHA-256: not computed, the file was not read in full\n---\n");
            }
        }

        if total_lines == 0 {
            output.push_str("(file is empty)\n");
        } else if start_line > total_lines {
            output.push_str(&format!("(start_line {} exceeds total lines {})\n", start_line, total_lines));
        } else if end_line < start_line {
            output.push_str(&format!("(end_line {} is before start_line {})\n", end_line, start_line));
        } else {
            output.push_str(&Self::render_lines(&lines, start_line, end_line, MAX_OUTPUT_BYTES));
        }

        if truncated {
            output.push_str(&format!(
                "--- truncated: the file is {} bytes and read_file reads at most {}; nothing after line {} is shown.\n",
                size, MAX_FILE_BYTES, total_lines
            ));
        }

        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "read_file".to_string(),
            success: true,
            output,
            preimage_hash: content_hash.clone(),
            postimage_hash: content_hash,
            diff: None,
            exit_code: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dir_link, file_link, Scratch};
    use std::fs;

    async fn read(s: &Scratch, args: serde_json::Value) -> Result<ToolResult, String> {
        ReadFileTool.execute(args, &s.ctx()).await
    }

    #[tokio::test]
    async fn test_reads_with_hash_and_line_numbers() {
        let s = Scratch::new("read_basic");
        fs::write(s.ws().join("a.txt"), "one\r\ntwo\nthree").unwrap();
        let res = read(&s, json!({ "path": "a.txt" })).await.expect("read");
        let hash = sha256_hex(b"one\r\ntwo\nthree");
        assert!(res.output.contains("total lines: 3"), "{}", res.output);
        assert!(res.output.contains(&hash));
        assert!(res.output.contains("   2 | two\n"));
        assert_eq!(res.preimage_hash.as_deref(), Some(hash.as_str()));
    }

    #[tokio::test]
    async fn test_link_out_of_the_workspace_is_denied() {
        let s = Scratch::new("read_link_out");
        if !file_link(&s.outside().join("secret.txt"), &s.ws().join("notes.txt")) {
            eprintln!("skipped: this machine cannot create symbolic links");
            return;
        }
        let err = read(&s, json!({ "path": "notes.txt" })).await.unwrap_err();
        assert!(err.contains("Access denied"), "{}", err);
        assert!(!err.contains("TOP SECRET"));
    }

    #[tokio::test]
    async fn test_directory_link_out_of_the_workspace_is_denied() {
        let s = Scratch::new("read_dir_link_out");
        if !dir_link(&s.outside(), &s.ws().join("notes")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let err = read(&s, json!({ "path": "notes/secret.txt" })).await.unwrap_err();
        assert!(err.contains("Access denied"), "{}", err);
    }

    #[tokio::test]
    async fn test_a_directory_is_refused() {
        let s = Scratch::new("read_directory");
        fs::create_dir_all(s.ws().join("src")).unwrap();
        let err = read(&s, json!({ "path": "src" })).await.unwrap_err();
        assert!(err.contains("not a regular file"), "{}", err);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_fifo_is_refused_without_blocking() {
        let s = Scratch::new("read_fifo");
        if !crate::test_support::mkfifo(&s.ws().join("pipe")) {
            eprintln!("skipped: mkfifo is unavailable");
            return;
        }
        let res = crate::test_support::execute_bounded(
            ReadFileTool,
            json!({ "path": "pipe" }),
            s.ctx(),
        );
        let err = res.unwrap_err();
        assert!(err.contains("a FIFO"), "{}", err);
    }

    #[tokio::test]
    async fn test_bad_line_ranges_do_not_panic() {
        let s = Scratch::new("read_ranges");
        fs::write(s.ws().join("a.txt"), "1\n2\n3\n").unwrap();
        let res = read(&s, json!({ "path": "a.txt", "start_line": 3, "end_line": 2 }))
            .await
            .expect("read");
        assert!(res.output.contains("end_line 2 is before start_line 3"), "{}", res.output);
        let res = read(&s, json!({ "path": "a.txt", "start_line": 9 })).await.expect("read");
        assert!(res.output.contains("exceeds total lines"), "{}", res.output);
    }

    #[tokio::test]
    async fn test_long_output_says_where_to_resume() {
        let s = Scratch::new("read_long_output");
        let line = "x".repeat(99);
        let body: String = (0..5000).map(|_| format!("{}\n", line)).collect();
        fs::write(s.ws().join("big.txt"), &body).unwrap();

        let res = read(&s, json!({ "path": "big.txt" })).await.expect("read");
        assert!(res.output.len() < MAX_OUTPUT_BYTES + 4096);
        assert!(res.output.contains("--- truncated"), "must say it was cut");
        assert!(res.output.contains("Call read_file again with start_line="));
        // The file itself was read in full, so its hash is still attested.
        assert_eq!(res.preimage_hash, Some(sha256_hex(body.as_bytes())));
    }

    #[test]
    fn test_render_lines_always_makes_progress() {
        let lines = ["short", "a line far longer than the budget", "tail"];
        let out = ReadFileTool::render_lines(&lines, 2, 3, 16);
        assert!(out.starts_with("   2 | a line"), "{}", out);
        assert!(out.contains("start_line=3"), "{}", out);

        let out = ReadFileTool::render_lines(&lines, 1, 3, 25);
        assert!(out.contains("   1 | short\n"), "{}", out);
        assert!(out.contains("start_line=2"), "{}", out);

        let out = ReadFileTool::render_lines(&lines, 1, 3, 1024);
        assert!(!out.contains("truncated"), "{}", out);
    }
}
