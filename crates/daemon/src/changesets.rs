//! Staged isolated changesets and verified `ApplyChangeSet` merge dispatch.
//!
//! Child runs write into disposable `AgentWorkspace` directories (Invariant 8).
//! When a child run finishes with modifications, `GitBroker::inspect_changes`
//! hashes every preimage and postimage and stages the resulting `ChangeSet`
//! here. The operator explicitly promotes it into the primary checkout via
//! `ApplyChangeSet`, which re-verifies target checkout identity, optional base
//! commit, and link-safe preimage/postimage integrity before writing.

use std::path::Path;

use vitna_git_broker::{ChangeSet, GitBroker, MergeConflict, MergeResult};
use vitna_git_workspaces::AgentWorkspace;
use vitna_protocol::messages::ApplyChangeSet;

use crate::DaemonServer;

/// A staged changeset awaiting explicit operator promotion into the primary checkout.
#[derive(Debug, Clone)]
pub struct StagedChangeSet {
    pub session_id: String,
    pub run_id: String,
    pub changeset: ChangeSet,
    pub workspace: AgentWorkspace,
}

/// Errors returned when applying a staged changeset over daemon IPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyChangeSetError {
    InvalidRequest(String),
    NotFound(String),
    CheckoutMismatch(String),
    BaseCommitMismatch(String),
    Conflict(Vec<MergeConflict>),
    ApplyFailed(String),
}

impl std::fmt::Display for ApplyChangeSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest(m)
            | Self::NotFound(m)
            | Self::CheckoutMismatch(m)
            | Self::BaseCommitMismatch(m)
            | Self::ApplyFailed(m) => f.write_str(m),
            Self::Conflict(conflicts) => {
                let summary: Vec<String> = conflicts
                    .iter()
                    .map(|c| format!("{}: {}", c.path, c.message))
                    .collect();
                write!(
                    f,
                    "preimage conflict prevented ApplyChangeSet: {}",
                    summary.join("; ")
                )
            }
        }
    }
}

/// Result of a verified `DaemonServer::apply_changeset` operation.
#[derive(Debug, Clone)]
pub struct AppliedChangeSet {
    pub session_id: String,
    pub run_id: String,
    pub change_set_id: String,
    pub diff_digest: String,
    pub merge: MergeResult,
}

impl DaemonServer {
    /// Stages an inspected changeset and its isolated workspace for operator promotion.
    pub fn stage_changeset(
        &self,
        session_id: impl Into<String>,
        run_id: impl Into<String>,
        changeset: ChangeSet,
        workspace: AgentWorkspace,
    ) -> Result<String, String> {
        let id = changeset.changeset_id.clone();
        let mut map = self.pending_changesets.lock().map_err(|e| e.to_string())?;
        map.insert(
            id.clone(),
            StagedChangeSet {
                session_id: session_id.into(),
                run_id: run_id.into(),
                changeset,
                workspace,
            },
        );
        Ok(id)
    }

    /// Applies a staged changeset to its primary checkout after verifying target
    /// checkout identity, optional expected base commit, and preimage hashes.
    pub fn apply_changeset(
        &self,
        req: &ApplyChangeSet,
    ) -> Result<AppliedChangeSet, ApplyChangeSetError> {
        let change_set_id = req.change_set_id.trim();
        if change_set_id.is_empty() {
            return Err(ApplyChangeSetError::InvalidRequest(
                "ApplyChangeSet needs a change_set_id".to_string(),
            ));
        }
        let target_path = req.target_checkout_path.trim();
        if target_path.is_empty() {
            return Err(ApplyChangeSetError::InvalidRequest(
                "ApplyChangeSet needs a target_checkout_path".to_string(),
            ));
        }

        let staged = {
            let map = self
                .pending_changesets
                .lock()
                .map_err(|e| ApplyChangeSetError::ApplyFailed(e.to_string()))?;
            map.get(change_set_id).cloned().ok_or_else(|| {
                ApplyChangeSetError::NotFound(format!(
                    "no staged changeset with id {change_set_id}"
                ))
            })?
        };

        if let Some(session) = self.get_session(&staged.session_id) {
            if session.mode == "inspect" {
                return Err(ApplyChangeSetError::InvalidRequest(
                    "session is in read-only 'inspect' mode: ApplyChangeSet is refused by policy"
                        .to_string(),
                ));
            }
        }

        let target_canon = std::fs::canonicalize(Path::new(target_path)).map_err(|e| {
            ApplyChangeSetError::CheckoutMismatch(format!(
                "cannot resolve target_checkout_path {target_path}: {e}"
            ))
        })?;
        let primary_canon = std::fs::canonicalize(&staged.workspace.primary_repo_root)
            .map_err(|e| {
                ApplyChangeSetError::ApplyFailed(format!(
                    "cannot resolve staged primary checkout {}: {e}",
                    staged.workspace.primary_repo_root.display()
                ))
            })?;
        if target_canon != primary_canon {
            return Err(ApplyChangeSetError::CheckoutMismatch(format!(
                "target_checkout_path {} does not match staged primary checkout {}",
                target_canon.display(),
                primary_canon.display()
            )));
        }

        let expected_base = req.expected_base_commit.trim();
        if !expected_base.is_empty() && expected_base != staged.changeset.base_commit_sha {
            return Err(ApplyChangeSetError::BaseCommitMismatch(format!(
                "expected_base_commit {expected_base} does not match staged base commit {}",
                staged.changeset.base_commit_sha
            )));
        }

        let merge = GitBroker::apply_changeset(&primary_canon, &staged.workspace, &staged.changeset)
            .map_err(ApplyChangeSetError::ApplyFailed)?;

        if !merge.success {
            return Err(ApplyChangeSetError::Conflict(merge.conflicts));
        }

        if let Ok(mut map) = self.pending_changesets.lock() {
            map.remove(change_set_id);
        }

        Ok(AppliedChangeSet {
            session_id: staged.session_id,
            run_id: staged.run_id,
            change_set_id: change_set_id.to_string(),
            diff_digest: staged.changeset.diff_digest,
            merge,
        })
    }
}
