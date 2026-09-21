use crate::workspace_fs::Workspace;
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;

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

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let path_str = args
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or_else(|| "Missing required 'path' parameter".to_string())?;

        let new_content = args
            .get("content")
            .and_then(|c| c.as_str())
            .ok_or_else(|| "Missing required 'content' parameter".to_string())?;

        // Same containment as write_file. The preimage is read through the
        // verified handle that the write will use, and nothing on disk
        // changes until the preimage check below has passed.
        let workspace = Workspace::new(&ctx.workspace_root)?;
        let pending = workspace.prepare_write(path_str)?;
        let current_preimage = pending.preimage_hash.clone();

        // Preimage validation: abort if file changed under the agent
        if let Some(expected) = args.get("expected_preimage_hash").and_then(|e| e.as_str()) {
            if expected != current_preimage {
                return Err(format!(
                    "Patch conflict: expected preimage '{}' does not match current file hash '{}'",
                    expected, current_preimage
                ));
            }
        }

        let old_content = String::from_utf8_lossy(&pending.preimage).into_owned();
        let diff = crate::write_file::WriteFileTool::generate_unified_diff(path_str, &old_content, new_content);

        // Hashed from the bytes read back after the write, as in write_file.
        let postimage_hash = workspace.commit_write(pending, new_content.as_bytes())?;

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
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{dir_link, Scratch};
    use crate::workspace_fs::sha256_hex;
    use std::fs;

    async fn patch(s: &Scratch, args: serde_json::Value) -> Result<ToolResult, String> {
        ApplyPatchTool.execute(args, &s.ctx()).await
    }

    #[tokio::test]
    async fn test_patch_preserves_crlf_and_hashes_the_bytes_on_disk() {
        let s = Scratch::new("patch_crlf");
        fs::write(s.ws().join("a.txt"), "one\r\ntwo\r\n").unwrap();
        let before = sha256_hex(b"one\r\ntwo\r\n");

        let res = patch(
            &s,
            json!({ "path": "a.txt", "content": "one\r\n2\r\n", "expected_preimage_hash": before }),
        )
        .await
        .expect("patch");
        let on_disk = fs::read(s.ws().join("a.txt")).unwrap();
        assert_eq!(on_disk, b"one\r\n2\r\n");
        assert_eq!(res.preimage_hash, Some(before));
        assert_eq!(res.postimage_hash, Some(sha256_hex(&on_disk)));
    }

    #[tokio::test]
    async fn test_conflicting_preimage_changes_nothing() {
        let s = Scratch::new("patch_conflict");
        fs::write(s.ws().join("a.txt"), "current\n").unwrap();
        let err = patch(
            &s,
            json!({ "path": "a.txt", "content": "new\n", "expected_preimage_hash": "0".repeat(64) }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("Patch conflict"), "{}", err);
        assert_eq!(fs::read_to_string(s.ws().join("a.txt")).unwrap(), "current\n");

        // A conflicting patch to a new file leaves no file and no directories.
        let err = patch(
            &s,
            json!({ "path": "sub/b.txt", "content": "x", "expected_preimage_hash": sha256_hex(b"y") }),
        )
        .await
        .unwrap_err();
        assert!(err.contains("Patch conflict"), "{}", err);
        assert!(!s.ws().join("sub").exists());
    }

    #[tokio::test]
    async fn test_patch_into_a_directory_link_out_of_the_workspace_is_denied() {
        let s = Scratch::new("patch_dir_link_out");
        let outside = s.outside();
        if !dir_link(&outside, &s.ws().join("home")) {
            eprintln!("skipped: this machine cannot create directory links");
            return;
        }
        let err = patch(&s, json!({ "path": "home/secret.txt", "content": "pwned\n" }))
            .await
            .unwrap_err();
        assert!(err.contains("Access denied"), "{}", err);
        let err = patch(&s, json!({ "path": "home/new.txt", "content": "pwned\n" }))
            .await
            .unwrap_err();
        assert!(err.contains("Access denied"), "{}", err);
        assert_eq!(fs::read_to_string(outside.join("secret.txt")).unwrap(), "TOP SECRET\n");
        assert!(!outside.join("new.txt").exists());
    }
}
