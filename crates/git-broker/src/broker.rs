use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use vitna_git_workspaces::AgentWorkspace;

pub const NULL_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub preimage_hash: String,
    pub postimage_hash: String,
    pub diff: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    pub changeset_id: String,
    pub base_commit_sha: String,
    pub files: Vec<FileChange>,
    pub combined_diff: String,
    pub diff_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeConflict {
    pub path: String,
    pub expected_preimage: String,
    pub actual_current_hash: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeResult {
    pub success: bool,
    pub applied_files: Vec<String>,
    pub conflicts: Vec<MergeConflict>,
    pub new_commit_sha: Option<String>,
}

pub struct GitBroker;

impl GitBroker {
    /// Inspects differences between the isolated agent workspace and primary checkout,
    /// calculating cryptographic preimages and postimages for every modified file.
    pub fn inspect_changes(agent_ws: &AgentWorkspace) -> Result<ChangeSet, String> {
        let mut changes = Vec::new();
        let mut all_diffs = String::new();

        Self::scan_directory(
            &agent_ws.agent_workspace_root,
            &agent_ws.primary_repo_root,
            &agent_ws.agent_workspace_root,
            &mut changes,
            &mut all_diffs,
        )?;

        let diff_digest = hex::encode(Sha256::digest(all_diffs.as_bytes()));
        let changeset_id = format!("cs-{}", &diff_digest[..16]);

        Ok(ChangeSet {
            changeset_id,
            base_commit_sha: agent_ws.base_commit_sha.clone(),
            files: changes,
            combined_diff: all_diffs,
            diff_digest,
        })
    }

    fn scan_directory(
        base_agent: &Path,
        base_primary: &Path,
        current_agent_dir: &Path,
        changes: &mut Vec<FileChange>,
        all_diffs: &mut String,
    ) -> Result<(), String> {
        if !current_agent_dir.is_dir() {
            return Ok(());
        }

        let entries = fs::read_dir(current_agent_dir).map_err(|e| e.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            if name_str == ".git" || name_str == ".vitna" || name_str == "target" || name_str == "node_modules" {
                continue;
            }

            if path.is_dir() {
                Self::scan_directory(base_agent, base_primary, &path, changes, all_diffs)?;
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(base_agent)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");

                let primary_file = base_primary.join(&rel);
                let agent_bytes = fs::read(&path).map_err(|e| e.to_string())?;
                let post_hash = hex::encode(Sha256::digest(&agent_bytes));

                let (pre_hash, pre_content) = if primary_file.exists() {
                    let prim_bytes = fs::read(&primary_file).map_err(|e| e.to_string())?;
                    let h = hex::encode(Sha256::digest(&prim_bytes));
                    (h, String::from_utf8_lossy(&prim_bytes).to_string())
                } else {
                    (NULL_HASH.to_string(), String::new())
                };

                // If content differs, compute diff and record change
                if pre_hash != post_hash {
                    let agent_content = String::from_utf8_lossy(&agent_bytes).to_string();
                    let diff = Self::generate_diff(&rel, &pre_content, &agent_content);
                    all_diffs.push_str(&diff);
                    all_diffs.push('\n');

                    changes.push(FileChange {
                        path: rel,
                        preimage_hash: pre_hash,
                        postimage_hash: post_hash,
                        diff,
                    });
                }
            }
        }

        Ok(())
    }

    fn generate_diff(rel_path: &str, old_content: &str, new_content: &str) -> String {
        let mut diff = format!("--- a/{}\n+++ b/{}\n", rel_path, rel_path);
        let old_lines: Vec<&str> = if old_content.is_empty() { Vec::new() } else { old_content.lines().collect() };
        let new_lines: Vec<&str> = if new_content.is_empty() { Vec::new() } else { new_content.lines().collect() };

        diff.push_str(&format!("@@ -1,{} +1,{} @@\n", old_lines.len(), new_lines.len()));
        for l in old_lines {
            diff.push_str(&format!("-{}\n", l));
        }
        for l in new_lines {
            diff.push_str(&format!("+{}\n", l));
        }
        diff
    }

    /// Validates that the primary checkout has not changed out from under the agent
    /// by verifying that the current hash of each target file matches its expected preimage.
    pub fn validate_preimages<P: AsRef<Path>>(
        primary_repo: P,
        changeset: &ChangeSet,
    ) -> Result<Vec<MergeConflict>, String> {
        let primary = primary_repo.as_ref();
        let mut conflicts = Vec::new();

        for change in &changeset.files {
            let target_file = primary.join(&change.path);
            let current_hash = if target_file.exists() {
                let bytes = fs::read(&target_file).map_err(|e| e.to_string())?;
                hex::encode(Sha256::digest(&bytes))
            } else {
                NULL_HASH.to_string()
            };

            if current_hash != change.preimage_hash {
                conflicts.push(MergeConflict {
                    path: change.path.clone(),
                    expected_preimage: change.preimage_hash.clone(),
                    actual_current_hash: current_hash,
                    message: format!(
                        "Preimage mismatch for '{}': file was modified externally in primary checkout",
                        change.path
                    ),
                });
            }
        }

        Ok(conflicts)
    }

    /// Applies approved changeset into the primary checkout, enforcing preimage validation.
    pub fn apply_changeset<P: AsRef<Path>>(
        primary_repo: P,
        agent_ws: &AgentWorkspace,
        changeset: &ChangeSet,
    ) -> Result<MergeResult, String> {
        let primary = primary_repo.as_ref();

        // 1. Validate preimages before any mutation
        let conflicts = Self::validate_preimages(primary, changeset)?;
        if !conflicts.is_empty() {
            return Ok(MergeResult {
                success: false,
                applied_files: Vec::new(),
                conflicts,
                new_commit_sha: None,
            });
        }

        // 2. Apply modifications
        let mut applied_files = Vec::new();
        for change in &changeset.files {
            let agent_src = agent_ws.agent_workspace_root.join(&change.path);
            let primary_dst = primary.join(&change.path);

            if let Some(parent) = primary_dst.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }

            fs::copy(&agent_src, &primary_dst).map_err(|e| e.to_string())?;
            applied_files.push(change.path.clone());
        }

        Ok(MergeResult {
            success: true,
            applied_files,
            conflicts: Vec::new(),
            new_commit_sha: Some(agent_ws.base_commit_sha.clone()),
        })
    }
}
