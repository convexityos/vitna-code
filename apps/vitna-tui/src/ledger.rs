//! The runs this folder has receipts for, read off disk.
//!
//! The engine writes each run's receipt to `.vitna/receipts/<run>.json` in the
//! folder it ran in. This reads that directory and nothing else, and it reads
//! each file through `vitna_receipt_verify`, the code `vitna verify` runs, so a
//! row here and that command cannot disagree about what a file says.
//!
//! The folder is untrusted input, and so is everything in it. A receipt is a
//! claim until its signature is checked, and this terminal has no key to check
//! one with: the daemon holds the key, and the declared handshake does not
//! publish it. So every run here is shown with its signature unchecked, which
//! the receipt view says in words. The files are read defensively too: a link
//! is not followed out of the directory, a FIFO or a device is not opened, and
//! nothing larger than a receipt could plausibly be is read at all (the file
//! access lessons 0.1 and 0.2 in CLAUDE-CODE-RELEASES-LESSONS).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::text::clean;

/// Larger than any receipt a run writes by three orders of magnitude, and
/// small enough that a hostile file cannot make the terminal read gigabytes.
const MAX_BYTES: u64 = 1 << 20;

const ZERO_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    New,
    Modified,
    Deleted,
    /// A preimage or postimage hash is missing, which the verifier also
    /// reports as a failed check.
    Unhashed,
}

impl ChangeKind {
    pub fn word(&self) -> &'static str {
        match self {
            ChangeKind::New => "new",
            ChangeKind::Modified => "modified",
            ChangeKind::Deleted => "deleted",
            ChangeKind::Unhashed => "hash missing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Change {
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone)]
pub struct Evidence {
    pub grade: String,
    pub description: String,
}

/// One receipt, with every string made safe to draw.
#[derive(Debug, Clone)]
pub struct Run {
    pub file: String,
    pub written: Option<SystemTime>,
    pub run_id: String,
    pub base_commit: String,
    pub provider: String,
    pub model_sku: String,
    pub isolation: String,
    pub completion: String,
    pub evidence: Vec<Evidence>,
    pub changes: Vec<Change>,
    pub children: usize,
    pub signed: bool,
    /// What the verifier found wrong without a key. Empty means every check it
    /// can make passed, which is not the same as the signature holding.
    pub failed_checks: Vec<String>,
}

/// A file in the receipts directory that is not a receipt this terminal could
/// read. It gets a row of its own, where its time puts it, rather than
/// vanishing: a receipt that silently drops out of a list is how a failed run
/// stops existing.
#[derive(Debug, Clone)]
pub struct Unreadable {
    pub file: String,
    pub written: Option<SystemTime>,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum Entry {
    Run(Box<Run>),
    Unreadable(Unreadable),
}

impl Entry {
    pub fn written(&self) -> Option<SystemTime> {
        match self {
            Entry::Run(r) => r.written,
            Entry::Unreadable(u) => u.written,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Ledger {
    /// Newest first, by when each file was last written. A receipt records no
    /// time of its own, so the file's is the only one there is.
    pub entries: Vec<Entry>,
    /// Why the directory itself could not be listed. A directory that does not
    /// exist is not trouble: it means no run has finished here yet.
    pub trouble: Option<String>,
}

pub fn receipts_dir(root: &Path) -> PathBuf {
    root.join(".vitna").join("receipts")
}

pub fn read(root: &Path) -> Ledger {
    let dir = receipts_dir(root);
    let listing = match fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ledger::default(),
        Err(e) => {
            return Ledger {
                entries: Vec::new(),
                trouble: Some(clean(&format!(".vitna/receipts could not be listed: {e}"))),
            }
        }
    };

    let mut entries = Vec::new();
    let mut trouble = None;
    for item in listing {
        let item = match item {
            Ok(item) => item,
            Err(e) => {
                trouble = Some(clean(&format!(
                    "part of .vitna/receipts could not be listed: {e}"
                )));
                continue;
            }
        };
        let name = item.file_name().to_string_lossy().to_string();
        if !name.to_ascii_lowercase().ends_with(".json") {
            continue;
        }
        entries.push(read_one(&item.path(), &name));
    }

    entries.sort_by_key(|e| std::cmp::Reverse(e.written()));
    Ledger { entries, trouble }
}

fn read_one(path: &Path, name: &str) -> Entry {
    let file = clean(name);
    let unreadable = |written, reason: String| {
        Entry::Unreadable(Unreadable {
            file: file.clone(),
            written,
            reason: clean(&reason),
        })
    };

    // symlink_metadata describes the entry itself, so a link is seen as a
    // link rather than as whatever it points at.
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) => return unreadable(None, format!("could not be examined: {e}")),
    };
    let written = meta.modified().ok();
    if meta.file_type().is_symlink() {
        return unreadable(
            written,
            "is a link, and links out of this folder are not followed".into(),
        );
    }
    if !meta.is_file() {
        return unreadable(written, "is not a regular file".into());
    }
    if meta.len() > MAX_BYTES {
        return unreadable(
            written,
            format!(
                "is {} bytes, larger than any receipt, so it was not read",
                meta.len()
            ),
        );
    }

    let mut content = String::new();
    let read =
        fs::File::open(path).and_then(|f| f.take(MAX_BYTES + 1).read_to_string(&mut content));
    if let Err(e) = read {
        return unreadable(written, format!("could not be read: {e}"));
    }
    if content.len() as u64 > MAX_BYTES {
        return unreadable(written, "grew past the size limit while it was read".into());
    }

    let verified = match vitna_receipt_verify::verify_receipt_json(&content) {
        Ok(v) => v,
        Err(e) => return unreadable(written, format!("is not a vitna-run-receipt-v1: {e}")),
    };
    let r = verified.receipt;
    Entry::Run(Box::new(Run {
        file: format!(".vitna/receipts/{file}"),
        written,
        run_id: clean(&r.run_id),
        base_commit: clean(&r.base_commit_sha),
        provider: clean(&r.model_selection.provider),
        model_sku: clean(&r.model_selection.model_sku),
        isolation: clean(&r.isolation_label),
        completion: clean(&r.completion_state),
        evidence: r
            .evidence_items
            .iter()
            .map(|e| Evidence {
                grade: clean(&e.grade),
                description: clean(&e.description),
            })
            .collect(),
        changes: r
            .changeset
            .files_modified
            .iter()
            .map(|f| Change {
                path: clean(&f.path),
                kind: if f.preimage_hash.is_empty() || f.postimage_hash.is_empty() {
                    ChangeKind::Unhashed
                } else if f.preimage_hash == ZERO_HASH {
                    ChangeKind::New
                } else if f.postimage_hash == ZERO_HASH {
                    ChangeKind::Deleted
                } else {
                    ChangeKind::Modified
                },
            })
            .collect(),
        children: r.child_receipt_roots.len(),
        signed: !r.device_signature.trim().is_empty(),
        failed_checks: verified.report.errors.iter().map(|e| clean(e)).collect(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vitna_tui_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(receipts_dir(&dir)).expect("create receipts dir");
        dir
    }

    fn receipt(state: &str, path: &str) -> String {
        serde_json::json!({
            "schema_version": "vitna-run-receipt-v1",
            "run_id": "run-1",
            "session_id": "sess-1",
            "workspace_fingerprint": "f",
            "base_commit_sha": "38d891d1cd3807f04a4ddd9499986afb9e4259ce",
            "model_selection": { "provider": "anthropic", "model_sku": "claude-fable-5-1", "routing_reason": "pinned_profile" },
            "event_hash_chain_root": "r",
            "isolation_label": "guarded",
            "completion_state": state,
            "evidence_items": [],
            "changeset": { "files_modified": [
                { "path": path, "preimage_hash": ZERO_HASH, "postimage_hash": "ab" }
            ], "diff_digest": "d" },
            "runner_execution_statements": [],
            "device_signature": "00"
        })
        .to_string()
    }

    #[test]
    fn no_receipts_directory_is_an_empty_list_not_trouble() {
        let dir = std::env::temp_dir().join(format!("vitna_tui_none_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ledger = read(&dir);
        assert!(ledger.entries.is_empty());
        assert!(ledger.trouble.is_none());
    }

    #[test]
    fn a_receipt_reads_through_the_verifier() {
        let dir = scratch("reads");
        fs::write(
            receipts_dir(&dir).join("run-1.json"),
            receipt("completed_with_evidence", "src/lib.rs"),
        )
        .unwrap();
        let ledger = read(&dir);
        let Some(Entry::Run(run)) = ledger.entries.first() else {
            panic!("no run read: {ledger:?}")
        };
        assert!(run.failed_checks.is_empty(), "{:?}", run.failed_checks);
        assert_eq!(run.changes[0].kind, ChangeKind::New);
        assert!(run.signed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_verifier_refuses_shows_as_a_failed_check() {
        let dir = scratch("refused");
        fs::write(
            receipts_dir(&dir).join("run-1.json"),
            receipt("finished_somehow", "a"),
        )
        .unwrap();
        let ledger = read(&dir);
        let Some(Entry::Run(run)) = ledger.entries.first() else {
            panic!("no run read")
        };
        assert_eq!(run.failed_checks.len(), 1, "{:?}", run.failed_checks);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_a_receipt_keeps_a_row() {
        let dir = scratch("junk");
        fs::write(receipts_dir(&dir).join("notes.json"), "{ not json").unwrap();
        fs::write(receipts_dir(&dir).join("readme.txt"), "ignored: not .json").unwrap();
        let ledger = read(&dir);
        assert_eq!(ledger.entries.len(), 1, "{ledger:?}");
        assert!(matches!(ledger.entries[0], Entry::Unreadable(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hostile_path_in_a_receipt_cannot_reach_the_terminal() {
        let dir = scratch("hostile");
        fs::write(
            receipts_dir(&dir).join("run-1.json"),
            receipt("completed_with_evidence", "src/\u{1b}]0;pwned\u{7}lib.rs"),
        )
        .unwrap();
        let ledger = read(&dir);
        let Some(Entry::Run(run)) = ledger.entries.first() else {
            panic!("no run read")
        };
        assert!(
            !run.changes[0].path.chars().any(|c| c.is_control()),
            "{:?}",
            run.changes[0].path
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_named_like_a_receipt_is_not_opened() {
        let dir = scratch("dir");
        fs::create_dir_all(receipts_dir(&dir).join("run-9.json")).unwrap();
        let ledger = read(&dir);
        assert!(
            matches!(&ledger.entries[0], Entry::Unreadable(u) if u.reason.contains("not a regular file"))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_larger_than_any_receipt_is_not_read() {
        let dir = scratch("big");
        let big = receipts_dir(&dir).join("run-big.json");
        fs::write(&big, vec![b' '; (MAX_BYTES + 1) as usize]).unwrap();
        let ledger = read(&dir);
        assert!(
            matches!(&ledger.entries[0], Entry::Unreadable(u) if u.reason.contains("larger than any receipt")),
            "{ledger:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A receipt that is a link to a file elsewhere, even a genuine receipt,
    /// is refused rather than followed out of the folder.
    #[test]
    fn a_link_in_the_receipts_folder_is_not_followed() {
        let dir = scratch("link");
        let outside = dir.join("outside.json");
        fs::write(&outside, receipt("completed_with_evidence", "a")).unwrap();
        let link = receipts_dir(&dir).join("run-link.json");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, &link).is_ok();
        // A file link needs a privilege most Windows accounts lack. A
        // junction needs none, and std reports it as a link too, so the same
        // refusal is exercised either way.
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(&outside, &link).is_ok()
            || std::process::Command::new("cmd")
                .arg("/C")
                .arg("mklink")
                .arg("/J")
                .arg(&link)
                .arg(&dir)
                .output()
                .is_ok_and(|o| o.status.success());
        if !made {
            eprintln!("skipped: this machine cannot create symbolic links");
            let _ = fs::remove_dir_all(&dir);
            return;
        }
        let ledger = read(&dir);
        assert!(
            matches!(&ledger.entries[0], Entry::Unreadable(u) if u.reason.contains("is a link")),
            "{ledger:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
