//! Implementations of Vitna CLI subcommands: sessions, resume, review, apply,
//! receipt show/export, live runtime `run`, and extended `doctor` probes.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use vitna_daemon::DaemonServer;
use vitna_git_broker::NULL_HASH;
use vitna_orchestration::{OrchestrationConfig, OrchestrationEngine, MAX_ROUNDS};
use vitna_protocol::messages::{
    run_state, ErrorResponse, HandshakeRequest, HandshakeResponse, MessageDelta, RunStateChanged,
    SubmitTurn, SubscribeEvents,
};
use vitna_protocol::{type_url, ProtocolEnvelope, MAX_FRAME_SIZE_BYTES};
use vitna_providers::{RuntimeLane, RuntimeLaneProvider, RUNTIME_PROVIDER_NAME};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::VitnaRunReceiptV1;
use vitna_runner::ProcessRunner;
use vitna_store::EventStore;
use vitna_tools::workspace_files::{EntryKind, FileError, WorkspaceFiles, ABSENT_PREIMAGE_HASH};

const PROBE_TIMEOUT: Duration = Duration::from_millis(600);
const TURN_STREAM_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_HANDSHAKE_REPLY_BYTES: u32 = 64 * 1024;

/// Sanitizes a public local identifier (such as a local session slug or UUID,
/// never a credential) by keeping only safe ASCII identifier characters so
/// corrupted local files cannot inject terminal escape sequences.
fn sanitize_public_id(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':') {
            cleaned.push(byte as char);
        }
    }
    cleaned
}

/// Writes one user-facing CLI output line directly to stdout.
fn write_stdout_line(line: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.write_all(b"\n");
}

/// Resolves the SQLite store path: explicit `--store`, else `.vitna/store.db` in
/// cwd if present, else `~/.vitna/daemon.db`.
pub fn resolve_store_path(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    let local = PathBuf::from(".vitna").join("store.db");
    if local.exists() {
        return Ok(local);
    }
    let home = vitna_daemon::launch::vitna_home()?;
    Ok(home.join("daemon.db"))
}

/// Validates a run ID before turning it into `.vitna/receipts/<run_id>.json`.
/// Rejects path separators, parent traversal, NUL bytes, and empty strings.
pub fn validate_run_id(run_id: &str) -> Result<&str, String> {
    let trimmed = run_id.trim();
    if trimmed.is_empty() {
        return Err("run_id cannot be empty".to_string());
    }
    if trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
        || trimmed.contains('\0')
        || trimmed == "."
    {
        return Err(format!(
            "Refusing unsafe run_id '{trimmed}': must not contain path separators or traversal segments"
        ));
    }
    Ok(trimmed)
}

fn read_receipt_from_workspace(
    workspace: &Path,
    run_id: &str,
) -> Result<(PathBuf, String, VitnaRunReceiptV1), String> {
    let id = validate_run_id(run_id)?;
    let files = WorkspaceFiles::open(workspace)
        .map_err(|e| format!("could not open workspace {}: {e}", workspace.display()))?;
    let rel = format!(".vitna/receipts/{id}.json");
    let bytes = files
        .read(&rel)
        .map_err(|e| format!("could not read {rel}: {e}"))?;
    let raw = String::from_utf8(bytes)
        .map_err(|e| format!("receipt {rel} is not valid UTF-8: {e}"))?;
    let receipt: VitnaRunReceiptV1 = serde_json::from_str(&raw)
        .map_err(|e| format!("receipt {rel} could not be parsed: {e}"))?;
    Ok((workspace.join(&rel), raw, receipt))
}

fn verify_signature_summary(
    receipt: &VitnaRunReceiptV1,
    explicit_key: Option<&str>,
) -> Result<(bool, String, Vec<String>), String> {
    let structural = ReceiptVerifier::verify_receipt(receipt, None)
        .map_err(|e| format!("receipt verification failed: {e}"))?;
    let mut errors = structural.errors;

    if let Some(given) = explicit_key {
        let report = ReceiptVerifier::verify_receipt(receipt, Some(given))
            .map_err(|e| format!("signature check error: {e}"))?;
        if report.signature_verified {
            return Ok((
                errors.is_empty(),
                "verified against the key given".to_string(),
                errors,
            ));
        }
        errors.extend(report.errors);
        return Ok((false, "failed against the key given".to_string(), errors));
    }

    let device = vitna_daemon::launch::vitna_home()
        .ok()
        .and_then(|home| vitna_daemon::key::public_key_in(&home).ok().flatten());
    match device {
        None => Ok((
            errors.is_empty(),
            "not checked (no --key given and no local device key found)".to_string(),
            errors,
        )),
        Some(public) => {
            let report = ReceiptVerifier::verify_receipt(receipt, Some(&public))
                .map_err(|e| format!("signature check error: {e}"))?;
            if report.signature_verified {
                Ok((
                    errors.is_empty(),
                    "verified against this device's key".to_string(),
                    errors,
                ))
            } else {
                let detail = if report
                    .errors
                    .iter()
                    .any(|e| e.contains("different device key"))
                {
                    "signed by a different device key (pass --key with the signer's public key)"
                } else {
                    "not verified against this device's key (signed elsewhere or modified)"
                };
                Ok((errors.is_empty(), detail.to_string(), errors))
            }
        }
    }
}

/// `vitna receipt show <run_id>`
pub fn run_receipt_show(
    run_id: &str,
    workspace: &Path,
    key: Option<&str>,
    as_json: bool,
) -> Result<(), String> {
    let (path, _raw, receipt) = read_receipt_from_workspace(workspace, run_id)?;
    let (valid, sig_summary, errors) = verify_signature_summary(&receipt, key)?;

    if as_json {
        let out = json!({
            "path": path.display().to_string(),
            "is_valid": valid,
            "signature_status": sig_summary,
            "errors": errors,
            "receipt": receipt,
        });
        println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
        if !valid {
            return Err(format!("Receipt {} failed verification", receipt.run_id));
        }
        return Ok(());
    }

    println!("Vitna Run Receipt");
    println!("=================");
    println!("File:              {}", path.display());
    println!("Run ID:            {}", receipt.run_id);
    write_stdout_line(&format!(
        "Session ID:        {}",
        sanitize_public_id(&receipt.session_id)
    ));
    println!("Schema:            {}", receipt.schema_version);
    println!(
        "Model:             {} ({}) [{}]",
        receipt.model_selection.model_sku,
        receipt.model_selection.provider,
        receipt.model_selection.routing_reason
    );
    println!("Isolation Label:   {}", receipt.isolation_label);
    println!("Completion State:  {}", receipt.completion_state);
    if let Some(pk) = &receipt.device_public_key {
        println!("Device Public Key: {}", pk);
    }
    println!("Device Signature:  {}", sig_summary);
    println!("Event Chain Root:  {}", receipt.event_hash_chain_root);
    println!(
        "Files Modified:    {} (diff digest: {})",
        receipt.changeset.files_modified.len(),
        receipt.changeset.diff_digest
    );
    for f in &receipt.changeset.files_modified {
        println!(
            "  - {} (pre: {}, post: {})",
            f.path,
            short_hash(&f.preimage_hash),
            short_hash(&f.postimage_hash)
        );
    }
    println!("Evidence Items:    {}", receipt.evidence_items.len());
    for ev in &receipt.evidence_items {
        println!("  - [{}] {}: {}", ev.grade, ev.evidence_id, ev.description);
    }
    println!(
        "Runner Statements: {}",
        receipt.runner_execution_statements.len()
    );
    for st in &receipt.runner_execution_statements {
        let backend = st.sandbox_backend.as_deref().unwrap_or("unspecified");
        let enforcement = st.sandbox_enforcement.as_deref().unwrap_or("unspecified");
        println!(
            "  - {} (sandbox: {}/{}, digest: {})",
            st.action_id,
            backend,
            enforcement,
            short_hash(&st.statement_digest)
        );
    }
    if !errors.is_empty() {
        println!("Errors:");
        for err in &errors {
            println!("  - {err}");
        }
        return Err(format!("Receipt {} failed verification", receipt.run_id));
    }

    Ok(())
}

/// `vitna receipt export <run_id> --redaction <none|standard|strict> [--out <path>]`
pub fn run_receipt_export(
    run_id: &str,
    workspace: &Path,
    redaction: &str,
    out: Option<&Path>,
) -> Result<(), String> {
    let (_path, raw, mut receipt) = read_receipt_from_workspace(workspace, run_id)?;

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
            // Modifying signed fields invalidates the original signature; strip it
            // honestly so no verifier is ever misled into thinking it still covers
            // the redacted payload.
            receipt.device_signature.clear();
            receipt.device_public_key = None;

            let envelope = json!({
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

            let envelope = json!({
                "redaction": "strict",
                "signature_stripped": true,
                "note": "Strictly redacted receipts strip paths, evidence descriptions, artifact digests, and the original device signature.",
                "receipt": receipt,
            });
            serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())?
        }
        other => {
            return Err(format!(
                "Unsupported redaction level '{other}'. Expected one of: none, standard, strict"
            ));
        }
    };

    if let Some(target) = out {
        if let Some(parent) = target.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
            }
        }
        fs::write(target, &exported)
            .map_err(|e| format!("could not write {}: {e}", target.display()))?;
        println!(
            "Exported receipt {} (redaction: {}) to {}",
            run_id,
            redaction,
            target.display()
        );
    } else {
        println!("{exported}");
    }

    Ok(())
}

/// `vitna sessions [--store <path>] [--json]`
pub fn run_list_sessions(store_arg: Option<&Path>, as_json: bool) -> Result<(), String> {
    let db_path = resolve_store_path(store_arg)?;
    if !db_path.exists() {
        if as_json {
            println!("[]");
        } else {
            println!("No session store found at {}.", db_path.display());
        }
        return Ok(());
    }

    let store = EventStore::open(&db_path)
        .map_err(|e| format!("could not open store {}: {e}", db_path.display()))?;
    let sessions = store
        .load_sessions()
        .map_err(|e| format!("could not load sessions: {e}"))?;

    if as_json {
        let rows: Vec<Value> = sessions
            .iter()
            .map(|s| {
                json!({
                    "session_id": s.session_id,
                    "workspace_root": s.workspace_root,
                    "created_at_ms": s.created_at_ms,
                    "status": s.status,
                    "run_count": s.runs.len(),
                    "latest_run_id": s.runs.last(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
        return Ok(());
    }

    println!("Vitna Sessions ({})", db_path.display());
    println!("--------------------------------");
    if sessions.is_empty() {
        println!("No sessions recorded.");
        return Ok(());
    }
    for s in sessions {
        let latest = s.runs.last().map(String::as_str).unwrap_or("none");
        let label = sanitize_public_id(&s.session_id);
        write_stdout_line(&format!(
            "- {} [{}] workspace={} runs={} latest_run={}",
            label,
            s.status,
            s.workspace_root,
            s.runs.len(),
            latest
        ));
    }
    Ok(())
}

/// `vitna resume [session_id] [--store <path>] [--prompt <text>] [--endpoint <ep>] [--json]`
pub async fn run_resume(
    session_id: Option<&str>,
    store_arg: Option<&Path>,
    prompt: Option<&str>,
    endpoint: Option<&str>,
    as_json: bool,
) -> Result<(), String> {
    let db_path = resolve_store_path(store_arg)?;
    if !db_path.exists() {
        return Err(format!(
            "No session store found at {}. Start a session with 'vitna run' or 'vitna serve' first.",
            db_path.display()
        ));
    }

    let store = EventStore::open(&db_path)
        .map_err(|e| format!("could not open store {}: {e}", db_path.display()))?;
    let sessions = store
        .load_sessions()
        .map_err(|e| format!("could not load sessions: {e}"))?;

    let target = match session_id {
        Some(wanted) => sessions
            .into_iter()
            .find(|s| s.session_id == wanted)
            .ok_or_else(|| format!("Session '{wanted}' not found in {}", db_path.display()))?,
        None => sessions
            .into_iter()
            .next()
            .ok_or_else(|| format!("No sessions recorded in {}", db_path.display()))?,
    };

    let latest_state = match target.runs.last() {
        Some(run_id) => Some(
            store
                .replay_run(run_id)
                .map_err(|e| format!("failed to replay run {run_id}: {e}"))?,
        ),
        None => None,
    };

    if let Some(turn_prompt) = prompt {
        return submit_prompt_to_daemon(&target.session_id, turn_prompt, endpoint).await;
    }

    if as_json {
        let out = json!({
            "session_id": target.session_id,
            "workspace_root": target.workspace_root,
            "status": target.status,
            "created_at_ms": target.created_at_ms,
            "runs": target.runs,
            "latest_run": latest_state.as_ref().map(|r| json!({
                "run_id": r.run_id,
                "state": r.state,
                "total_events": r.total_events,
                "last_sequence": r.last_sequence,
                "last_event_hash": r.last_event_hash,
            })),
        });
        println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
        return Ok(());
    }

    write_stdout_line(&format!(
        "Resumed Session: {}",
        sanitize_public_id(&target.session_id)
    ));
    println!("Workspace:       {}", target.workspace_root);
    println!("Status:          {}", target.status);
    println!("Recorded Runs:   {}", target.runs.len());
    if let Some(r) = latest_state {
        println!("Latest Run:      {} ({:?})", r.run_id, r.state);
        println!("Events Replayed: {} (last seq {})", r.total_events, r.last_sequence);
    } else {
        println!("Latest Run:      none");
    }
    println!(
        "Pass --prompt \"...\" to submit a follow-up turn to a running daemon for this session."
    );
    Ok(())
}

/// `vitna review [path_or_ref] [--workspace <path>] [--json]`
pub fn run_review(
    path_or_ref: Option<&str>,
    workspace: &Path,
    as_json: bool,
) -> Result<(), String> {
    let files = WorkspaceFiles::open(workspace)
        .map_err(|e| format!("could not open workspace {}: {e}", workspace.display()))?;

    // 1. Collect git working tree status via hardened host_git::command (never bare git).
    let mut status_cmd = vitna_git_workspaces::host_git::command(workspace)?;
    status_cmd.args(["status", "--porcelain=v1", "--untracked-files=all"]);
    if let Some(target) = path_or_ref {
        status_cmd.arg("--");
        status_cmd.arg(target);
    }
    let status_out = status_cmd
        .output()
        .map_err(|e| format!("failed to run git status: {e}"))?;
    let is_git_repo = status_out.status.success();

    let mut changed_paths: Vec<(String, String)> = Vec::new();
    if is_git_repo {
        let text = String::from_utf8_lossy(&status_out.stdout);
        for line in text.lines() {
            if line.len() < 4 {
                continue;
            }
            let code = line[..2].trim().to_string();
            let raw_path = line[3..].trim();
            let path = raw_path
                .split(" -> ")
                .last()
                .unwrap_or(raw_path)
                .trim_matches('"')
                .to_string();
            if !path.is_empty() {
                changed_paths.push((code, path));
            }
        }
    }

    // 2. Load receipts under .vitna/receipts safely via WorkspaceFiles.
    let mut receipts: Vec<VitnaRunReceiptV1> = Vec::new();
    if let Ok(entries) = files.list(".vitna/receipts", false) {
        let mut names: Vec<String> = entries
            .into_iter()
            .filter(|e| e.kind == EntryKind::File && e.name.ends_with(".json"))
            .map(|e| e.name)
            .collect();
        names.sort();
        for name in names {
            if let Ok(bytes) = files.read(&format!(".vitna/receipts/{name}")) {
                if let Ok(r) = serde_json::from_slice::<VitnaRunReceiptV1>(&bytes) {
                    receipts.push(r);
                }
            }
        }
    }

    // Index receipts by normalized file path -> matching attestations.
    let mut by_path: HashMap<String, Vec<Value>> = HashMap::new();
    for r in &receipts {
        let highest_grade = highest_evidence_grade(r);
        for fm in &r.changeset.files_modified {
            let norm = fm.path.replace('\\', "/");
            by_path.entry(norm).or_default().push(json!({
                "run_id": r.run_id,
                "completion_state": r.completion_state,
                "isolation_label": r.isolation_label,
                "preimage_hash": fm.preimage_hash,
                "postimage_hash": fm.postimage_hash,
                "highest_evidence_grade": highest_grade,
                "evidence_count": r.evidence_items.len(),
            }));
        }
    }

    // 3. Correlate changed files against receipts and current disk hashes.
    let mut findings: Vec<Value> = Vec::new();
    for (git_status, rel_path) in &changed_paths {
        let norm = rel_path.replace('\\', "/");
        let current_hash = match files.read(&norm) {
            Ok(bytes) => hex::encode(Sha256::digest(&bytes)),
            Err(FileError::Missing) => NULL_HASH.to_string(),
            Err(_) => "unreadable".to_string(),
        };
        let matching: Vec<Value> = by_path
            .get(&norm)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut entry| {
                let matches_postimage = entry["postimage_hash"].as_str() == Some(current_hash.as_str());
                entry["matches_current_disk_hash"] = json!(matches_postimage);
                entry
            })
            .collect();
        let attested = matching
            .iter()
            .any(|m| m["matches_current_disk_hash"].as_bool() == Some(true));
        findings.push(json!({
            "path": norm,
            "git_status": git_status,
            "current_sha256": current_hash,
            "attested_by_receipt": attested,
            "matching_receipts": matching,
        }));
    }

    if as_json {
        let out = json!({
            "workspace": workspace.display().to_string(),
            "is_git_repo": is_git_repo,
            "receipts_scanned": receipts.len(),
            "changed_files": findings,
        });
        println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
        return Ok(());
    }

    println!("Vitna Review ({})", workspace.display());
    println!("=====================================");
    if !is_git_repo {
        println!("Workspace is not a git repository; scanned {} receipt(s).", receipts.len());
        return Ok(());
    }
    println!(
        "Working tree changes: {} | Receipts scanned: {}",
        findings.len(),
        receipts.len()
    );
    if findings.is_empty() {
        println!("Working tree is clean.");
        return Ok(());
    }
    for row in &findings {
        let path = row["path"].as_str().unwrap_or("");
        let status = row["git_status"].as_str().unwrap_or("?");
        let attested = row["attested_by_receipt"].as_bool().unwrap_or(false);
        let matches = row["matching_receipts"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        if attested {
            let runs: Vec<&str> = matches
                .iter()
                .filter(|m| m["matches_current_disk_hash"].as_bool() == Some(true))
                .filter_map(|m| m["run_id"].as_str())
                .collect();
            let grade = matches
                .iter()
                .filter(|m| m["matches_current_disk_hash"].as_bool() == Some(true))
                .filter_map(|m| m["highest_evidence_grade"].as_str())
                .next()
                .unwrap_or("none");
            println!(
                "  [{status}] {path}  -> ATTESTED by {} (highest evidence: {grade})",
                runs.join(", ")
            );
        } else if !matches.is_empty() {
            println!(
                "  [{status}] {path}  -> DRIFTED (current disk hash does not match any recorded receipt postimage)"
            );
        } else {
            println!("  [{status}] {path}  -> UNATTESTED (no matching receipt)");
        }
    }
    Ok(())
}

/// `vitna apply <change_set_id> [--workspace <path>] [--source <dir>] [--check]`
pub fn run_apply(
    change_set_id: &str,
    workspace: &Path,
    source: Option<&Path>,
    check_only: bool,
) -> Result<(), String> {
    let receipt = load_receipt_by_id_or_path(workspace, change_set_id)?;
    let target_files = WorkspaceFiles::open(workspace)
        .map_err(|e| format!("could not open target workspace {}: {e}", workspace.display()))?;
    let source_files = match source {
        Some(dir) => Some(
            WorkspaceFiles::open(dir)
                .map_err(|e| format!("could not open source workspace {}: {e}", dir.display()))?,
        ),
        None => None,
    };

    if receipt.changeset.files_modified.is_empty() {
        println!(
            "Receipt {} records 0 modified files; nothing to apply.",
            receipt.run_id
        );
        return Ok(());
    }

    // Phase 1: Validate every preimage (and source postimage when --source is given)
    // before mutating a single file.
    let mut conflicts: Vec<String> = Vec::new();
    let mut already_applied = 0usize;
    let mut pending: Vec<(&vitna_receipts::FileModificationRecord, Option<Vec<u8>>)> = Vec::new();

    for fm in &receipt.changeset.files_modified {
        let current_hash = match target_files.read(&fm.path) {
            Ok(bytes) => hex::encode(Sha256::digest(&bytes)),
            Err(FileError::Missing) => NULL_HASH.to_string(),
            Err(e) => {
                conflicts.push(format!("{}: cannot read target ({e})", fm.path));
                continue;
            }
        };

        let preimage_matches = is_absent_hash(&fm.preimage_hash) && current_hash == NULL_HASH
            || current_hash.eq_ignore_ascii_case(&fm.preimage_hash);
        let postimage_matches = is_absent_hash(&fm.postimage_hash) && current_hash == NULL_HASH
            || current_hash.eq_ignore_ascii_case(&fm.postimage_hash);

        if postimage_matches && !preimage_matches {
            already_applied += 1;
            continue;
        }
        if !preimage_matches {
            conflicts.push(format!(
                "{}: preimage mismatch (expected {}, found on disk {})",
                fm.path,
                short_hash(&fm.preimage_hash),
                short_hash(&current_hash)
            ));
            continue;
        }

        if let Some(src) = &source_files {
            if is_absent_hash(&fm.postimage_hash) {
                pending.push((fm, None));
            } else {
                let bytes = match src.read(&fm.path) {
                    Ok(b) => b,
                    Err(e) => {
                        conflicts.push(format!("{}: missing or unreadable in source ({e})", fm.path));
                        continue;
                    }
                };
                let src_hash = hex::encode(Sha256::digest(&bytes));
                if !src_hash.eq_ignore_ascii_case(&fm.postimage_hash) {
                    conflicts.push(format!(
                        "{}: source postimage hash mismatch (expected {}, source has {})",
                        fm.path,
                        short_hash(&fm.postimage_hash),
                        short_hash(&src_hash)
                    ));
                    continue;
                }
                pending.push((fm, Some(bytes)));
            }
        } else {
            pending.push((fm, None));
        }
    }

    if !conflicts.is_empty() {
        let mut msg = format!(
            "Refusing to apply changeset {} due to {} conflict(s):\n",
            receipt.run_id,
            conflicts.len()
        );
        for c in &conflicts {
            msg.push_str(&format!("  - {c}\n"));
        }
        return Err(msg.trim_end().to_string());
    }

    if check_only || source_files.is_none() {
        println!(
            "Preimage validation PASSED for {} ({} ready, {} already at postimage).",
            receipt.run_id,
            pending.len(),
            already_applied
        );
        if !check_only && !pending.is_empty() {
            println!(
                "No files written: pass --source <agent-workspace-dir> to apply verified postimage bytes."
            );
        }
        return Ok(());
    }

    // Phase 2: Apply each file atomically via CAS write/remove on WorkspaceFiles.
    let mut applied: Vec<String> = Vec::new();
    for (fm, maybe_bytes) in pending {
        let cas_expect = if is_absent_hash(&fm.preimage_hash) {
            ABSENT_PREIMAGE_HASH
        } else {
            fm.preimage_hash.as_str()
        };
        match maybe_bytes {
            Some(bytes) => {
                target_files
                    .write(&fm.path, cas_expect, &bytes)
                    .map_err(|e| format!("failed to write {}: {e}", fm.path))?;
                applied.push(fm.path.clone());
            }
            None => {
                target_files
                    .remove(&fm.path, cas_expect)
                    .map_err(|e| format!("failed to remove {}: {e}", fm.path))?;
                applied.push(fm.path.clone());
            }
        }
    }

    println!(
        "Applied changeset {} cleanly: {} file(s) updated, {} already up to date.",
        receipt.run_id,
        applied.len(),
        already_applied
    );
    for path in &applied {
        println!("  - {path}");
    }
    Ok(())
}

/// Options for `vitna run --runtime-url <url> --lane <lane>`.
pub struct LiveRunOptions<'a> {
    pub task: &'a str,
    pub workspace: &'a Path,
    pub store_arg: Option<&'a Path>,
    pub runtime_url: &'a str,
    pub lane_name: &'a str,
    pub auto_approve: bool,
    pub verify_cmd: Option<String>,
    pub allow_unsandboxed: bool,
}

/// Runs a live model-driven agent turn via `vitna-runtime` when `--runtime-url`
/// and `--lane` are passed to `vitna run`.
pub async fn run_live_runtime_turn(opts: LiveRunOptions<'_>) -> Result<(), String> {
    let lane = RuntimeLane::new(opts.runtime_url, opts.lane_name)?;
    let provider = RuntimeLaneProvider::new(lane.clone());

    let db_path = match opts.store_arg {
        Some(p) => p.to_path_buf(),
        None => opts.workspace.join(".vitna").join("store.db"),
    };
    let home = vitna_daemon::launch::vitna_home()?;
    fs::create_dir_all(&home)
        .map_err(|e| format!("could not create {}: {e}", home.display()))?;
    let signing_key = vitna_daemon::key::load_or_create(&home)?;
    let daemon = DaemonServer::open_with_key_in(&db_path, &home)?;
    let session = daemon.create_session(opts.workspace)?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let run_id = format!("run-{now_ms}");
    let next_seq = daemon.next_session_sequence(&session.session_id)?;
    daemon.register_run(&session.session_id, &run_id)?;

    let store = EventStore::open(&db_path)
        .map_err(|e| format!("could not open event store {}: {e}", db_path.display()))?;
    let journal_path = db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("vitna_runner.journal");
    let runner = ProcessRunner::open_or_create(journal_path)
        .map_err(|e| format!("could not initialize runner journal: {e}"))?;

    let config = OrchestrationConfig {
        session_id: session.session_id.clone(),
        run_id: run_id.clone(),
        workspace_root: opts.workspace.to_path_buf(),
        auto_approve: opts.auto_approve,
        model_sku: lane.lane.clone(),
        provider_name: RUNTIME_PROVIDER_NAME.to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: opts.verify_cmd,
        allow_unsandboxed: opts.allow_unsandboxed,
    };

    let mut engine = OrchestrationEngine::new(
        config,
        Arc::new(Mutex::new(store)),
        Arc::new(runner),
        signing_key,
    )
    .continuing_at(next_seq);
    engine.routing_reason = format!("runtime_lane:{}", lane.lane);

    println!("Vitna Code Live Agent Run");
    println!("-------------------------");
    println!("Task:      {}", opts.task);
    println!("Workspace: {}", opts.workspace.display());
    write_stdout_line(&format!(
        "Session:   {}",
        sanitize_public_id(&session.session_id)
    ));
    println!("Runtime:   {} (lane: {})", lane.endpoint, lane.lane);

    let turn = engine
        .run_agent_turn(&provider, &[], opts.task, MAX_ROUNDS)
        .await?;
    let receipt = engine.finalize_run().await?;

    if !turn.text.is_empty() {
        println!("\nModel Response:\n{}", turn.text);
    }
    println!("\nRun Complete");
    println!("============");
    println!("Run ID:            {}", receipt.run_id);
    println!("Completion State:  {}", receipt.completion_state);
    println!("Rounds:            {}", turn.rounds);
    println!("Files Modified:    {}", receipt.changeset.files_modified.len());
    println!("Evidence Captured: {}", receipt.evidence_items.len());
    println!("Receipt Path:      .vitna/receipts/{}.json", receipt.run_id);
    Ok(())
}

/// Probes the declared daemon IPC endpoints with a real `HandshakeRequest` and
/// returns `(is_listening, status_line)`.
pub async fn probe_daemon_endpoint() -> (bool, String) {
    let candidates = vitna_protocol::endpoint::candidates();
    if candidates.is_empty() {
        return (
            false,
            "[INFO] Daemon: no declared IPC endpoint on this environment".to_string(),
        );
    }
    for ep in &candidates {
        if let Ok(Ok(reply)) =
            tokio::time::timeout(PROBE_TIMEOUT, handshake_endpoint(ep)).await
        {
            if reply.accepted {
                return (
                    true,
                    format!(
                        "[OK] Daemon: listening on {ep} (protocol {}.{}, build {})",
                        reply.selected_version_major,
                        reply.selected_version_minor,
                        if reply.daemon_build_commit.is_empty() {
                            "dev"
                        } else {
                            reply.daemon_build_commit.as_str()
                        }
                    ),
                );
            }
            return (
                false,
                format!(
                    "[WARN] Daemon at {ep} refused handshake: {}",
                    reply.rejection_reason
                ),
            );
        }
    }
    let primary = candidates.first().map(String::as_str).unwrap_or("default");
    (
        false,
        format!("[INFO] Daemon: not running on {primary} (start with 'vitna serve')"),
    )
}

async fn submit_prompt_to_daemon(
    session_id: &str,
    prompt: &str,
    explicit_endpoint: Option<&str>,
) -> Result<(), String> {
    let ep = match explicit_endpoint {
        Some(e) => e.to_string(),
        None => vitna_protocol::endpoint::preferred()?,
    };
    let mut stream = connect_endpoint(&ep).await?;
    let hs = do_handshake(&mut stream).await?;
    if !hs.accepted {
        return Err(format!("Daemon refused handshake: {}", hs.rejection_reason));
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    send_envelope(
        &mut stream,
        type_url::command::SUBSCRIBE_EVENTS,
        session_id,
        "",
        2,
        &SubscribeEvents {
            session_id: session_id.to_string(),
            resume_after_sequence: 0,
        },
    )
    .await?;

    send_envelope(
        &mut stream,
        type_url::command::SUBMIT_TURN,
        session_id,
        "",
        3,
        &SubmitTurn {
            session_id: session_id.to_string(),
            prompt: prompt.to_string(),
            attached_file_paths: Vec::new(),
            idempotency_key: format!("cli-resume-{now_ms}"),
        },
    )
    .await?;

    let deadline = tokio::time::Instant::now() + TURN_STREAM_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("Timed out waiting for daemon turn to finish".to_string());
        }
        let env = tokio::time::timeout(
            remaining,
            read_envelope(&mut stream, MAX_FRAME_SIZE_BYTES),
        )
        .await
        .map_err(|_| "Timed out reading daemon event stream".to_string())??;

        match env.type_url.as_str() {
            type_url::event::MESSAGE_DELTA => {
                if let Ok(delta) = serde_json::from_slice::<MessageDelta>(&env.payload) {
                    if delta.role == "model" && !delta.delta_text.is_empty() {
                        print!("{}", delta.delta_text);
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                    }
                }
            }
            type_url::event::RUN_STATE_CHANGED => {
                if let Ok(state) = serde_json::from_slice::<RunStateChanged>(&env.payload) {
                    if matches!(
                        state.new_state.as_str(),
                        run_state::COMPLETED
                            | run_state::FAILED
                            | run_state::CANCELLED
                            | "needs_reconciliation"
                    ) {
                        println!("\nRun {} finished: {}", state.run_id, state.new_state);
                        return Ok(());
                    }
                }
            }
            type_url::envelope::ERROR_RESPONSE => {
                if let Ok(err) = serde_json::from_slice::<ErrorResponse>(&env.payload) {
                    return Err(format!("Daemon error ({:?}): {}", err.code, err.message));
                }
            }
            _ => {}
        }
    }
}

fn load_receipt_by_id_or_path(
    workspace: &Path,
    id_or_path: &str,
) -> Result<VitnaRunReceiptV1, String> {
    let as_path = Path::new(id_or_path);
    if as_path.is_file() {
        let text = fs::read_to_string(as_path)
            .map_err(|e| format!("could not read {}: {e}", as_path.display()))?;
        return serde_json::from_str(&text)
            .map_err(|e| format!("could not parse {}: {e}", as_path.display()));
    }
    let (_path, _raw, receipt) = read_receipt_from_workspace(workspace, id_or_path)?;
    Ok(receipt)
}

fn highest_evidence_grade(receipt: &VitnaRunReceiptV1) -> &'static str {
    let rank = |grade: &str| match grade {
        "remote_or_hardware_attested" => 5,
        "independently_reproduced" => 4,
        "sandbox_captured" => 3,
        "broker_observed" => 2,
        "model_reported" => 1,
        _ => 0,
    };
    let mut best_rank = 0;
    let mut best_label = "none";
    for item in &receipt.evidence_items {
        let r = rank(&item.grade);
        if r > best_rank {
            best_rank = r;
            best_label = match r {
                5 => "remote_or_hardware_attested",
                4 => "independently_reproduced",
                3 => "sandbox_captured",
                2 => "broker_observed",
                1 => "model_reported",
                _ => "none",
            };
        }
    }
    best_label
}

fn is_absent_hash(hash: &str) -> bool {
    hash == NULL_HASH || hash == ABSENT_PREIMAGE_HASH
}

fn short_hash(hash: &str) -> &str {
    if hash.len() > 12 {
        &hash[..12]
    } else {
        hash
    }
}

#[cfg(unix)]
type IpcStream = tokio::net::UnixStream;

#[cfg(windows)]
type IpcStream = tokio::net::windows::named_pipe::NamedPipeClient;

#[cfg(unix)]
async fn connect_endpoint(endpoint: &str) -> Result<IpcStream, String> {
    tokio::net::UnixStream::connect(endpoint)
        .await
        .map_err(|e| format!("could not connect to {endpoint}: {e}"))
}

#[cfg(windows)]
async fn connect_endpoint(endpoint: &str) -> Result<IpcStream, String> {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .map_err(|e| format!("could not connect to {endpoint}: {e}"))
}

async fn handshake_endpoint(endpoint: &str) -> Result<HandshakeResponse, String> {
    let mut stream = connect_endpoint(endpoint).await?;
    do_handshake(&mut stream).await
}

async fn do_handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
) -> Result<HandshakeResponse, String> {
    send_envelope(
        stream,
        type_url::envelope::HANDSHAKE_REQUEST,
        "",
        "",
        1,
        &HandshakeRequest {
            min_supported_version: vitna_protocol::PROTOCOL_VERSION_MAJOR,
            max_supported_version: vitna_protocol::PROTOCOL_VERSION_MAJOR,
            client_identifier: "vitna-cli/0.1.0".to_string(),
        },
    )
    .await?;
    let reply = read_envelope(stream, MAX_HANDSHAKE_REPLY_BYTES).await?;
    if reply.type_url != type_url::envelope::HANDSHAKE_RESPONSE {
        return Err(format!("unexpected handshake reply type: {}", reply.type_url));
    }
    serde_json::from_slice(&reply.payload)
        .map_err(|e| format!("invalid handshake response: {e}"))
}

async fn send_envelope<S: AsyncWrite + Unpin, T: serde::Serialize>(
    stream: &mut S,
    type_url_str: &str,
    session_id: &str,
    run_id: &str,
    sequence: u64,
    body: &T,
) -> Result<(), String> {
    let env = ProtocolEnvelope::carrying(type_url_str, session_id, run_id, sequence, "", body)
        .map_err(|e| format!("failed to build envelope: {e}"))?;
    let framed = env
        .encode_frame()
        .map_err(|e| format!("failed to frame envelope: {e}"))?;
    stream
        .write_all(&framed)
        .await
        .map_err(|e| format!("failed to write to daemon: {e}"))
}

async fn read_envelope<S: AsyncRead + Unpin>(
    stream: &mut S,
    max_len: u32,
) -> Result<ProtocolEnvelope, String> {
    let mut header = [0u8; 4];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|e| format!("failed to read frame length: {e}"))?;
    let len = u32::from_be_bytes(header);
    if len == 0 || len > max_len {
        return Err(format!("daemon frame size {len} out of bounds"));
    }
    let mut buf = vec![0u8; len as usize];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|e| format!("failed to read frame body: {e}"))?;
    serde_json::from_slice(&buf).map_err(|e| format!("invalid daemon envelope: {e}"))
}
