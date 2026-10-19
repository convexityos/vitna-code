//! Where a turn would run: the folder, and the branch git says it is on.
//!
//! The branch comes from `host_git`, the one hardened way this workspace asks
//! git anything, because git executes commands a repository's own
//! configuration names and this folder is untrusted input. When git cannot
//! say, the bar shows the folder alone rather than a guessed branch.

use std::path::Path;

use crate::text::clean;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Branch {
    Reading,
    Named(String),
    Detached,
    /// Not a repository, git missing, or git refused: no branch to show.
    Unknown,
}

pub fn folder_name(root: &Path) -> String {
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());
    clean(&name)
}

/// Blocks on two git processes, so it runs off the drawing thread.
pub fn read_branch(root: &Path) -> Branch {
    let Ok(mut cmd) = vitna_git_workspaces::host_git::command(root) else {
        return Branch::Unknown;
    };
    match cmd.arg("branch").arg("--show-current").output() {
        Ok(out) if out.status.success() => {
            let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if name.is_empty() {
                Branch::Detached
            } else {
                Branch::Named(clean(&name))
            }
        }
        _ => Branch::Unknown,
    }
}
