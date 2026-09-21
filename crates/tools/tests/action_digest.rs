//! What an action digest must mean, because an approval is bound to it.
//!
//! CONTRIBUTING's third rule makes the digest the identity of an action: an
//! operator approves a digest, and what they approved is whatever produces
//! that digest. So two different actions sharing one is not a hashing detail.
//! It is an approval for one action that also covers another.
//!
//! Two independent schemes existed, and each had its own collision class:
//!
//! - `write_file`, `apply_patch` and `run_command` joined their fields with an
//!   unescaped `:`, so writing `c` to the file `a:b` and writing `b:c` to the
//!   file `a` both canonicalized to `write_file:a:b:c`. On Windows, where a
//!   path routinely carries a drive colon, `run_command`'s working directory
//!   reaches this without trying.
//! - The other five hashed their arguments with no tool name, so
//!   `read_file {"path":"x"}` and `list_dir {"path":"x"}` were one action.
//!
//! What these tests do not check: whether the digest binds everything
//! CONTRIBUTING's rule lists. It does not yet bind the resolved executable,
//! the canonical cwd, environment names, mounts or limits, which live in the
//! runner rather than in a tool's arguments. That is a separate, larger
//! change, and this file says so rather than implying the rule is met.

use serde_json::json;
use std::collections::HashMap;
use vitna_tools::{
    ApplyPatchTool, BrowserVerifyTool, GitStatusTool, ListDirTool, ReadFileTool, RunCommandTool,
    SearchCodeTool, Tool, WriteFileTool,
};

/// The built-in tools, for the behavioural tests below.
///
/// A hand-written list, and therefore incomplete by construction: it missed
/// the ninth `Tool` in this workspace, `vitna_mcp::McpToolBridge`, which carried
/// the same separator flaw. So the behavioural tests here are paired with
/// [`every_digest_in_the_workspace_goes_through_the_shared_function`], which
/// finds its subjects by scanning rather than by being told.
fn every_tool() -> Vec<Box<dyn Tool>> {
    vec![
        Box::new(ReadFileTool),
        Box::new(WriteFileTool),
        Box::new(ListDirTool),
        Box::new(RunCommandTool),
        Box::new(SearchCodeTool),
        Box::new(GitStatusTool),
        Box::new(ApplyPatchTool),
        Box::new(BrowserVerifyTool),
    ]
}

/// Every place any crate DEFINES a digest must route it through
/// `vitna_tools::action_digest` or `vitna_tools::canonical_digest`.
///
/// Deny by default over whatever exists, because the flaws this file tests for
/// were each introduced by a tool writing its own digest, and the next tool to
/// do so will not be in anyone's list. It scans every `src/` tree under
/// `crates/` and `apps/`, so a tool added in a new crate is covered without
/// this file changing.
///
/// What it cannot see: a digest computed somewhere that is not a method named
/// `compute_action_digest`. It identifies its subjects by that name, which is
/// a proxy, and says so here rather than pretending otherwise.
#[test]
fn every_digest_in_the_workspace_goes_through_the_shared_function() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("crates/tools sits two levels below the repository root")
        .to_path_buf();

    // Spelled in two halves so this file does not match its own search.
    let needle = concat!("fn compute_", "action_digest(");

    let mut sites = 0usize;
    let mut violations = Vec::new();

    for top in ["crates", "apps"] {
        for file in rust_sources_under_src(&root.join(top)) {
            let source = std::fs::read_to_string(&file).expect("read source");
            let mut from = 0;
            while let Some(offset) = source[from..].find(needle) {
                let at = from + offset;
                let body = body_after(&source, at);
                sites += 1;

                let delegates =
                    body.contains("action_digest(") || body.contains("canonical_digest(");
                let hand_rolled = body.contains("Sha256") || body.contains("format!(");
                if !delegates || hand_rolled {
                    let line = source[..at].lines().count();
                    violations.push(format!(
                        "{}:{line} computes its own digest",
                        file.strip_prefix(&root).unwrap_or(&file).display()
                    ));
                }
                from = at + needle.len();
            }
        }
    }

    // The scanner itself: a search that silently found nothing would pass by
    // comparing zero sites against zero violations.
    assert!(
        sites >= 2,
        "found only {sites} digest definitions; the scan is not reaching the \
         trait default and the MCP bridge, so it is not checking anything"
    );
    assert!(
        violations.is_empty(),
        "a digest must go through vitna_tools::action_digest or \
         canonical_digest, never its own join or hash:\n  {}",
        violations.join("\n  ")
    );
}

/// Every `.rs` file under any `src/` directory below `dir`, skipping build
/// output and dependencies.
fn rust_sources_under_src(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if matches!(name.as_ref(), "target" | "node_modules" | ".git" | "tests") {
                    continue;
                }
                stack.push(path);
            } else if name.ends_with(".rs") && path.components().any(|c| c.as_os_str() == "src") {
                out.push(path);
            }
        }
    }
    out
}

/// The text of the function body that starts after `at`, by brace depth.
///
/// Naive about braces inside string literals, which is acceptable for bodies
/// this short and balanced; a pathological literal would fail closed by
/// swallowing too much, which reports a violation rather than hiding one.
fn body_after(source: &str, at: usize) -> &str {
    let Some(open) = source[at..].find('{') else {
        return "";
    };
    let start = at + open;
    let mut depth = 0i32;
    for (i, ch) in source[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=start + i];
                }
            }
            _ => {}
        }
    }
    &source[start..]
}

#[test]
fn a_separator_inside_an_argument_cannot_make_two_writes_one_action() {
    let first = WriteFileTool.compute_action_digest(&json!({ "path": "a:b", "content": "c" }));
    let second = WriteFileTool.compute_action_digest(&json!({ "path": "a", "content": "b:c" }));

    assert_ne!(
        first, second,
        "writing `c` to `a:b` and writing `b:c` to `a` are different actions, so \
         an approval for one must not cover the other"
    );
}

#[test]
fn a_drive_colon_cannot_make_two_commands_one_action() {
    // The same bytes split differently between the command and the directory
    // it runs in. Joined with an unescaped colon, both of these canonicalized
    // to `run_command:echo:C:\tmp`. On Windows a working directory carries a
    // drive colon by default, so the split point is ambiguous without anyone
    // trying to make it so.
    let first = RunCommandTool.compute_action_digest(&json!({
        "command": "echo",
        "working_directory": "C:\\tmp",
    }));
    let second = RunCommandTool.compute_action_digest(&json!({
        "command": "echo:C",
        "working_directory": "\\tmp",
    }));

    assert_ne!(
        first, second,
        "a different command in a different directory is a different action"
    );
}

/// `run_command` reads `timeout_ms` and its digest used to ignore it, so an
/// approval for a command with a 30 second limit also approved that command
/// with any limit at all. CONTRIBUTING's rule names limits explicitly. A digest
/// built from a hand-picked list of fields is how one gets left out, which is
/// why the digest now covers every argument the tool is given.
#[test]
fn a_command_limit_is_part_of_the_action() {
    let bounded = RunCommandTool.compute_action_digest(&json!({
        "command": "cargo test",
        "timeout_ms": 30_000,
    }));
    let unbounded = RunCommandTool.compute_action_digest(&json!({
        "command": "cargo test",
        "timeout_ms": 86_400_000,
    }));

    assert_ne!(
        bounded, unbounded,
        "approving a command with one time limit must not approve it with another"
    );
}

#[test]
fn apply_patch_has_no_separator_collision_either() {
    let first = ApplyPatchTool.compute_action_digest(&json!({ "path": "a:b", "content": "c" }));
    let second = ApplyPatchTool.compute_action_digest(&json!({ "path": "a", "content": "b:c" }));
    assert_ne!(first, second);
}

/// The tool is part of the action. Reading a path and listing it are not the
/// same thing to approve, even when the arguments are identical.
#[test]
fn no_two_tools_share_a_digest_for_the_same_arguments() {
    let args = json!({ "path": "x", "content": "y", "command": "z", "query": "q" });

    let mut seen: HashMap<String, String> = HashMap::new();
    for tool in every_tool() {
        let digest = tool.compute_action_digest(&args);
        if let Some(other) = seen.insert(digest, tool.name().to_string()) {
            panic!(
                "{} and {} produce the same digest for identical arguments, so an \
                 approval for one is an approval for the other",
                other,
                tool.name()
            );
        }
    }
}

/// JSON object keys have no order. A digest that depended on it would give
/// one action two identities, and an approval would stop matching the action
/// it was granted for depending on how the arguments were assembled.
#[test]
fn argument_key_order_does_not_change_the_digest() {
    let a: serde_json::Value = serde_json::from_str(r#"{"path":"p","content":"c"}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"content":"c","path":"p"}"#).unwrap();

    for tool in every_tool() {
        assert_eq!(
            tool.compute_action_digest(&a),
            tool.compute_action_digest(&b),
            "{}: key order changed the digest",
            tool.name()
        );
    }
}

#[test]
fn nested_key_order_does_not_change_the_digest_either() {
    let a: serde_json::Value = serde_json::from_str(r#"{"opts":{"x":1,"y":2}}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"opts":{"y":2,"x":1}}"#).unwrap();
    for tool in every_tool() {
        assert_eq!(
            tool.compute_action_digest(&a),
            tool.compute_action_digest(&b)
        );
    }
}

#[test]
fn any_change_to_an_argument_changes_the_digest() {
    let base = json!({ "path": "p", "content": "c" });
    let changed = json!({ "path": "p", "content": "c2" });
    for tool in every_tool() {
        assert_ne!(
            tool.compute_action_digest(&base),
            tool.compute_action_digest(&changed),
            "{}: a different argument produced the same digest",
            tool.name()
        );
    }
}

#[test]
fn the_digest_is_a_sha256_hex_string() {
    for tool in every_tool() {
        let digest = tool.compute_action_digest(&json!({ "path": "p" }));
        assert_eq!(digest.len(), 64, "{}: not 32 bytes of hex", tool.name());
        assert!(
            digest
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{}: not lowercase hex",
            tool.name()
        );
    }
}
