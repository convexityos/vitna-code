use crate::tree::{sha256_hex, Entry, Tree};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use vitna_git_workspaces::AgentWorkspace;

pub const NULL_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

const PRIMARY: &str = "the primary checkout";
const AGENT: &str = "the agent workspace";

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

/// A change that cannot be merged because of what the primary checkout holds
/// at its path. Conflicts are found before anything is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeConflict {
    pub path: String,
    pub expected_preimage: String,
    /// SHA-256 of the regular file the primary checkout holds at `path`, or
    /// `NULL_HASH` when it holds nothing there. Empty when there is nothing
    /// the merge will hash: a link at the path or at a directory above it,
    /// or a directory, FIFO, socket or device where the file belongs.
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
    ///
    /// The primary checkout is read without following links. A changed file
    /// whose path the primary checkout holds as a link (at the file or at any
    /// directory above it, whether the link leads outside the checkout or
    /// stays inside it), or as a directory, FIFO, socket or device, has no
    /// preimage a changeset can state, so this returns an error naming it,
    /// the same way an unreadable preimage already did.
    pub fn inspect_changes(agent_ws: &AgentWorkspace) -> Result<ChangeSet, String> {
        let primary = Tree::new(&agent_ws.primary_repo_root, PRIMARY)?;
        let mut changes = Vec::new();
        let mut all_diffs = String::new();

        Self::scan_directory(
            &agent_ws.agent_workspace_root,
            &primary,
            &agent_ws.agent_workspace_root,
            &mut changes,
            &mut all_diffs,
        )?;

        let diff_digest = sha256_hex(all_diffs.as_bytes());
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
        primary: &Tree,
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

            // The entry's own type: a link is never followed. Materialization
            // keeps links that stay inside the checkout, `loop -> .` among
            // them, and a link the agent made could point anywhere; following
            // either would loop this scan or put files from outside the
            // workspace into the changeset. Links are left out of it.
            let file_type = entry.file_type().map_err(|e| e.to_string())?;
            if file_type.is_symlink() {
                tracing::warn!(
                    "Not scanning link '{}' in the agent workspace; changesets carry regular files only",
                    path.display()
                );
                continue;
            }

            if file_type.is_dir() {
                Self::scan_directory(base_agent, primary, &path, changes, all_diffs)?;
            } else if file_type.is_file() {
                let rel = path
                    .strip_prefix(base_agent)
                    .map_err(|e| e.to_string())?
                    .to_string_lossy()
                    .replace('\\', "/");

                let agent_bytes = fs::read(&path).map_err(|e| e.to_string())?;
                let post_hash = sha256_hex(&agent_bytes);

                let (pre_hash, pre_content) = match primary.entry(&rel)? {
                    Entry::File { bytes, .. } => {
                        (sha256_hex(&bytes), String::from_utf8_lossy(&bytes).into_owned())
                    }
                    // Nothing there, even when a file stands where one of its
                    // directories belongs; the merge refuses that as a conflict.
                    Entry::Absent | Entry::Blocked(_) => (NULL_HASH.to_string(), String::new()),
                    Entry::Refused(reason) => {
                        return Err(format!("Cannot compute the preimage of '{}': {}", rel, reason))
                    }
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
    ///
    /// The checkout is read without following links, and a path the merge
    /// could not write without following one, or without replacing something
    /// that is not a regular file, is a conflict too: a link at the path or at
    /// any directory above it (leading outside the checkout or staying
    /// inside), a directory, FIFO, socket or device at the path, or something
    /// other than a directory where one of its directories belongs. A
    /// changeset path that is absolute or climbs out with `..` is malformed,
    /// and is an error rather than a conflict.
    pub fn validate_preimages<P: AsRef<Path>>(
        primary_repo: P,
        changeset: &ChangeSet,
    ) -> Result<Vec<MergeConflict>, String> {
        let primary = Tree::new(primary_repo.as_ref(), PRIMARY)?;
        let mut conflicts = Vec::new();

        for change in &changeset.files {
            let (current_hash, refusal) = match primary.entry(&change.path)? {
                Entry::File { bytes, .. } => (sha256_hex(&bytes), None),
                Entry::Absent => (NULL_HASH.to_string(), None),
                Entry::Blocked(reason) => (NULL_HASH.to_string(), Some(reason)),
                Entry::Refused(reason) => (String::new(), Some(reason)),
            };

            let message = match refusal {
                Some(reason) => format!("Cannot merge '{}' into {}: {}", change.path, PRIMARY, reason),
                None if current_hash != change.preimage_hash => format!(
                    "Preimage mismatch for '{}': file was modified externally in primary checkout",
                    change.path
                ),
                None => continue,
            };
            conflicts.push(MergeConflict {
                path: change.path.clone(),
                expected_preimage: change.preimage_hash.clone(),
                actual_current_hash: current_hash,
                message,
            });
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
        let applied_files =
            Self::write_changes(primary, &agent_ws.agent_workspace_root, changeset)?;

        Ok(MergeResult {
            success: true,
            applied_files,
            conflicts: Vec::new(),
            new_commit_sha: Some(agent_ws.base_commit_sha.clone()),
        })
    }

    /// Writes each change into the primary checkout, taking its bytes from
    /// the agent workspace; `apply_changeset` and the merge queue both merge
    /// through this. Neither tree is resolved through a link, missing
    /// directories are created one level at a time, and each file is checked
    /// against its preimage again, through the handle about to be written.
    ///
    /// Run it only after `validate_preimages` has found no conflicts. A
    /// refusal here means a tree changed in between, and by then earlier
    /// files may already be written, so it is an error that names them.
    pub(crate) fn write_changes(
        primary_repo: &Path,
        agent_root: &Path,
        changeset: &ChangeSet,
    ) -> Result<Vec<String>, String> {
        let primary = Tree::new(primary_repo, PRIMARY)?;
        let agent = Tree::new(agent_root, AGENT)?;
        let total = changeset.files.len();
        let mut applied = Vec::new();
        for change in &changeset.files {
            let (bytes, permissions) = match agent.entry(&change.path) {
                Ok(Entry::File { bytes, permissions }) => (bytes, permissions),
                Ok(Entry::Absent | Entry::Blocked(_)) => {
                    let error = format!("'{}' is no longer in {}", change.path, AGENT);
                    return Err(stopped(error, &applied, total));
                }
                Ok(Entry::Refused(reason)) => {
                    let error = format!("Cannot merge '{}' from {}: {}", change.path, AGENT, reason);
                    return Err(stopped(error, &applied, total));
                }
                Err(error) => return Err(stopped(error, &applied, total)),
            };
            // The permission bits travel with the bytes, as they did when
            // this was `fs::copy`.
            primary
                .write(&change.path, &change.preimage_hash, &bytes, &permissions)
                .map_err(|error| stopped(error, &applied, total))?;
            applied.push(change.path.clone());
        }
        Ok(applied)
    }
}

/// An error that stopped a merge partway, saying what it had already written.
fn stopped(error: String, applied: &[String], total: usize) -> String {
    if applied.is_empty() {
        return error;
    }
    format!(
        "{}. The merge stopped there, after writing {} of {} files: {}",
        error,
        applied.len(),
        total,
        applied.join(", ")
    )
}
