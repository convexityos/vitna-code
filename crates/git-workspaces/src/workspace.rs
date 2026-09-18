use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWorkspace {
    pub workspace_id: String,
    pub primary_repo_root: PathBuf,
    pub agent_workspace_root: PathBuf,
    pub base_commit_sha: String,
    pub branch_name: String,
    pub is_ephemeral: bool,
}

pub struct AgentWorkspaceManager;

impl AgentWorkspaceManager {
    /// Creates an independent, disposable workspace for an agent run,
    /// enforcing the invariant that write-capable agents never share a mutable workspace with the operator.
    pub fn create_isolated_workspace<P: AsRef<Path>>(
        primary_repo: P,
        run_id: &str,
    ) -> Result<AgentWorkspace, String> {
        let primary = primary_repo.as_ref().to_path_buf();
        if !primary.exists() {
            return Err(format!("Primary repository path does not exist: {}", primary.display()));
        }

        let workspace_id = format!("ws-agent-{}", run_id);
        let branch_name = format!("vitna/agent-{}", run_id);

        let ws_dir = primary
            .join(".vitna")
            .join("workspaces")
            .join(format!("agent-{}", run_id));

        fs::create_dir_all(&ws_dir)
            .map_err(|e| format!("Failed to create isolated workspace directory: {}", e))?;

        // Determine base commit SHA or workspace fingerprint
        let base_commit_sha = Self::determine_base_commit(&primary);

        // Populate isolated workspace with initial file tree
        Self::copy_workspace_contents(&primary, &ws_dir)?;

        Ok(AgentWorkspace {
            workspace_id,
            primary_repo_root: primary,
            agent_workspace_root: ws_dir,
            base_commit_sha,
            branch_name,
            is_ephemeral: true,
        })
    }

    /// Recursively copies non-ignored workspace files into the disposable agent workspace.
    fn copy_workspace_contents(src: &Path, dst: &Path) -> Result<(), String> {
        if !src.is_dir() {
            return Ok(());
        }

        let entries = fs::read_dir(src).map_err(|e| e.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            // Skip VCS metadata, build caches, and vitna internal directories
            if name_str == ".git"
                || name_str == ".vitna"
                || name_str == "target"
                || name_str == "node_modules"
            {
                continue;
            }

            let src_path = entry.path();
            let dst_path = dst.join(&file_name);

            if src_path.is_dir() {
                fs::create_dir_all(&dst_path).map_err(|e| e.to_string())?;
                Self::copy_workspace_contents(&src_path, &dst_path)?;
            } else if src_path.is_file() {
                fs::copy(&src_path, &dst_path).map_err(|e| e.to_string())?;
            }
        }

        Ok(())
    }

    fn determine_base_commit(primary_repo: &Path) -> String {
        // Try git rev-parse HEAD if git is present. The repository is untrusted
        // input, so this goes through the hardened invocation rather than a bare
        // `git`; when the protections cannot be established we fall through to
        // the fingerprint below rather than running git unprotected.
        match crate::host_git::command(primary_repo) {
            Ok(mut cmd) => {
                let output = cmd.arg("rev-parse").arg("HEAD").output();

                if let Ok(out) = output {
                    if out.status.success() {
                        let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if sha.len() == 40 {
                            return sha;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Skipping git base-commit detection for {}: {}",
                    primary_repo.display(),
                    e
                );
            }
        }

        // Fallback: SHA-256 hash of primary repo path + current timestamp
        let mut hasher = Sha256::new();
        hasher.update(primary_repo.to_string_lossy().as_bytes());
        let hex_hash = hex::encode(hasher.finalize());
        hex_hash[..40].to_string()
    }

    /// Cleans up and removes an isolated agent workspace.
    pub fn cleanup_workspace(workspace: &AgentWorkspace) -> Result<(), String> {
        if workspace.agent_workspace_root.exists() {
            fs::remove_dir_all(&workspace.agent_workspace_root)
                .map_err(|e| format!("Failed to delete agent workspace: {}", e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_workspace_lifecycle() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_ws_test_{}", std::process::id()));
        fs::create_dir_all(temp_dir.join("src")).expect("create primary src");
        fs::write(temp_dir.join("src/main.rs"), "fn main() {}\n").expect("write primary file");

        let ws = AgentWorkspaceManager::create_isolated_workspace(&temp_dir, "run-99")
            .expect("create workspace");

        assert!(ws.agent_workspace_root.exists());
        assert!(ws.agent_workspace_root.join("src/main.rs").exists());
        assert_eq!(ws.base_commit_sha.len(), 40);

        // Modifying agent workspace does NOT touch primary checkout
        fs::write(ws.agent_workspace_root.join("src/main.rs"), "fn main() { altered(); }\n")
            .expect("modify in agent ws");

        let primary_content = fs::read_to_string(temp_dir.join("src/main.rs")).expect("read primary");
        assert_eq!(primary_content, "fn main() {}\n", "Primary checkout must remain untouched");

        // Clean up
        AgentWorkspaceManager::cleanup_workspace(&ws).expect("cleanup");
        assert!(!ws.agent_workspace_root.exists());

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
