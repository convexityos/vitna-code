//! Brokered tool definitions, schemas, path safety, and execution dispatch.

pub mod list_dir;
pub mod path_safety;
pub mod read_file;
pub mod run_command;
pub mod write_file;

use async_trait::async_trait;
pub use list_dir::ListDirTool;
pub use path_safety::resolve_workspace_path;
pub use read_file::ReadFileTool;
pub use run_command::RunCommandTool;
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

    /// Creates standard tool suite: read_file, write_file, list_dir, run_command.
    pub fn standard() -> Self {
        let mut registry = Self::new();
        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(WriteFileTool));
        registry.register(Arc::new(ListDirTool));
        registry.register(Arc::new(RunCommandTool));
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
        assert_eq!(defs.len(), 4);

        let names: Vec<String> = defs.into_iter().map(|d| d.name).collect();
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&"write_file".to_string()));
        assert!(names.contains(&"list_dir".to_string()));
        assert!(names.contains(&"run_command".to_string()));

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
        assert!(write_res.diff.is_some());
        assert!(write_res.postimage_hash.is_some());

        // 2. Read file
        let read_args = serde_json::json!({
            "path": "test.txt"
        });
        let read_res = registry.execute("read_file", read_args, &ctx).await.expect("read succeeds");
        assert!(read_res.success);
        assert!(read_res.output.contains("Hello Vitna!"));

        // 3. List dir
        let list_args = serde_json::json!({});
        let list_res = registry.execute("list_dir", list_args, &ctx).await.expect("list succeeds");
        assert!(list_res.success);
        assert!(list_res.output.contains("test.txt"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}