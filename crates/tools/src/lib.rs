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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

pub struct ToolContext {
    pub workspace_root: PathBuf,
    pub runner: Arc<dyn Runner>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    fn compute_action_digest(&self, args: &serde_json::Value) -> String;
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

        let ctx = ToolContext {
            workspace_root: temp_dir.clone(),
            runner,
        };

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