use crate::workspace_fs::Workspace;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;

pub struct WriteFileTool;

impl WriteFileTool {
    /// Generates a unified diff comparing old_content with new_content.
    ///
    /// Each line keeps its exact terminator, so a change of line endings
    /// alone still shows up, and a last line without a newline carries the
    /// `\ No newline at end of file` marker that diff and git use.
    pub fn generate_unified_diff(path: &str, old_content: &str, new_content: &str) -> String {
        let mut diff = format!("--- a/{}\n+++ b/{}\n", path, path);

        let old_lines = split_lines(old_content);
        let new_lines = split_lines(new_content);

        diff.push_str(&format!(
            "@@ -1,{} +1,{} @@\n",
            old_lines.len(),
            new_lines.len()
        ));

        // Simple line diff representation
        push_lines(&mut diff, '-', &old_lines);
        push_lines(&mut diff, '+', &new_lines);

        diff
    }
}

/// Lines with their terminators (`\n` or `\r\n`) still attached.
fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        Vec::new()
    } else {
        content.split_inclusive('\n').collect()
    }
}

fn push_lines(diff: &mut String, sign: char, lines: &[&str]) {
    for line in lines {
        diff.push(sign);
        diff.push_str(line);
        if !line.ends_with('\n') {
            diff.push_str("\n\\ No newline at end of file\n");
        }
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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required 'path' parameter".to_string())?;

        let new_content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required 'content' parameter".to_string())?;

        // Resolved on the real filesystem: a link out of the workspace, a
        // FIFO or a directory at the path is refused before anything is read.
        let workspace = Workspace::new(&ctx.workspace_root)?;
        let pending = workspace.prepare_write(path_str)?;
        let preimage_hash = pending.preimage_hash.clone();
        let old_content = String::from_utf8_lossy(&pending.preimage).into_owned();
        let diff = Self::generate_unified_diff(path_str, &old_content, new_content);

        // The postimage is the hash of what reading the file back returned,
        // never of `new_content`: a short write, an error the filesystem
        // reports only at close, or anything rewriting the file first would
        // otherwise be attested as done. A mismatch fails the write.
        let postimage_hash = workspace.commit_write(pending, new_content.as_bytes())?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{file_link, Scratch};
    use crate::workspace_fs::{sha256_hex, ABSENT_PREIMAGE_HASH};
    use std::fs;

    async fn write(s: &Scratch, path: &str, content: &str) -> Result<ToolResult, String> {
        WriteFileTool
            .execute(json!({ "path": path, "content": content }), &s.ctx())
            .await
    }

    #[tokio::test]
    async fn test_postimage_is_the_hash_of_the_bytes_on_disk() {
        let s = Scratch::new("write_postimage");
        let content = "line one\r\nline two\nno newline";
        let res = write(&s, "src/lib.rs", content).await.expect("write");
        let on_disk = fs::read(s.ws().join("src/lib.rs")).unwrap();
        assert_eq!(on_disk, content.as_bytes(), "bytes must land exactly");
        assert_eq!(res.postimage_hash, Some(sha256_hex(&on_disk)));
        assert_eq!(res.preimage_hash.as_deref(), Some(ABSENT_PREIMAGE_HASH));

        let res = write(&s, "src/lib.rs", "replaced\n").await.expect("rewrite");
        assert_eq!(res.preimage_hash, Some(sha256_hex(&on_disk)));
        assert_eq!(res.postimage_hash, Some(sha256_hex(b"replaced\n")));
    }

    #[tokio::test]
    async fn test_write_through_a_link_out_of_the_workspace_is_denied() {
        let s = Scratch::new("write_link_out");
        let target = s.outside().join("secret.txt");
        if !file_link(&target, &s.ws().join("notes.txt")) {
            eprintln!("skipped: this machine cannot create symbolic links");
            return;
        }
        let err = write(&s, "notes.txt", "pwned\n").await.unwrap_err();
        assert!(err.contains("Access denied"), "{}", err);
        assert_eq!(fs::read_to_string(&target).unwrap(), "TOP SECRET\n");
    }

    #[test]
    fn test_diff_keeps_line_endings() {
        let diff = WriteFileTool::generate_unified_diff("f", "a\nb\n", "a\r\nb\r\n");
        assert!(diff.contains("-a\n-b\n+a\r\n+b\r\n"), "{:?}", diff);

        let diff = WriteFileTool::generate_unified_diff("f", "a\n", "a");
        assert!(diff.ends_with("+a\n\\ No newline at end of file\n"), "{:?}", diff);

        let diff = WriteFileTool::generate_unified_diff("f", "", "x\n");
        assert!(diff.contains("@@ -1,0 +1,1 @@\n+x\n"), "{:?}", diff);
    }
}
