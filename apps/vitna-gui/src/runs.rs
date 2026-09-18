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
//! One distinction this module refuses to blur. `verify_receipt_json` supplies
//! no public key, so the device signature is NOT CHECKED rather than found
//! wanting, and `signature_verified: false` means exactly that. A window that
//! painted it as a failure would be accusing every honest receipt.

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
}

impl Run {
    /// The signature's standing, as three states rather than two.
    pub fn signature(&self) -> Signature {
        if self.report.signature_verified {
            Signature::Verified
        } else if self.receipt.device_signature.is_empty() {
            Signature::Absent
        } else {
            Signature::Unchecked
        }
    }
}

/// What can be said about the device signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signature {
    /// Checked against a key and it held.
    Verified,
    /// Present, and no key was supplied to check it against. Not a fault.
    Unchecked,
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
