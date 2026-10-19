//! What a session did to the folder, and the signed receipt made from it
//! (ADR-0006, 8).
//!
//! The server records every effect it caused: each write and removal with the
//! hashes it read from disk, and each command with the statement the runner
//! produced or the reason it was not run. A receipt is built from those
//! records alone. What only the page knows, which model answered, goes in as
//! the page reported it and says so.

use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::Path;
use vitna_receipts::{
    ChangeSetRecord, EvidenceItemRecord, FileModificationRecord, ModelSelectionRecord,
    RunnerExecutionStatementRecord, VitnaRunReceiptV1,
};
use vitna_tools::workspace_files::ABSENT_PREIMAGE_HASH;

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const NO_COMMIT: &str = "0000000000000000000000000000000000000000";

/// A command that ran, as the runner reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Ran {
    pub action_id: String,
    pub exit_code: i32,
    pub statement_digest: String,
    pub sandbox_backend: String,
    pub sandbox_enforcement: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub(crate) enum Effect {
    Write {
        path: String,
        preimage_hash: String,
        postimage_hash: String,
    },
    Remove {
        path: String,
        preimage_hash: String,
    },
    Command {
        command: String,
        directory: String,
        /// Set when the runner ran it.
        #[serde(skip_serializing_if = "Option::is_none")]
        ran: Option<Ran>,
        /// Set when it did not run, or did not finish: refused for want of a
        /// sandbox, failed to start, or stopped at its time limit.
        #[serde(skip_serializing_if = "Option::is_none")]
        not_run: Option<String>,
    },
}

#[derive(Default)]
pub(crate) struct Journal {
    effects: Vec<Effect>,
}

impl Journal {
    pub(crate) fn record(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    /// Everything recorded since the last receipt, leaving the journal empty.
    pub(crate) fn take(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Puts effects back at the front, after a receipt that could not be
    /// written, so the next one still covers them.
    pub(crate) fn restore(&mut self, mut effects: Vec<Effect>) {
        effects.append(&mut self.effects);
        self.effects = effects;
    }
}

/// What the receipt says that the journal cannot: the run's identity, the
/// model as the page reported it, and how the page says the turn ended.
pub(crate) struct Details<'a> {
    pub run_id: String,
    pub session_id: &'a str,
    pub root: &'a Path,
    pub base_commit: Option<String>,
    pub provider: String,
    pub model: String,
    pub completion: String,
    pub summary: Option<String>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Builds and signs a receipt from a session's effects.
pub(crate) fn receipt(
    effects: &[Effect],
    details: Details<'_>,
    key: &SigningKey,
) -> Result<VitnaRunReceiptV1, String> {
    // Each effect is hashed with the hash before it, so the root commits to
    // the order as well as the set.
    let mut previous = GENESIS.to_string();
    let mut event_hashes = Vec::with_capacity(effects.len());
    for effect in effects {
        let body = serde_json::to_string(effect).map_err(|e| e.to_string())?;
        let hash = sha256_hex(format!("{previous}\n{body}").as_bytes());
        event_hashes.push(hash.clone());
        previous = hash;
    }

    // One record per file, in the order files were first touched: what it
    // held before its first change and after its last.
    let mut files: Vec<FileModificationRecord> = Vec::new();
    for effect in effects {
        let (path, pre, post) = match effect {
            Effect::Write {
                path,
                preimage_hash,
                postimage_hash,
            } => (path, preimage_hash, postimage_hash.as_str()),
            Effect::Remove {
                path,
                preimage_hash,
            } => (path, preimage_hash, ABSENT_PREIMAGE_HASH),
            Effect::Command { .. } => continue,
        };
        match files.iter_mut().find(|f| &f.path == path) {
            Some(record) => record.postimage_hash = post.to_string(),
            None => files.push(FileModificationRecord {
                path: path.clone(),
                preimage_hash: pre.clone(),
                postimage_hash: post.to_string(),
            }),
        }
    }
    // The server never computes a diff, so the digest covers what defines
    // one exactly: each path with the hashes of its two sides.
    let listing: String = files
        .iter()
        .map(|f| format!("{}\t{}\t{}\n", f.path, f.preimage_hash, f.postimage_hash))
        .collect();

    let mut statements = Vec::new();
    let mut evidence = Vec::new();
    let mut ran_unsandboxed = false;
    for effect in effects {
        let Effect::Command {
            command,
            directory,
            ran,
            not_run,
        } = effect
        else {
            continue;
        };
        let place = if directory.is_empty() {
            "at the top of the folder".to_string()
        } else {
            format!("in {directory}")
        };
        if let Some(ran) = ran {
            ran_unsandboxed |= ran.sandbox_backend == vitna_runner::BACKEND_NONE;
            statements.push(RunnerExecutionStatementRecord {
                action_id: ran.action_id.clone(),
                statement_digest: ran.statement_digest.clone(),
                // This device's word for the statement, over its digest.
                signature: hex::encode(key.sign(ran.statement_digest.as_bytes()).to_bytes()),
                sandbox_backend: Some(ran.sandbox_backend.clone()),
                sandbox_enforcement: Some(ran.sandbox_enforcement.clone()),
            });
            let grade = if ran.sandbox_backend == vitna_runner::BACKEND_NONE
                || ran.sandbox_backend == vitna_runner::BACKEND_NO_PROCESS
            {
                "broker_observed"
            } else {
                "sandbox_captured"
            };
            let isolation = match ran.sandbox_backend.as_str() {
                vitna_runner::BACKEND_NONE => "with no sandbox".to_string(),
                vitna_runner::BACKEND_NO_PROCESS => {
                    "in a runner that starts no process".to_string()
                }
                backend => format!("in the {backend} sandbox ({})", ran.sandbox_enforcement),
            };
            evidence.push((
                grade,
                format!("`{command}` {place} exited {}, {isolation}", ran.exit_code),
                Some(ran.statement_digest.clone()),
            ));
        } else if let Some(reason) = not_run {
            evidence.push((
                "broker_observed",
                format!("`{command}` {place} did not complete: {reason}"),
                None,
            ));
        }
    }
    if let Some(summary) = details.summary.as_deref().filter(|s| !s.trim().is_empty()) {
        evidence.push((
            "model_reported",
            summary.to_string(),
            Some(sha256_hex(summary.as_bytes())),
        ));
    }
    let evidence_items = evidence
        .into_iter()
        .enumerate()
        .map(
            |(i, (grade, description, artifact_digest))| EvidenceItemRecord {
                evidence_id: format!("ev-{}", i + 1),
                grade: grade.to_string(),
                description,
                artifact_digest,
            },
        )
        .collect();

    let mut receipt = VitnaRunReceiptV1 {
        schema_version: "vitna-run-receipt-v1".to_string(),
        run_id: details.run_id,
        session_id: details.session_id.to_string(),
        workspace_fingerprint: sha256_hex(details.root.to_string_lossy().as_bytes()),
        base_commit_sha: details.base_commit.unwrap_or_else(|| NO_COMMIT.to_string()),
        model_selection: ModelSelectionRecord {
            provider: details.provider,
            model_sku: details.model,
            routing_reason: "reported_by_page".to_string(),
            policy_digest: None,
        },
        event_hash_chain_root: VitnaRunReceiptV1::compute_event_merkle_root(&event_hashes),
        // Full access the moment one command ran with nothing around it,
        // however the rest of the run went.
        isolation_label: if ran_unsandboxed {
            "full_access"
        } else {
            "guarded"
        }
        .to_string(),
        completion_state: details.completion,
        evidence_items,
        changeset: ChangeSetRecord {
            files_modified: files,
            diff_digest: sha256_hex(listing.as_bytes()),
        },
        runner_execution_statements: statements,
        child_receipt_roots: Vec::new(),
        device_signature: String::new(),
    };
    receipt
        .sign(key)
        .map_err(|e| format!("could not sign the receipt: {e}"))?;
    Ok(receipt)
}

/// The commit the folder's checkout is at, read from its `.git` files by the
/// caller and checked here: a full hex object id, or nothing.
pub(crate) fn commit_id(text: &str) -> Option<String> {
    let id = text.trim();
    (matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| id.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vitna_receipt_verify::ReceiptVerifier;

    fn key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn details(completion: &str) -> Details<'static> {
        Details {
            run_id: "app-1".to_string(),
            session_id: "app-session-1",
            root: Path::new("/work/project"),
            base_commit: None,
            provider: "openrouter".to_string(),
            model: "some/model".to_string(),
            completion: completion.to_string(),
            summary: None,
        }
    }

    fn write(path: &str, pre: &str, post: &str) -> Effect {
        Effect::Write {
            path: path.into(),
            preimage_hash: pre.into(),
            postimage_hash: post.into(),
        }
    }

    fn ran(backend: &str) -> Effect {
        Effect::Command {
            command: "npm test".into(),
            directory: String::new(),
            ran: Some(Ran {
                action_id: "act-1".into(),
                exit_code: 0,
                statement_digest: "d".repeat(64),
                sandbox_backend: backend.into(),
                sandbox_enforcement: if backend == "none" {
                    "none"
                } else {
                    "fully_enforced"
                }
                .into(),
                duration_ms: 12,
            }),
            not_run: None,
        }
    }

    #[test]
    fn a_receipt_verifies_against_the_key_that_signed_it() {
        let effects = vec![write("a.txt", "1", "2"), ran("bubblewrap")];
        let receipt =
            receipt(&effects, details("completed_with_unknowns"), &key()).expect("receipt");
        let public = hex::encode(key().verifying_key().to_bytes());
        let report = ReceiptVerifier::verify_receipt(&receipt, Some(&public)).expect("verify");
        assert!(
            report.is_valid && report.signature_verified,
            "{:?}",
            report.errors
        );

        let other = hex::encode(
            SigningKey::from_bytes(&[8u8; 32])
                .verifying_key()
                .to_bytes(),
        );
        let report = ReceiptVerifier::verify_receipt(&receipt, Some(&other)).expect("verify");
        assert!(
            !report.signature_verified,
            "verified against a key that did not sign it"
        );
    }

    #[test]
    fn a_file_touched_twice_is_one_change_from_its_first_state_to_its_last() {
        let effects = vec![
            write("a.txt", "1", "2"),
            write("b.txt", ABSENT_PREIMAGE_HASH, "5"),
            write("a.txt", "2", "3"),
            Effect::Remove {
                path: "b.txt".into(),
                preimage_hash: "5".into(),
            },
        ];
        let receipt =
            receipt(&effects, details("completed_with_unknowns"), &key()).expect("receipt");
        let files = &receipt.changeset.files_modified;
        assert_eq!(files.len(), 2);
        assert_eq!(
            (
                files[0].path.as_str(),
                files[0].preimage_hash.as_str(),
                files[0].postimage_hash.as_str()
            ),
            ("a.txt", "1", "3")
        );
        assert_eq!(
            (
                files[1].preimage_hash.as_str(),
                files[1].postimage_hash.as_str()
            ),
            (ABSENT_PREIMAGE_HASH, ABSENT_PREIMAGE_HASH),
            "a file made and removed again"
        );
    }

    #[test]
    fn the_isolation_label_is_what_the_commands_actually_had() {
        let sandboxed = receipt(
            &[ran("bubblewrap")],
            details("completed_with_unknowns"),
            &key(),
        )
        .expect("receipt");
        assert_eq!(sandboxed.isolation_label, "guarded");
        assert_eq!(sandboxed.evidence_items[0].grade, "sandbox_captured");
        let bare = receipt(
            &[ran("bubblewrap"), ran("none")],
            details("completed_with_unknowns"),
            &key(),
        )
        .expect("receipt");
        assert_eq!(bare.isolation_label, "full_access");
        assert_eq!(
            bare.evidence_items[1].grade, "broker_observed",
            "nothing confined it"
        );
        assert_eq!(
            bare.evidence_items[1].description,
            "`npm test` at the top of the folder exited 0, with no sandbox"
        );
        assert_eq!(
            sandboxed.evidence_items[0].description,
            "`npm test` at the top of the folder exited 0, in the bubblewrap sandbox (fully_enforced)"
        );
        let none = receipt(
            &[write("a", "1", "2")],
            details("completed_with_unknowns"),
            &key(),
        )
        .expect("receipt");
        assert_eq!(none.isolation_label, "guarded");
    }

    #[test]
    fn each_statement_is_signed_over_its_digest_by_this_device() {
        use ed25519_dalek::{Signature, Verifier};
        let receipt = receipt(
            &[ran("seatbelt")],
            details("completed_with_unknowns"),
            &key(),
        )
        .expect("receipt");
        let statement = &receipt.runner_execution_statements[0];
        assert_eq!(statement.action_id, "act-1");
        let bytes: [u8; 64] = hex::decode(&statement.signature)
            .unwrap()
            .try_into()
            .unwrap();
        key()
            .verifying_key()
            .verify(
                statement.statement_digest.as_bytes(),
                &Signature::from_bytes(&bytes),
            )
            .expect("the statement signature checks");
    }

    #[test]
    fn the_model_is_recorded_as_the_page_reported_it() {
        let mut d = details("cancelled");
        d.summary = Some("Renamed the helper.".into());
        let receipt = receipt(&[], d, &key()).expect("receipt");
        assert_eq!(receipt.model_selection.routing_reason, "reported_by_page");
        assert_eq!(receipt.model_selection.model_sku, "some/model");
        assert_eq!(receipt.completion_state, "cancelled");
        assert_eq!(receipt.evidence_items.len(), 1);
        assert_eq!(receipt.evidence_items[0].grade, "model_reported");
        assert_eq!(receipt.base_commit_sha, NO_COMMIT);
    }

    #[test]
    fn the_chain_root_changes_with_the_order_of_effects() {
        let a = vec![write("a", "1", "2"), write("b", "3", "4")];
        let b = vec![write("b", "3", "4"), write("a", "1", "2")];
        let ra = receipt(&a, details("completed_with_unknowns"), &key()).expect("receipt");
        let rb = receipt(&b, details("completed_with_unknowns"), &key()).expect("receipt");
        assert_ne!(ra.event_hash_chain_root, rb.event_hash_chain_root);
    }

    #[test]
    fn a_commit_id_is_a_full_hex_object_id() {
        assert_eq!(
            commit_id(&format!("{}\n", "A".repeat(40))),
            Some("a".repeat(40))
        );
        assert_eq!(commit_id(&"b".repeat(64)), Some("b".repeat(64)));
        assert_eq!(commit_id("ref: refs/heads/main"), None);
        assert_eq!(commit_id(&"g".repeat(40)), None);
        assert_eq!(commit_id("abc"), None);
    }

    #[test]
    fn a_journal_restored_after_a_failed_receipt_keeps_its_order() {
        let mut journal = Journal::default();
        journal.record(write("a", "1", "2"));
        let taken = journal.take();
        journal.record(write("b", "3", "4"));
        journal.restore(taken);
        let order: Vec<Effect> = journal.take();
        assert_eq!(order, vec![write("a", "1", "2"), write("b", "3", "4")]);
    }
}
