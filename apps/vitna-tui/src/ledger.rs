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

#[derive(Debug, Clone)]
pub struct EgressCall {
    pub method: String,
    pub redacted_url: String,
    pub status_code: u16,
    pub bytes_received: u64,
    pub latency_ms: u64,
    pub response_sha256: String,
}

#[derive(Debug, Clone)]
pub struct RunnerStatement {
    pub action_id: String,
    pub sandbox_backend: Option<String>,
    pub sandbox_enforcement: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MandateProvenance {
    pub root_jti: String,
    pub chain_hash: String,
    pub ledger: String,
    pub currency: String,
    pub total_spent: String,
    pub path: Vec<String>,
    pub spend_count: usize,
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
    pub policy_digest: Option<String>,
    pub isolation: String,
    pub completion: String,
    pub evidence: Vec<Evidence>,
    pub changes: Vec<Change>,
    pub egress_calls: Vec<EgressCall>,
    pub runner_statements: Vec<RunnerStatement>,
    pub mandate_provenance: Option<MandateProvenance>,
    pub children: usize,
    pub signed: bool,
    pub device_public_key: Option<String>,
    /// `Some(true)` when verified against the embedded `device_public_key`,
    /// `Some(false)` when that verification failed, or `None` when no embedded key was present.
    pub signature_verified: Option<bool>,
    /// What the verifier found wrong. Empty means every check it could make passed.
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

/// Exports a single receipt from `<root>/.vitna/receipts/<file_name>` to
/// `<root>/.vitna/exports/<stem>-<redaction>.json`, refusing symlinks and
/// stripping `device_signature` / `device_public_key` whenever signed fields
/// are redacted (`standard` or `strict`).
pub fn export_receipt(
    root: &Path,
    file_name: &str,
    redaction: &str,
) -> Result<String, String> {
    let bare_name = file_name
        .strip_prefix(".vitna/receipts/")
        .unwrap_or(file_name);
    if bare_name.is_empty()
        || bare_name.contains('/')
        || bare_name.contains('\\')
        || bare_name.contains("..")
    {
        return Err("invalid receipt filename".to_string());
    }
    if !matches!(redaction, "none" | "standard" | "strict") {
        return Err(format!(
            "unsupported redaction level '{redaction}': expected none, standard, or strict"
        ));
    }

    let vitna_dir = root.join(".vitna");
    if let Ok(meta) = fs::symlink_metadata(&vitna_dir) {
        if meta.file_type().is_symlink() {
            return Err(".vitna is a symlink and will not be followed".to_string());
        }
    }
    let rdir = receipts_dir(root);
    if let Ok(meta) = fs::symlink_metadata(&rdir) {
        if meta.file_type().is_symlink() {
            return Err(".vitna/receipts is a symlink and will not be followed".to_string());
        }
    }

    let src = rdir.join(bare_name);
    let meta = fs::symlink_metadata(&src)
        .map_err(|e| format!("cannot inspect receipt {bare_name}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err("receipt path is a symlink and will not be followed".to_string());
    }
    if !meta.is_file() {
        return Err("receipt path is not a regular file".to_string());
    }
    if meta.len() > MAX_BYTES {
        return Err("receipt exceeds maximum readable size".to_string());
    }

    let mut raw = String::new();
    fs::File::open(&src)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_string(&mut raw))
        .map_err(|e| format!("cannot read receipt {bare_name}: {e}"))?;

    let mut receipt: vitna_receipts::VitnaRunReceiptV1 =
        serde_json::from_str(&raw).map_err(|e| format!("receipt is not valid JSON: {e}"))?;

    let exported = match redaction {
        "none" => raw,
        "standard" => {
            receipt.workspace_fingerprint = "[REDACTED]".to_string();
            receipt.base_commit_sha = "[REDACTED]".to_string();
            for f in &mut receipt.changeset.files_modified {
                let basename = Path::new(&f.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                f.path = format!("<redacted>/{basename}");
            }
            receipt.device_signature.clear();
            receipt.device_public_key = None;
            let envelope = serde_json::json!({
                "redaction": "standard",
                "signature_stripped": true,
                "note": "Redacted receipts cannot carry the original device signature because signed fields were modified.",
                "receipt": receipt,
            });
            serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())?
        }
        "strict" => {
            receipt.workspace_fingerprint = "[REDACTED]".to_string();
            receipt.base_commit_sha = "[REDACTED]".to_string();
            for (idx, f) in receipt.changeset.files_modified.iter_mut().enumerate() {
                f.path = format!("redacted-file-{idx}");
            }
            for ev in &mut receipt.evidence_items {
                ev.description = "[REDACTED]".to_string();
                ev.artifact_digest = None;
            }
            receipt.device_signature.clear();
            receipt.device_public_key = None;
            let envelope = serde_json::json!({
                "redaction": "strict",
                "signature_stripped": true,
                "note": "Strictly redacted receipts strip paths, evidence descriptions, artifact digests, and the original device signature.",
                "receipt": receipt,
            });
            serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())?
        }
        _ => unreachable!(),
    };

    let exports_dir = vitna_dir.join("exports");
    if let Ok(meta) = fs::symlink_metadata(&exports_dir) {
        if meta.file_type().is_symlink() {
            return Err(".vitna/exports is a symlink and will not be written to".to_string());
        }
    }
    fs::create_dir_all(&exports_dir)
        .map_err(|e| format!("could not create .vitna/exports: {e}"))?;

    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("receipt");
    let out_name = format!("{stem}-{redaction}.json");
    let out_path = exports_dir.join(&out_name);
    if let Ok(meta) = fs::symlink_metadata(&out_path) {
        if meta.file_type().is_symlink() {
            return Err("export destination is a symlink and will not be overwritten".to_string());
        }
    }
    fs::write(&out_path, exported).map_err(|e| format!("could not write export: {e}"))?;
    Ok(format!(".vitna/exports/{out_name}"))
}

fn read_receipt_file_checked(
    root: &Path,
    file_name: &str,
) -> Result<vitna_receipts::VitnaRunReceiptV1, String> {
    let bare_name = file_name
        .strip_prefix(".vitna/receipts/")
        .unwrap_or(file_name);
    if bare_name.is_empty()
        || bare_name.contains('/')
        || bare_name.contains('\\')
        || bare_name.contains("..")
    {
        return Err("invalid receipt filename".to_string());
    }
    let vitna_dir = root.join(".vitna");
    if let Ok(meta) = fs::symlink_metadata(&vitna_dir) {
        if meta.file_type().is_symlink() {
            return Err(".vitna is a symlink and will not be followed".to_string());
        }
    }
    let rdir = receipts_dir(root);
    if let Ok(meta) = fs::symlink_metadata(&rdir) {
        if meta.file_type().is_symlink() {
            return Err(".vitna/receipts is a symlink and will not be followed".to_string());
        }
    }
    let src = rdir.join(bare_name);
    let meta = fs::symlink_metadata(&src)
        .map_err(|e| format!("cannot inspect receipt {bare_name}: {e}"))?;
    if meta.file_type().is_symlink() {
        return Err("receipt path is a symlink and will not be followed".to_string());
    }
    if !meta.is_file() {
        return Err("receipt path is not a regular file".to_string());
    }
    if meta.len() > MAX_BYTES {
        return Err("receipt exceeds maximum readable size".to_string());
    }
    let mut raw = String::new();
    fs::File::open(&src)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_string(&mut raw))
        .map_err(|e| format!("cannot read receipt {bare_name}: {e}"))?;
    if raw.len() as u64 > MAX_BYTES {
        return Err("receipt grew past maximum readable size".to_string());
    }
    serde_json::from_str(&raw).map_err(|e| format!("receipt is not valid JSON: {e}"))
}

/// Safely loads two receipts from `.vitna/receipts/` (refusing symlinks,
/// traversal, non-regular files, and oversized payloads) and returns a
/// terminal-sanitized summary diff line.
pub fn diff_receipts_summary(root: &Path, file_a: &str, file_b: &str) -> Result<String, String> {
    let a = read_receipt_file_checked(root, file_a)?;
    let b = read_receipt_file_checked(root, file_b)?;
    let d = vitna_receipt_verify::diff_receipts(&a, &b);

    let iso_tag = if d.isolation.regression {
        " [REGRESSION]"
    } else if d.isolation.changed {
        " [CHANGED]"
    } else {
        ""
    };
    let model_tag = if d.model.changed { " [CHANGED]" } else { "" };
    let egress_part = if d.egress.new_domains.is_empty() {
        format!("egress {}->{}", d.egress.count_a, d.egress.count_b)
    } else {
        format!(
            "egress {}->{} (new: {})",
            d.egress.count_a,
            d.egress.count_b,
            d.egress.new_domains.join(",")
        )
    };
    let mandate_part = match (&d.mandate.spent_a, &d.mandate.spent_b) {
        (None, None) => String::new(),
        (sa, sb) => format!(
            " | mandate {}->{}",
            sa.as_deref().unwrap_or("none"),
            sb.as_deref().unwrap_or("none")
        ),
    };

    let raw = format!(
        "Diff {} -> {}: model {}->{}{} | iso {}->{}{} | files +{} -{} ~{} ={} | {}{}",
        d.run_a,
        d.run_b,
        d.model.a.model_sku,
        d.model.b.model_sku,
        model_tag,
        d.isolation.a,
        d.isolation.b,
        iso_tag,
        d.files.only_in_b.len(),
        d.files.only_in_a.len(),
        d.files.postimage_changed.len(),
        d.files.unchanged.len(),
        egress_part,
        mandate_part,
    );
    Ok(clean(&raw))
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
    let signed = !r.device_signature.trim().is_empty();
    let mut failed_checks: Vec<String> = verified.report.errors.iter().map(|e| clean(e)).collect();
    let mut signature_verified: Option<bool> = None;
    if signed {
        if let Some(ref embedded_pk) = r.device_public_key {
            let trimmed_pk = embedded_pk.trim();
            if !trimmed_pk.is_empty() {
                match vitna_receipt_verify::ReceiptVerifier::verify_receipt(&r, Some(trimmed_pk)) {
                    Ok(sig_report) if sig_report.signature_verified => {
                        signature_verified = Some(true);
                    }
                    Ok(sig_report) => {
                        signature_verified = Some(false);
                        for err in sig_report.errors {
                            let cleaned = clean(&err);
                            if !failed_checks.contains(&cleaned) {
                                failed_checks.push(cleaned);
                            }
                        }
                    }
                    Err(e) => {
                        signature_verified = Some(false);
                        failed_checks.push(clean(&format!("signature check error: {e}")));
                    }
                }
            }
        }
    }

    Entry::Run(Box::new(Run {
        file: format!(".vitna/receipts/{file}"),
        written,
        run_id: clean(&r.run_id),
        base_commit: clean(&r.base_commit_sha),
        provider: clean(&r.model_selection.provider),
        model_sku: clean(&r.model_selection.model_sku),
        policy_digest: r
            .model_selection
            .policy_digest
            .as_deref()
            .map(clean)
            .filter(|s| !s.is_empty()),
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
        egress_calls: r
            .egress_calls
            .iter()
            .map(|eg| EgressCall {
                method: clean(&eg.method),
                redacted_url: clean(&eg.redacted_url),
                status_code: eg.status_code,
                bytes_received: eg.bytes_received,
                latency_ms: eg.latency_ms,
                response_sha256: clean(&eg.response_sha256),
            })
            .collect(),
        runner_statements: r
            .runner_execution_statements
            .iter()
            .map(|st| RunnerStatement {
                action_id: clean(&st.action_id),
                sandbox_backend: st.sandbox_backend.as_deref().map(clean),
                sandbox_enforcement: st.sandbox_enforcement.as_deref().map(clean),
            })
            .collect(),
        mandate_provenance: r.mandate_provenance.as_ref().map(|mp| MandateProvenance {
            root_jti: clean(&mp.root_jti),
            chain_hash: clean(&mp.chain_hash),
            ledger: clean(&mp.ledger),
            currency: clean(&mp.currency),
            total_spent: clean(&mp.total_spent),
            path: mp.path.iter().map(|s| clean(s)).collect(),
            spend_count: mp.spend_digests.len(),
        }),
        children: r.child_receipt_roots.len(),
        signed,
        device_public_key: r
            .device_public_key
            .as_deref()
            .map(clean)
            .filter(|s| !s.is_empty()),
        signature_verified,
        failed_checks,
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

    #[test]
    fn egress_runner_and_mandate_provenance_are_loaded_and_sanitized() {
        let dir = scratch("provenance");
        let body = serde_json::json!({
            "schema_version": "vitna-run-receipt-v1",
            "run_id": "run-prov-1",
            "session_id": "sess-1",
            "workspace_fingerprint": "fp",
            "base_commit_sha": "abc1234",
            "model_selection": {
                "provider": "vitna-anchor",
                "model_sku": "qwen3-coder-30b",
                "routing_reason": "vitna_anchor:qwen3-coder-30b"
            },
            "event_hash_chain_root": "root-1",
            "isolation_label": "strong",
            "completion_state": "completed_with_evidence",
            "evidence_items": [],
            "changeset": {
                "files_modified": [],
                "diff_digest": "diff-1"
            },
            "runner_execution_statements": [{
                "action_id": "act-1",
                "statement_digest": "stmt-1",
                "signature": "sig-1",
                "sandbox_backend": "oci_container",
                "sandbox_enforcement": "fully_enforced"
            }],
            "egress_calls": [{
                "target_domain": "api.example.com",
                "method": "GET",
                "redacted_url": "https://api.example.com/\u{001b}[31mstatus",
                "request_sha256": "a".repeat(64),
                "status_code": 200,
                "response_sha256": "b".repeat(64),
                "bytes_sent": 0,
                "bytes_received": 64,
                "latency_ms": 11
            }],
            "mandate_provenance": {
                "chain_hash": "sha256:chain123",
                "root_jti": "mnd_root_1",
                "path": ["mnd_root_1", "mnd_child_1"],
                "ledger": "https://ledger.example.com/\u{001b}]0;title\u{0007}",
                "currency": "USD",
                "total_spent": "0.12",
                "spend_digests": ["sha256:s1"]
            },
            "device_signature": "sig"
        });
        fs::write(
            receipts_dir(&dir).join("run-prov-1.json"),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();

        let ledger = read(&dir);
        let Some(Entry::Run(run)) = ledger.entries.first() else {
            panic!("expected Run entry, got {:?}", ledger.entries);
        };
        assert_eq!(run.runner_statements.len(), 1);
        assert_eq!(
            run.runner_statements[0].sandbox_backend.as_deref(),
            Some("oci_container")
        );
        assert_eq!(run.egress_calls.len(), 1);
        assert_eq!(run.egress_calls[0].status_code, 200);
        assert!(
            !run.egress_calls[0]
                .redacted_url
                .chars()
                .any(|c| c.is_control()),
            "control chars must be stripped from redacted_url"
        );
        let mp = run
            .mandate_provenance
            .as_ref()
            .expect("mandate_provenance loaded");
        assert_eq!(mp.root_jti, "mnd_root_1");
        assert_eq!(mp.spend_count, 1);
        assert!(
            !mp.ledger.chars().any(|c| c.is_control()),
            "control chars must be stripped from mandate ledger"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn embedded_key_signature_and_policy_digest_are_verified_in_ledger() {
        let dir = scratch("embedded_sig");
        let key = vitna_receipts::generate_signing_key();
        let mut r = vitna_receipts::VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: "run-signed-1".to_string(),
            session_id: "sess-1".to_string(),
            workspace_fingerprint: "a".repeat(64),
            base_commit_sha: "b".repeat(40),
            model_selection: vitna_receipts::ModelSelectionRecord {
                provider: "vitna-runtime".to_string(),
                model_sku: "vitna/code".to_string(),
                routing_reason: "runtime_lane:vitna/code".to_string(),
                policy_digest: Some("c".repeat(64)),
            },
            event_hash_chain_root: "d".repeat(64),
            isolation_label: "guarded".to_string(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: Vec::new(),
            changeset: vitna_receipts::ChangeSetRecord {
                files_modified: Vec::new(),
                diff_digest: "e".repeat(64),
            },
            runner_execution_statements: Vec::new(),
            egress_calls: Vec::new(),
            child_receipt_roots: Vec::new(),
            device_public_key: None,
            mandate_provenance: None,
            device_signature: String::new(),
        };
        r.sign(&key).expect("sign receipt");
        let valid_path = receipts_dir(&dir).join("run-signed-1.json");
        fs::write(&valid_path, serde_json::to_string_pretty(&r).unwrap()).unwrap();

        let ledger = read(&dir);
        let Some(Entry::Run(run)) = ledger.entries.first() else {
            panic!("expected Run entry");
        };
        assert_eq!(run.signature_verified, Some(true));
        assert!(run.failed_checks.is_empty(), "{:?}", run.failed_checks);
        assert_eq!(run.policy_digest.as_deref(), Some("c".repeat(64).as_str()));
        assert!(run.device_public_key.is_some());

        // Tamper with isolation_label and confirm TUI flags the broken signature.
        r.isolation_label = "strong".to_string();
        fs::write(&valid_path, serde_json::to_string_pretty(&r).unwrap()).unwrap();
        let tampered_ledger = read(&dir);
        let Some(Entry::Run(tampered_run)) = tampered_ledger.entries.first() else {
            panic!("expected Run entry");
        };
        assert_eq!(tampered_run.signature_verified, Some(false));
        assert!(
            !tampered_run.failed_checks.is_empty(),
            "tampered receipt must report failed check"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
