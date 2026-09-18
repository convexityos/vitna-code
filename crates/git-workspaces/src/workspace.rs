use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

mod materialize;

pub use materialize::RefusedEntry;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentWorkspace {
    pub workspace_id: String,
    pub primary_repo_root: PathBuf,
    pub agent_workspace_root: PathBuf,
    pub base_commit_sha: String,
    pub branch_name: String,
    pub is_ephemeral: bool,
    /// Entries of the primary checkout left out of this workspace, and why:
    /// links that lead outside it, and FIFOs, sockets and devices.
    #[serde(default)]
    pub refused_entries: Vec<RefusedEntry>,
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

        let ws_dir = Self::create_workspace_dir(&primary, run_id)?;

        // Determine base commit SHA or workspace fingerprint
        let base_commit_sha = Self::determine_base_commit(&primary);

        // Populate isolated workspace with initial file tree. Links are
        // recreated or refused, never followed; refusals are recorded.
        let refused_entries = materialize::copy_tree(&primary, &ws_dir)?;

        Ok(AgentWorkspace {
            workspace_id,
            primary_repo_root: primary,
            agent_workspace_root: ws_dir,
            base_commit_sha,
            branch_name,
            is_ephemeral: true,
            refused_entries,
        })
    }

    /// Creates `.vitna/workspaces/agent-<run_id>` inside the primary checkout,
    /// fresh. The repository decides what already sits at those names: a
    /// committed `.vitna` link must not choose where a workspace lands, and a
    /// directory that already exists must not be filled on top of.
    fn create_workspace_dir(primary: &Path, run_id: &str) -> Result<PathBuf, String> {
        let state_dir = primary.join(".vitna");
        let parent = state_dir.join("workspaces");
        for dir in [&state_dir, &parent] {
            if fs::symlink_metadata(dir).is_ok_and(|meta| !meta.is_dir()) {
                return Err(format!(
                    "Refusing to create an agent workspace: {} is not a real directory",
                    dir.display()
                ));
            }
        }
        fs::create_dir_all(&parent)
            .map_err(|e| format!("Failed to create isolated workspace directory: {}", e))?;

        let name = format!("agent-{}", run_id);
        let ws_dir = parent.join(&name);
        fs::create_dir(&ws_dir).map_err(|e| {
            format!(
                "Failed to create isolated workspace directory {}: {}",
                ws_dir.display(),
                e
            )
        })?;

        // Whatever raced the checks above, the workspace must be exactly
        // where it was asked to be.
        let expected = fs::canonicalize(primary)
            .map_err(|e| format!("Cannot resolve primary repository: {}", e))?
            .join(".vitna")
            .join("workspaces")
            .join(&name);
        let actual = fs::canonicalize(&ws_dir)
            .map_err(|e| format!("Cannot resolve agent workspace: {}", e))?;
        if !(materialize::is_within(&actual, &expected) && materialize::is_within(&expected, &actual)) {
            return Err(format!(
                "Refusing to use agent workspace {}: it resolves to {}",
                ws_dir.display(),
                actual.display()
            ));
        }
        Ok(ws_dir)
    }

    fn determine_base_commit(primary_repo: &Path) -> String {
        // Try git rev-parse HEAD if git is present
        let output = std::process::Command::new("git")
            .arg("rev-parse")
            .arg("HEAD")
            .current_dir(primary_repo)
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if sha.len() == 40 {
                    return sha;
                }
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
        assert!(ws.refused_entries.is_empty(), "{:?}", ws.refused_entries);

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

    #[test]
    fn test_refused_links_are_recorded_on_the_workspace() {
        let base = std::env::temp_dir().join(format!("vitna_ws_refused_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let primary = base.join("repo");
        fs::create_dir_all(primary.join("src")).expect("create primary src");
        fs::create_dir_all(base.join("home/.ssh")).expect("create outside dir");
        fs::write(base.join("home/.ssh/id_rsa"), "PRIVATE KEY\n").expect("write key");
        if !materialize::tests::dir_link(&base.join("home/.ssh"), &primary.join("notes")) {
            eprintln!("skipped: this machine cannot create directory links");
            let _ = fs::remove_dir_all(&base);
            return;
        }

        let ws = AgentWorkspaceManager::create_isolated_workspace(&primary, "run-refused")
            .expect("create workspace");
        assert_eq!(ws.refused_entries.len(), 1, "{:?}", ws.refused_entries);
        assert_eq!(ws.refused_entries[0].path, "notes");
        assert!(!ws.agent_workspace_root.join("notes").exists());

        AgentWorkspaceManager::cleanup_workspace(&ws).expect("cleanup");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_committed_state_link_cannot_place_the_workspace() {
        let base = std::env::temp_dir().join(format!("vitna_ws_state_link_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let primary = base.join("repo");
        let elsewhere = base.join("elsewhere");
        fs::create_dir_all(&primary).expect("create primary");
        fs::create_dir_all(&elsewhere).expect("create elsewhere");
        fs::write(primary.join("a.txt"), "a\n").expect("write primary file");
        if !materialize::tests::dir_link(&elsewhere, &primary.join(".vitna")) {
            eprintln!("skipped: this machine cannot create directory links");
            let _ = fs::remove_dir_all(&base);
            return;
        }

        let err = AgentWorkspaceManager::create_isolated_workspace(&primary, "run-1").unwrap_err();
        assert!(err.contains("not a real directory"), "{}", err);
        assert!(
            fs::read_dir(&elsewhere).expect("list elsewhere").next().is_none(),
            "nothing may be created through the link"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn test_existing_workspace_directory_is_not_reused() {
        let temp_dir =
            std::env::temp_dir().join(format!("vitna_ws_reuse_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(temp_dir.join(".vitna/workspaces/agent-run-7")).expect("pre-seed");

        let err = AgentWorkspaceManager::create_isolated_workspace(&temp_dir, "run-7").unwrap_err();
        assert!(err.contains("agent-run-7"), "{}", err);

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
