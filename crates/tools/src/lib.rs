//! Brokered tool definitions, schemas, path safety, and execution dispatch.

pub mod apply_patch;
pub mod browser_verify;
pub mod git_status;
pub mod list_dir;
pub mod path_safety;
pub mod read_file;
pub mod run_command;
pub mod search_code;
#[cfg(test)]
mod test_support;
mod workspace_fs;
pub mod write_file;

pub use apply_patch::ApplyPatchTool;
use async_trait::async_trait;
pub use browser_verify::BrowserVerifyTool;
pub use git_status::GitStatusTool;
pub use list_dir::ListDirTool;
pub use path_safety::resolve_workspace_path;
pub use read_file::ReadFileTool;
pub use run_command::RunCommandTool;
pub use search_code::SearchCodeTool;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use vitna_runner::Runner;
pub use write_file::WriteFileTool;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters_schema: serde_json::Value,
    pub is_mutating: bool,
    pub requires_approval: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub tool_name: String,
    pub success: bool,
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preimage_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub postimage_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The sandbox that confined this tool's work, when it ran a process at
    /// all. Carried so the receipt records what was applied rather than what
    /// the configuration hoped for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_enforcement: Option<String>,
}

pub struct ToolContext {
    pub workspace_root: PathBuf,
    pub runner: Arc<dyn Runner>,
    /// Explicit operator approval to run a command with no OS sandbox.
    ///
    /// Separate from any general auto-approve, and false unless a human said
    /// yes to this specific thing. `ToolContext::new` is the way to build one
    /// so that adding a permission is always a visible edit.
    pub allow_unsandboxed: bool,
    pub allow_network: bool,
}

impl ToolContext {
    /// A context that grants nothing beyond the workspace.
    pub fn new(workspace_root: PathBuf, runner: Arc<dyn Runner>) -> Self {
        Self {
            workspace_root,
            runner,
            allow_unsandboxed: false,
            allow_network: false,
        }
    }
}

/// The identity of an action: which tool, given exactly which arguments.
///
/// An approval is bound to this value, so what an operator approves is every
/// action that produces it. Two properties follow, and each was once broken:
///
/// - **The tool is part of the action.** Five tools hashed their arguments
///   alone, so reading a path and listing it were one action to approve.
/// - **Every argument is part of the action, in an encoding with one reading.**
///   Three tools joined a hand-picked list of fields with an unescaped `:`.
///   The join made `c` written to `a:b` and `b:c` written to `a` the same
///   action, and the hand-picked list left out `run_command`'s `timeout_ms`,
///   so approving a command with one time limit approved it with any. Hashing
///   the whole argument object removes the list, and with it the field that
///   gets forgotten.
///
/// The encoding is JSON with object keys sorted at every depth, done here
/// rather than left to `serde_json`'s map type: its order depends on whether
/// any crate in the build enables `preserve_order`, and Cargo unifies features
/// across the workspace, so a dependency added for an unrelated reason would
/// silently re-key every approval.
///
/// What this does NOT bind, and CONTRIBUTING's rule says it should: the
/// resolved executable, the canonical working directory, environment names,
/// mounts and runner limits. Those are decided in the runner, not carried in a
/// tool's arguments, so binding them is a change to where the digest is
/// computed rather than to how.
pub fn action_digest(tool_name: &str, args: &serde_json::Value) -> String {
    canonical_digest(&serde_json::json!({ "tool": tool_name, "args": args }))
}

/// SHA-256 of a structured identity, canonically encoded.
///
/// For a tool whose identity is more than its name. An MCP tool is one: two
/// servers can expose a tool of the same name, so the server belongs in the
/// identity too. Pass it as a FIELD of `identity`, never spliced into the name
/// with a separator, because splicing is the flaw [`action_digest`] exists to
/// remove: server `a:b` exposing `c` and server `a` exposing `b:c` would be one
/// action again.
pub fn canonical_digest(identity: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    let mut canonical = String::new();
    write_canonical(identity, &mut canonical);
    hex::encode(Sha256::digest(canonical.as_bytes()))
}

/// Writes `value` as JSON with object keys sorted at every depth.
///
/// Scalars go through `serde_json`, which escapes strings, so a quote, colon
/// or brace inside a value can never be read back as structure. That escaping
/// is the whole fix for the separator collisions.
fn write_canonical(value: &serde_json::Value, out: &mut String) {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;

    /// The digest an approval of this call is bound to.
    ///
    /// Provided, and meant to stay that way. Every tool once wrote its own,
    /// which is how three schemes with three different flaws came to coexist.
    /// A tool that overrides this narrows what an operator is shown relative
    /// to what runs, so an override needs a reason written next to it.
    fn compute_action_digest(&self, args: &serde_json::Value) -> String {
        action_digest(self.name(), args)
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String>;
}

#[derive(Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self.tools.values().map(|t| t.definition()).collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Creates standard tool suite: read_file, write_file, list_dir, run_command, search_code, git_status, apply_patch, browser_verify.
    pub fn standard() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(ListDirTool));
        registry.register(Arc::new(RunCommandTool));
        registry.register(Arc::new(SearchCodeTool));
        registry.register(Arc::new(GitStatusTool));
        registry.register(Arc::new(ApplyPatchTool));
        registry.register(Arc::new(BrowserVerifyTool));
        registry
    }

    pub async fn execute(
        &self,
        name: &str,
        args: serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<ToolResult, String> {
        let tool = self
            .get(name)
            .ok_or_else(|| format!("Unknown tool: {}", name))?;
        tool.execute(args, ctx).await
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use vitna_runner::FakeRunner;

    #[tokio::test]
    async fn test_tool_registry_lifecycle() {
        let registry = ToolRegistry::standard();
        let defs = registry.definitions();
        assert_eq!(defs.len(), 8);

        let names: Vec<String> = defs.into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"write_file".to_string()));
        assert!(names.contains(&"list_dir".to_string()));
        assert!(names.contains(&"run_command".to_string()));
        assert!(names.contains(&"search_code".to_string()));
        assert!(names.contains(&"git_status".to_string()));
        assert!(names.contains(&"apply_patch".to_string()));
        assert!(names.contains(&"browser_verify".to_string()));

        let temp_dir = std::env::temp_dir().join(format!("vitna_tools_test_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        let journal_path = temp_dir.join("test.journal");
        let fake_runner = FakeRunner::new(&journal_path).expect("open fake runner");
        let runner = Arc::new(std::sync::Mutex::new(fake_runner));

        let ctx = ToolContext::new(temp_dir.clone(), runner);

        // 1. Write file
        let write_args = serde_json::json!({
            "path": "test.txt",
            "content": "Hello Vitna!\nLine 2\n"
        });
        let write_res = registry.execute("write_file", write_args, &ctx).await.expect("write succeeds");
        assert!(write_res.success);

        // 2. Search code
        let search_args = serde_json::json!({
            "query": "Vitna"
        });
        let search_res = registry.execute("search_code", search_args, &ctx).await.expect("search succeeds");
        assert!(search_res.success);
        assert!(search_res.output.contains("test.txt:1: Hello Vitna!"));

        // 3. Apply patch
        let patch_args = serde_json::json!({
            "path": "test.txt",
            "content": "Hello Vitna World!\nLine 2\n",
            "expected_preimage_hash": write_res.postimage_hash.unwrap()
        });
        let patch_res = registry.execute("apply_patch", patch_args, &ctx).await.expect("patch succeeds");
        assert!(patch_res.success);
        assert!(patch_res.diff.is_some());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}