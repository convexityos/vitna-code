//! The runs this workspace has on disk, and the verdict on each.
//!
//! Receipts are files: the engine writes one per run to
//! `<workspace>/.vitna/receipts/<run_id>.json`. So this reads them straight off
//! disk and checks them here, in the window, rather than asking the daemon
//! whether its own work was sound. That is the difference between a receipt and
//! a status message, and it is the product's whole claim: the history survives
//! the daemon being stopped, and the check does not depend on the thing being
//! checked.
//!
//! Two checks, kept apart on purpose. The structural one (`verify_receipt_json`)
//! takes no key, so a receipt signed by some other key is not called invalid.
//! The signature is checked on its own, against the key the connected daemon
//! publishes, and it has four answers rather than two: verified, not checked
//! (no key to check it with, which is not a fault), does not match (another
//! key signed it, or it changed since; a receipt names no key, so it cannot
//! say which), and absent. A window that folded "not checked" into a failure
//! would be accusing every honest receipt.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::SystemTime;

use vitna_receipt_verify::{verify_receipt_json, VerificationReport};
use vitna_receipts::VitnaRunReceiptV1;

/// One receipt on disk.
pub struct Run {
    pub path: PathBuf,
    pub receipt: Box<VitnaRunReceiptV1>,
    pub report: VerificationReport,
    /// When the file was written, for ordering and for the age line. The
    /// receipt carries no timestamp of its own.
    pub written: Option<SystemTime>,
    /// The last signature check and the key it was made with, since a check
    /// costs a canonical encoding and a curve operation and the run view asks
    /// every frame.
    pub checked: RefCell<Option<(String, Signature)>>,
}

impl Run {
    /// The signature's standing against `key`, the public key (hex) the
    /// connected daemon signs with, or `None` when there is no such key.
    pub fn signature(&self, key: Option<&str>) -> Signature {
        if self.receipt.device_signature.is_empty() {
            return Signature::Absent;
        }
        let Some(key) = key else {
            return Signature::Unchecked;
        };
        if let Some((with, answer)) = &*self.checked.borrow() {
            if with == key {
                return *answer;
            }
        }
        let answer = check_signature(&self.receipt, key);
        *self.checked.borrow_mut() = Some((key.to_string(), answer));
        answer
    }
}

/// Checks `receipt`'s device signature against `key_hex`. A key that is not
/// one is no key to check with, which is `Unchecked` rather than a verdict on
/// the receipt.
pub fn check_signature(receipt: &VitnaRunReceiptV1, key_hex: &str) -> Signature {
    let key = hex::decode(key_hex)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .and_then(|bytes| ed25519_dalek::VerifyingKey::from_bytes(&bytes).ok());
    match key {
        None => Signature::Unchecked,
        Some(key) => match receipt.verify_signature(&key) {
            Ok(true) => Signature::Verified,
            // A signature that fails, or that is not even a signature's shape.
            _ => Signature::Mismatch,
        },
    }
}

/// What can be said about the device signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signature {
    /// Checked against the connected daemon's key, and it held.
    Verified,
    /// Present, with no key to check it against. Not a fault.
    Unchecked,
    /// Present, and it does not verify with the connected daemon's key:
    /// another key signed it, or the receipt changed after it was signed.
    Mismatch,
    /// The receipt carries no signature at all, which IS a fault.
    Absent,
}

/// A file in the receipts directory that could not be read as one. Shown
/// rather than skipped: a receipt that will not parse is the most interesting
/// thing in the directory.
pub struct Unreadable {
    pub path: PathBuf,
    pub reason: String,
    /// The file's own mtime, so the list can place it among the runs.
    pub written: Option<SystemTime>,
}

#[derive(Default)]
pub struct Ledger {
    /// Newest first.
    pub runs: Vec<Run>,
    pub unreadable: Vec<Unreadable>,
    /// Why the directory itself could not be listed, if it could not be.
    pub trouble: Option<String>,
}

pub enum Loading {
    Reading,
    Done(Box<Ledger>),
}

pub struct Probe {
    rx: Receiver<Ledger>,
    state: Loading,
}

impl Probe {
    pub fn start(workspace: &Path) -> Self {
        let (tx, rx) = mpsc::channel();
        let dir = workspace.join(".vitna").join("receipts");
        std::thread::spawn(move || {
            let _ = tx.send(read(&dir));
        });
        Self {
            rx,
            state: Loading::Reading,
        }
    }

    pub fn poll(&mut self) -> &Loading {
        if matches!(self.state, Loading::Reading) {
            if let Ok(ledger) = self.rx.try_recv() {
                self.state = Loading::Done(Box::new(ledger));
            }
        }
        &self.state
    }
}

fn read(dir: &Path) -> Ledger {
    let mut ledger = Ledger::default();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No runs yet is not a fault, and must not print as one.
            return ledger;
        }
        Err(e) => {
            ledger.trouble = Some(format!("{}: {e}", dir.display()));
            return ledger;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }

        let written = entry.metadata().ok().and_then(|m| m.modified().ok());

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                ledger.unreadable.push(Unreadable {
                    path,
                    reason: e.to_string(),
                    written,
                });
                continue;
            }
        };

        match verify_receipt_json(&content) {
            Ok(v) => ledger.runs.push(Run {
                path,
                receipt: Box::new(v.receipt),
                report: v.report,
                written,
                checked: RefCell::new(None),
            }),
            Err(e) => ledger.unreadable.push(Unreadable {
                path,
                reason: e.to_string(),
                written,
            }),
        }
    }

    // Newest first. A receipt carries no timestamp, so the file's mtime is
    // what there is; one without a readable mtime sorts last rather than
    // being given a made up one.
    ledger.runs.sort_by(|a, b| newest_first(a.written, b.written));
    ledger.unreadable.sort_by(|a, b| newest_first(a.written, b.written));

    ledger
}

/// Newest first, and anything without a time after everything with one.
pub fn newest_first(a: Option<SystemTime>, b: Option<SystemTime>) -> std::cmp::Ordering {
    match (b, a) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// "now", "7m", "3h", "2d": the age a dense list can carry, the way Cursor's
/// agent list writes it. The long form is `age`, for a hover.
pub fn short_age(written: Option<SystemTime>) -> Option<String> {
    let secs = SystemTime::now().duration_since(written?).ok()?.as_secs();
    Some(match secs {
        0..=44 => "now".to_string(),
        45..=5399 => format!("{}m", ((secs as f64) / 60.0).round().max(1.0) as u64),
        5400..=129_599 => format!("{}h", ((secs as f64) / 3600.0).round() as u64),
        _ => format!("{}d", ((secs as f64) / 86_400.0).round() as u64),
    })
}

/// "4 minutes ago", or None when there is no timestamp to say it from.
pub fn age(written: Option<SystemTime>) -> Option<String> {
    let secs = SystemTime::now().duration_since(written?).ok()?.as_secs();
    Some(match secs {
        0..=45 => "just now".to_string(),
        46..=5400 => {
            let m = (secs as f64 / 60.0).round() as u64;
            format!("{m} minute{} ago", if m == 1 { "" } else { "s" })
        }
        5401..=172_800 => {
            let h = (secs as f64 / 3600.0).round() as u64;
            format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
        }
        _ => {
            let d = (secs as f64 / 86_400.0).round() as u64;
            format!("{d} day{} ago", if d == 1 { "" } else { "s" })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use vitna_receipts::{ChangeSetRecord, ModelSelectionRecord};

    fn signed_by(key: &SigningKey) -> VitnaRunReceiptV1 {
        let mut receipt = VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-1".to_string(),
            session_id: "sess-1".to_string(),
            workspace_fingerprint: "ws".to_string(),
            base_commit_sha: "0".repeat(40),
            model_selection: ModelSelectionRecord {
                provider: "anthropic".to_string(),
                model_sku: "m".to_string(),
                routing_reason: "pinned_profile".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: "root".to_string(),
            isolation_label: "guarded".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: vec![],
            changeset: ChangeSetRecord { files_modified: vec![], diff_digest: "d".to_string() },
            runner_execution_statements: vec![],
            child_receipt_roots: vec![],
            device_signature: String::new(),
        };
        receipt.sign(key).expect("signs");
        receipt
    }

    fn run_of(receipt: VitnaRunReceiptV1) -> Run {
        let report = vitna_receipt_verify::ReceiptVerifier::verify_receipt(&receipt, None).expect("report");
        Run {
            path: PathBuf::from("run-1.json"),
            receipt: Box::new(receipt),
            report,
            written: None,
            checked: RefCell::new(None),
        }
    }

    fn hex_of(key: &SigningKey) -> String {
        hex::encode(key.verifying_key().to_bytes())
    }

    #[test]
    fn a_signature_is_checked_against_the_daemons_key() {
        let daemon = vitna_receipts::generate_signing_key();
        let other = vitna_receipts::generate_signing_key();
        let run = run_of(signed_by(&daemon));

        assert_eq!(run.signature(Some(&hex_of(&daemon))), Signature::Verified);
        // Asked again with another key, the answer cached for the first is
        // not reused.
        assert_eq!(run.signature(Some(&hex_of(&other))), Signature::Mismatch);
        assert_eq!(run.signature(None), Signature::Unchecked);
        assert_eq!(run.signature(Some("not a key")), Signature::Unchecked);
        // Signed by another key is not a structural fault.
        assert!(run.report.is_valid);

        let mut changed = signed_by(&daemon);
        changed.completion_state = "failed".to_string();
        assert_eq!(run_of(changed).signature(Some(&hex_of(&daemon))), Signature::Mismatch);

        let mut unsigned = signed_by(&daemon);
        unsigned.device_signature.clear();
        assert_eq!(run_of(unsigned).signature(Some(&hex_of(&daemon))), Signature::Absent);
    }
}
