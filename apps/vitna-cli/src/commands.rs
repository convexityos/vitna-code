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
use vitna_providers::{RuntimeLaneProvider, RUNTIME_PROVIDER_NAME};
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
    if !receipt.egress_calls.is_empty() {
        println!("Egress Calls:      {}", receipt.egress_calls.len());
        for eg in &receipt.egress_calls {
            println!(
                "  - {} {} -> {} ({} bytes, {}ms, sha256: {})",
                eg.method,
                eg.redacted_url,
                eg.status_code,
                eg.bytes_received,
                eg.latency_ms,
                short_hash(&eg.response_sha256)
            );
        }
    }
    if let Some(mp) = &receipt.mandate_provenance {
        println!(
            "Mandate (am/1):    root {} | spent {} {} ({} receipt(s)) | chain {}",
            mp.root_jti,
            mp.total_spent,
            mp.currency,
            mp.spend_digests.len(),
            short_hash(&mp.chain_hash)
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
                    "mode": s.mode,
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
            "- {} [{}] mode={} workspace={} runs={} latest_run={}",
            label,
            s.status,
            s.mode,
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
            "mode": target.mode,
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
    println!("Mode:            {}", target.mode);
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

/// Options for `vitna run` with a live model route (`--runtime-url`/`--lane` or
/// OS-keychain `--provider`/`--model`) and optional MCP servers.
pub struct LiveRunOptions<'a> {
    pub task: &'a str,
    pub workspace: &'a Path,
    pub mode: &'a str,
    pub store_arg: Option<&'a Path>,
    pub runtime_url: Option<String>,
    pub lane: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub checkpoint: Option<PathBuf>,
    pub mcp_http: Vec<String>,
    pub mcp_stdio: Vec<String>,
    pub allow_egress: Vec<String>,
    pub sandbox_guarantee: Option<String>,
    pub sandbox_image: Option<String>,
    pub mandate: Option<PathBuf>,
    pub auto_approve: bool,
    pub verify_cmd: Option<String>,
    pub allow_unsandboxed: bool,
}

/// Runs a live model-driven agent turn via a local `vitna-runtime` lane, a
/// local `vitna-anchor` server, or a direct OS-keychain provider route
/// (`anthropic` / `openai`), wiring Trust Level 5 workspace skills and any
/// attached MCP servers.
pub async fn run_live_turn(opts: LiveRunOptions<'_>) -> Result<(), String> {
    use vitna_providers::{
        AnchorConfig, AnthropicProvider, CredentialResolver, KeyringStore, OpenAIProvider,
        Provider, ProviderId, VitnaAnchorProvider,
    };

    // Reuse Launch's exact route, MCP server, egress, sandbox, and mandate
    // validation so `vitna run`, `vitna serve`, and `vitna-coded` never drift.
    let launch = vitna_daemon::launch::Launch::resolve(
        Some("vitna-run".to_string()),
        Some(PathBuf::from("vitna-run.db")),
    )?
    .with_runtime(opts.runtime_url, opts.lane)?
    .with_direct_provider(opts.provider, opts.model, opts.base_url)?
    .with_checkpoint(opts.checkpoint)?
    .with_mcp_servers(opts.mcp_http, opts.mcp_stdio)?
    .with_egress_allowlist(opts.allow_egress)?
    .with_sandbox(opts.sandbox_guarantee, opts.sandbox_image)?
    .with_mandate(opts.mandate)?;

    let db_path = match opts.store_arg {
        Some(p) => p.to_path_buf(),
        None => opts.workspace.join(".vitna").join("store.db"),
    };
    let home = vitna_daemon::launch::vitna_home()?;
    fs::create_dir_all(&home)
        .map_err(|e| format!("could not create {}: {e}", home.display()))?;
    let signing_key = vitna_daemon::key::load_or_create(&home)?;
    let mut daemon = DaemonServer::open_with_key_in(&db_path, &home)?;

    let mandate_session = if let Some(ref mandate_path) = launch.mandate_file {
        let session = Arc::new(vitna_mcp::LiveMandateSession::from_file(mandate_path)?);
        daemon = daemon.with_mandate(Arc::clone(&session));
        Some(session)
    } else {
        None
    };

    let telemetry = daemon.telemetry();
    let (provider, model_sku, provider_name, routing_reason, route_label, policy_digest): (
        Arc<dyn Provider>,
        String,
        String,
        String,
        String,
        Option<String>,
    ) = if let Some(lane) = &launch.route {
        let p: Arc<dyn Provider> =
            Arc::new(RuntimeLaneProvider::new(lane.clone()).with_telemetry(telemetry));
        (
            p,
            lane.lane.clone(),
            RUNTIME_PROVIDER_NAME.to_string(),
            format!("runtime_lane:{}", lane.lane),
            format!("{} (lane: {})", lane.endpoint, lane.lane),
            None,
        )
    } else if let Some(direct) = &launch.direct {
        let provider_id = direct.provider;
        let mut creds = tokio::task::spawn_blocking(move || {
            let store = KeyringStore::open()?;
            let resolver = CredentialResolver::new(Arc::new(store));
            resolver.resolve(provider_id)
        })
        .await
        .map_err(|e| format!("keychain lookup task failed: {e}"))?
        .map_err(|e| {
            format!(
                "{e}. Store a key with `vitna auth set {}`",
                provider_id.as_str()
            )
        })?;
        creds.base_url = direct.base_url.clone();
        let p: Arc<dyn Provider> = match provider_id {
            ProviderId::Anthropic => Arc::new(AnthropicProvider::new(creds).with_telemetry(telemetry)),
            ProviderId::OpenAi => Arc::new(OpenAIProvider::new(creds).with_telemetry(telemetry)),
        };
        (
            p,
            direct.model.clone(),
            provider_id.as_str().to_string(),
            format!("direct_keychain:{}", provider_id.as_str()),
            format!("{} ({})", provider_id.as_str(), direct.model),
            None,
        )
    } else if let Some(anchor) = &launch.anchor {
        let mut cfg = AnchorConfig::default().with_base_url(&anchor.base_url);
        if let Some(cp) = &anchor.checkpoint_path {
            cfg = cfg.with_checkpoint_path(cp);
        }
        let adapter = VitnaAnchorProvider::new(cfg)?;
        let checkpoint_digest = if anchor.checkpoint_path.is_some() {
            let att = adapter.build_attestation().await?;
            att.checkpoint_sha256
        } else {
            match adapter.build_attestation().await {
                Ok(att) => att.checkpoint_sha256,
                Err(_) => None,
            }
        };
        let p: Arc<dyn Provider> = Arc::new(adapter);
        (
            p,
            anchor.model.clone(),
            "vitna-anchor".to_string(),
            format!("vitna_anchor:{}", anchor.model),
            format!("{} (vitna-anchor: {})", anchor.base_url, anchor.model),
            checkpoint_digest,
        )
    } else {
        return Err(
            "no model route specified: pass --runtime-url/--lane or --provider/--model".to_string(),
        );
    };

    for spec in &launch.mcp_servers {
        daemon.attach_mcp_server(spec).await?;
    }
    let extra_tools = daemon.mcp_tools();
    let mcp_tool_count = extra_tools.len();

    let session = daemon.create_session_with_mode(opts.workspace, opts.mode)?;
    let is_inspect = session.mode == "inspect";

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
        model_sku,
        provider_name,
        sandbox_guarantee: if is_inspect {
            "read_only".to_string()
        } else {
            launch.sandbox_guarantee.clone()
        },
        verification_command: opts.verify_cmd,
        allow_unsandboxed: opts.allow_unsandboxed,
    };

    let mut engine = OrchestrationEngine::new(
        config,
        Arc::new(Mutex::new(store)),
        Arc::new(runner),
        signing_key,
    )
    .continuing_at(next_seq)
    .with_extra_tools(extra_tools)
    .with_egress_allowlist(launch.egress_allowlist.clone())
    .with_policy_digest(policy_digest)
    .with_sandbox_image(launch.sandbox_image.clone())
    .with_read_only_mode(is_inspect);
    if let Some(ref mandate) = mandate_session {
        let m = Arc::clone(mandate);
        engine = engine.with_mandate_provenance_fn(Arc::new(move || m.provenance_record()));
    }
    engine.routing_reason = routing_reason;

    let mut loaded_skill_names = Vec::new();
    if let Ok(registry) = vitna_skills::SkillRegistry::discover_in_workspace(opts.workspace) {
        if let Some(block) = registry.render_untrusted_prompt_block() {
            loaded_skill_names = registry
                .skills()
                .iter()
                .map(|s| s.name.clone())
                .collect();
            engine = engine.with_skill_instructions(block, loaded_skill_names.clone());
        }
    }

    println!("Vitna Code Live Agent Run");
    println!("-------------------------");
    println!("Task:       {}", opts.task);
    println!("Workspace:  {}", opts.workspace.display());
    println!(
        "Mode:       {}",
        if is_inspect {
            "inspect (read-only)"
        } else {
            &session.mode
        }
    );
    write_stdout_line(&format!(
        "Session:    {}",
        sanitize_public_id(&session.session_id)
    ));
    println!("Route:      {route_label}");
    if mcp_tool_count > 0 {
        println!("MCP Tools:  {mcp_tool_count} attached");
    }
    if !launch.egress_allowlist.is_empty() {
        println!("Egress:     {}", launch.egress_allowlist.join(", "));
    }
    if !loaded_skill_names.is_empty() {
        println!("Skills:     {}", loaded_skill_names.join(", "));
    }

    let turn = engine
        .run_agent_turn(provider.as_ref(), &[], opts.task, MAX_ROUNDS)
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
    if !receipt.egress_calls.is_empty() {
        println!("Egress Calls:      {}", receipt.egress_calls.len());
    }
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

fn parse_provider_id(raw: &str) -> Result<vitna_providers::ProviderId, String> {
    let normalized = raw.trim().to_ascii_lowercase();
    vitna_providers::ProviderId::from_name(&normalized).ok_or_else(|| {
        format!(
            "unknown provider '{}': expected 'anthropic' or 'openai'",
            sanitize_public_id(raw)
        )
    })
}

/// `vitna auth status [--json]`
pub fn run_auth_status(as_json: bool) -> Result<(), String> {
    use vitna_providers::{CredentialError, CredentialResolver, KeyringStore, ProviderId};

    let (store_name, rows) = match KeyringStore::open() {
        Ok(store) => {
            let resolver = CredentialResolver::new(Arc::new(store));
            let name = resolver.source_name().to_string();
            let mut rows = Vec::new();
            for provider in ProviderId::ALL {
                let (state, detail) = match resolver.check(provider) {
                    Ok(()) => ("configured".to_string(), None),
                    Err(CredentialError::AuthenticationRequired { .. }) => {
                        ("not_configured".to_string(), None)
                    }
                    Err(e) => ("unavailable".to_string(), Some(e.to_string())),
                };
                rows.push(json!({
                    "provider": provider.as_str(),
                    "status": state,
                    "detail": detail,
                }));
            }
            (name, rows)
        }
        Err(open_err) => {
            let detail = open_err.to_string();
            let rows = ProviderId::ALL
                .into_iter()
                .map(|provider| {
                    json!({
                        "provider": provider.as_str(),
                        "status": "unavailable",
                        "detail": detail,
                    })
                })
                .collect();
            ("os-keychain (unavailable)".to_string(), rows)
        }
    };

    if as_json {
        let out = json!({
            "store": store_name,
            "ambient_env_scanning": false,
            "providers": rows,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    println!("Vitna Provider Credentials ({store_name})");
    println!("------------------------------------------");
    for row in &rows {
        let provider = row["provider"].as_str().unwrap_or("");
        let status = row["status"].as_str().unwrap_or("");
        match row["detail"].as_str() {
            Some(detail) => println!("  - {provider}: {status} ({detail})"),
            None => println!("  - {provider}: {status}"),
        }
    }
    println!("Note: Vitna never scans environment variables for API keys. Use `vitna auth set <provider>` via stdin.");
    Ok(())
}

/// `vitna auth set <provider>`
///
/// Reads the API key strictly from stdin (never CLI argv, so it does not leak
/// into process listings) and writes it to the OS keychain without logging or
/// echoing any part of the secret.
pub fn run_auth_set(provider_name: &str) -> Result<(), String> {
    use std::io::Read;
    use vitna_providers::{ApiKey, CredentialSource, CredentialStore, KeyringStore};

    let provider = parse_provider_id(provider_name)?;
    let mut raw = String::new();
    let mut handle = std::io::stdin().lock().take(8 * 1024);
    handle
        .read_to_string(&mut raw)
        .map_err(|e| format!("failed to read API key from stdin: {e}"))?;

    let parsed = ApiKey::parse(&raw);
    drop(raw);
    let key = parsed.map_err(|e| {
        format!(
            "invalid API key for {}: {}",
            provider.as_str(),
            e.reason()
        )
    })?;

    let store = KeyringStore::open()
        .map_err(|e| format!("OS keychain unavailable for {}: {e}", provider.as_str()))?;
    store
        .set_api_key(provider, &key)
        .map_err(|e| format!("could not store key for {}: {e}", provider.as_str()))?;
    drop(key);

    println!("Stored API key for {} in {}.", provider.as_str(), store.name());
    Ok(())
}

/// `vitna auth clear <provider>`
pub fn run_auth_clear(provider_name: &str) -> Result<(), String> {
    use vitna_providers::{CredentialSource, CredentialStore, KeyringStore};

    let provider = parse_provider_id(provider_name)?;
    let store = KeyringStore::open()
        .map_err(|e| format!("OS keychain unavailable for {}: {e}", provider.as_str()))?;
    let removed = store
        .delete_api_key(provider)
        .map_err(|e| format!("could not remove key for {}: {e}", provider.as_str()))?;
    if removed {
        println!(
            "Removed stored API key for {} from {}.",
            provider.as_str(),
            store.name()
        );
    } else {
        println!(
            "No stored API key found for {} in {}.",
            provider.as_str(),
            store.name()
        );
    }
    Ok(())
}

/// `vitna skills list [--workspace <path>] [--json]`
pub fn run_skills_list(workspace: &Path, as_json: bool) -> Result<(), String> {
    use vitna_skills::SkillRegistry;

    let registry = SkillRegistry::discover_in_workspace(workspace)
        .map_err(|e| format!("failed to discover skills in {}: {e}", workspace.display()))?;

    if as_json {
        let out = json!({
            "workspace": workspace.display().to_string(),
            "skills": registry.skills().iter().map(|s| json!({
                "name": s.name,
                "description": s.description,
                "version": s.version,
                "source_path": s.source_path,
                "sha256_digest": s.sha256_digest,
            })).collect::<Vec<_>>(),
            "refused": registry.refused(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    println!("Vitna Workspace Skills ({})", workspace.display());
    println!("------------------------------------------");
    if registry.skills().is_empty() && registry.refused().is_empty() {
        println!("No workspace skills found under .vitna/skills.");
        return Ok(());
    }
    for skill in registry.skills() {
        let ver = skill
            .version
            .as_deref()
            .map(|v| format!(" v{v}"))
            .unwrap_or_default();
        println!(
            "  - {}{} ({}) [sha256:{}] - {}",
            skill.name,
            ver,
            skill.source_path,
            short_hash(&skill.sha256_digest),
            skill.description
        );
    }
    if !registry.refused().is_empty() {
        println!("Refused skill paths:");
        for r in registry.refused() {
            println!("  ! {}: {}", r.path, r.reason);
        }
    }
    Ok(())
}

/// `vitna skills verify [--workspace <path>] [--lock <file>] [--write-lock] [--json]`
pub fn run_skills_verify(
    workspace: &Path,
    lock_arg: Option<&Path>,
    write_lock: bool,
    as_json: bool,
) -> Result<(), String> {
    use vitna_skills::{SkillLock, SkillRegistry};

    let registry = SkillRegistry::discover_in_workspace(workspace)
        .map_err(|e| format!("failed to discover skills in {}: {e}", workspace.display()))?;

    if !registry.refused().is_empty() {
        let reasons: Vec<String> = registry
            .refused()
            .iter()
            .map(|r| format!("{}: {}", r.path, r.reason))
            .collect();
        return Err(format!(
            "skill discovery refused {} path(s): {}",
            reasons.len(),
            reasons.join("; ")
        ));
    }

    let lock_path = lock_arg
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join(".vitna").join("skills.lock.json"));

    if write_lock {
        let lock = SkillLock::from_manifests(registry.skills());
        if let Some(parent) = lock_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
            }
        }
        let body = serde_json::to_string_pretty(&lock).map_err(|e| e.to_string())?;
        fs::write(&lock_path, body)
            .map_err(|e| format!("could not write lockfile {}: {e}", lock_path.display()))?;

        if as_json {
            let out = json!({
                "action": "write_lock",
                "lock_path": lock_path.display().to_string(),
                "skills_locked": lock.entries.len(),
                "entries": lock.entries,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
            );
        } else {
            println!(
                "Wrote skill lockfile {} ({} skill(s) pinned).",
                lock_path.display(),
                lock.entries.len()
            );
        }
        return Ok(());
    }

    let raw = fs::read_to_string(&lock_path).map_err(|e| {
        format!(
            "could not read skill lockfile {} ({e}). Generate one with `vitna skills verify --write-lock`.",
            lock_path.display()
        )
    })?;
    let lock: SkillLock = serde_json::from_str(&raw)
        .map_err(|e| format!("invalid skill lockfile {}: {e}", lock_path.display()))?;

    lock.verify_all(registry.skills())
        .map_err(|e| format!("skill lock verification failed: {e}"))?;

    // Also fail closed if any pinned skill in the lockfile is missing from disk.
    for locked_name in lock.entries.keys() {
        if registry.get(locked_name).is_none() {
            return Err(format!(
                "skill lock verification failed: locked skill '{locked_name}' is missing from workspace"
            ));
        }
    }

    if as_json {
        let out = json!({
            "verified": true,
            "lock_path": lock_path.display().to_string(),
            "skills_verified": registry.skills().len(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    } else {
        println!(
            "Verified {} workspace skill(s) against {}.",
            registry.skills().len(),
            lock_path.display()
        );
    }
    Ok(())
}

fn default_min_os_for_target(target_triple: &str) -> &'static str {
    match target_triple {
        "x86_64-pc-windows-msvc" => "Windows 10 Build 19041+",
        "aarch64-pc-windows-msvc" => "Windows 11 Build 22000+",
        "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu" => {
            "Linux kernel 5.15+, glibc 2.31+"
        }
        "x86_64-apple-darwin" => "macOS Monterey 12.0+",
        "aarch64-apple-darwin" => "macOS Monterey 12.0+ (Apple Silicon)",
        _ => "unspecified",
    }
}

/// `vitna release sign --version <ver> --channel <ch> --asset <TRIPLE=PATH>... --out <file>`
pub fn run_release_sign(
    version: &str,
    channel: &str,
    release_notes_url: Option<&str>,
    asset_specs: &[String],
    out_path: &Path,
) -> Result<(), String> {
    use vitna_receipts::{
        ReleaseAsset, VitnaReleaseManifestV1, RELEASE_MANIFEST_SCHEMA_V1,
        SUPPORTED_RELEASE_CHANNELS,
    };

    if !SUPPORTED_RELEASE_CHANNELS.contains(&channel) {
        return Err(format!(
            "unsupported channel '{channel}': expected one of {}",
            SUPPORTED_RELEASE_CHANNELS.join(", ")
        ));
    }

    let mut assets = Vec::with_capacity(asset_specs.len());
    for spec in asset_specs {
        let Some((triple, raw_path)) = spec.split_once('=') else {
            return Err(format!(
                "invalid --asset '{spec}': expected 'TARGET_TRIPLE=ARCHIVE_PATH'"
            ));
        };
        let triple = triple.trim();
        let file_path = Path::new(raw_path.trim());
        let asset =
            ReleaseAsset::from_file(file_path, triple, default_min_os_for_target(triple))?;
        assets.push(asset);
    }

    let home = vitna_daemon::launch::vitna_home()?;
    fs::create_dir_all(&home)
        .map_err(|e| format!("could not create {}: {e}", home.display()))?;
    let signing_key = vitna_daemon::key::load_or_create(&home)?;

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let release_date = format!("unix:{now_secs}");
    let notes_url = release_notes_url
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!("https://github.com/convexityos/vitna-code/releases/tag/v{version}")
        });

    let mut manifest = VitnaReleaseManifestV1 {
        schema_version: RELEASE_MANIFEST_SCHEMA_V1.to_string(),
        version: version.to_string(),
        channel: channel.to_string(),
        release_date,
        release_notes_url: notes_url,
        assets,
        publisher_public_key: String::new(),
        publisher_signature: String::new(),
    };
    manifest.sign(&signing_key)?;

    let report = manifest.verify(Some(&manifest.publisher_public_key));
    if !report.is_valid {
        return Err(format!(
            "generated manifest failed self-verification: {}",
            report.errors.join("; ")
        ));
    }

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
    }
    let body = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    fs::write(out_path, body)
        .map_err(|e| format!("could not write {}: {e}", out_path.display()))?;

    println!(
        "Signed release manifest v{} ({} asset(s)) -> {}",
        manifest.version,
        manifest.assets.len(),
        out_path.display()
    );
    println!("Publisher Public Key: {}", manifest.publisher_public_key);
    Ok(())
}

/// `vitna release verify <manifest_file> [--key <hex>] [--assets-dir <dir>] [--json]`
pub fn run_release_verify(
    manifest_file: &Path,
    expected_key: Option<&str>,
    assets_dir: Option<&Path>,
    as_json: bool,
) -> Result<(), String> {
    use vitna_receipts::VitnaReleaseManifestV1;

    let raw = fs::read_to_string(manifest_file)
        .map_err(|e| format!("could not read {}: {e}", manifest_file.display()))?;
    let manifest: VitnaReleaseManifestV1 = serde_json::from_str(&raw)
        .map_err(|e| format!("could not parse {}: {e}", manifest_file.display()))?;

    let mut report = manifest.verify(expected_key);
    let mut disk_verified = Vec::new();
    if let Some(dir) = assets_dir {
        match manifest.verify_assets_in_dir(dir) {
            Ok(files) => disk_verified = files,
            Err(errs) => {
                report.is_valid = false;
                report.errors.extend(errs);
            }
        }
    }

    if as_json {
        let out = json!({
            "manifest": manifest_file.display().to_string(),
            "version": manifest.version,
            "channel": manifest.channel,
            "publisher_public_key": manifest.publisher_public_key,
            "report": report,
            "disk_assets_verified": disk_verified,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    } else {
        println!("Vitna Release Manifest Verification");
        println!("===================================");
        println!("Manifest:   {}", manifest_file.display());
        println!("Version:    {} ({})", manifest.version, manifest.channel);
        println!("Signature:  {}", if report.signature_verified { "VALID" } else { "INVALID" });
        println!("Assets:     {} declared", report.assets_checked);
        if !disk_verified.is_empty() {
            println!("On Disk:    {} verified by SHA-256 + byte size", disk_verified.len());
        }
        if !report.errors.is_empty() {
            println!("Errors:");
            for err in &report.errors {
                println!("  - {err}");
            }
        }
    }

    if !report.is_valid {
        return Err(format!(
            "release manifest verification failed ({} error(s))",
            report.errors.len()
        ));
    }
    Ok(())
}

pub struct MandateInitOptions<'a> {
    pub out: &'a Path,
    pub max_total: &'a str,
    pub max_per_txn: &'a str,
    pub currency: &'a str,
    pub effect: &'a str,
    pub aud: Vec<String>,
    pub resources: Vec<String>,
    pub ledger: &'a str,
    pub ttl_secs: u64,
    pub step_up_over: Option<&'a str>,
    pub tools: Vec<String>,
    pub as_json: bool,
}

fn parse_mandate_effect(raw: &str) -> Result<vitna_policy::mandate::Effect, String> {
    use vitna_policy::mandate::Effect;
    match raw.trim().to_ascii_lowercase().as_str() {
        "read" => Ok(Effect::Read),
        "write" => Ok(Effect::Write),
        "irreversible" => Ok(Effect::Irreversible),
        other => Err(format!(
            "invalid mandate effect '{other}': expected read, write, or irreversible"
        )),
    }
}

/// `vitna mandate init --out <file> ...`
pub fn run_mandate_init(opts: MandateInitOptions<'_>) -> Result<(), String> {
    use vitna_mcp::{
        LiveMandateSession, MandateBundleConfig, ToolCatalogEntry, ToolPriceSpec,
        MANDATE_CONFIG_SCHEMA_V1,
    };
    use vitna_policy::generate_policy_signing_key;
    use vitna_policy::mandate::{
        did_key_from_verifying_key, jwk_thumbprint_from_verifying_key, sign_mandate_jws, CnfClaim,
        MandateCaveats, MandateClaims, Money, StepUpCaveat, PROTOCOL_VERSION,
    };

    if opts.ttl_secs == 0 {
        return Err("--ttl-secs must be greater than 0".to_string());
    }
    let currency = opts.currency.trim().to_ascii_uppercase();
    if currency.is_empty() {
        return Err("--currency must be non-empty".to_string());
    }
    let max_effect = parse_mandate_effect(opts.effect)?;

    let mut pricing = Vec::new();
    for spec_str in &opts.tools {
        let parts: Vec<&str> = spec_str.split('|').collect();
        if parts.len() != 5 {
            return Err(format!(
                "invalid --tool '{spec_str}': expected SERVER|TOOL|RESOURCE_URI|AMOUNT|EFFECT"
            ));
        }
        let server = parts[0].trim();
        let tool = parts[1].trim();
        let resource_uri = parts[2].trim();
        let amount = parts[3].trim();
        let eff = parse_mandate_effect(parts[4])?;
        if server.is_empty() || tool.is_empty() || resource_uri.is_empty() || amount.is_empty() {
            return Err(format!(
                "invalid --tool '{spec_str}': server, tool, resource_uri, and amount must be non-empty"
            ));
        }
        pricing.push(ToolCatalogEntry {
            server: server.to_string(),
            tool: tool.to_string(),
            spec: ToolPriceSpec {
                resource_uri: resource_uri.to_string(),
                amount: Money::new(currency.clone(), amount.to_string()),
                effect: eff,
                purpose: None,
            },
        });
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let root_key = generate_policy_signing_key();
    let root_did = did_key_from_verifying_key(&root_key.verifying_key());
    let agent_key = generate_policy_signing_key();
    let agent_did = did_key_from_verifying_key(&agent_key.verifying_key());
    let agent_jkt = jwk_thumbprint_from_verifying_key(&agent_key.verifying_key());

    let jti = format!("mnd_{now}");
    let claims = MandateClaims {
        v: PROTOCOL_VERSION.to_string(),
        typ: "mandate".to_string(),
        jti: jti.clone(),
        iss: root_did.clone(),
        sub: agent_did.clone(),
        par: None,
        iat: now,
        nbf: now,
        exp: now.saturating_add(opts.ttl_secs),
        cnf: CnfClaim { jkt: agent_jkt },
        ledger: opts.ledger.to_string(),
        cav: MandateCaveats {
            max_total: Some(Money::new(currency.clone(), opts.max_total.to_string())),
            max_per_txn: Some(Money::new(currency.clone(), opts.max_per_txn.to_string())),
            aud: if opts.aud.is_empty() {
                None
            } else {
                Some(opts.aud)
            },
            resources: if opts.resources.is_empty() {
                None
            } else {
                Some(opts.resources)
            },
            effect: Some(max_effect),
            currency: Some(currency.clone()),
            step_up: opts.step_up_over.map(|over| StepUpCaveat {
                over: Money::new(currency.clone(), over.to_string()),
            }),
            ..Default::default()
        },
        st: None,
    };
    let token = sign_mandate_jws(&claims, &root_key, None);

    let bundle = MandateBundleConfig {
        schema_version: MANDATE_CONFIG_SCHEMA_V1.to_string(),
        tokens: vec![token],
        trusted_root_issuers: vec![root_did.clone()],
        ledger: opts.ledger.to_string(),
        currency: currency.clone(),
        pricing,
        task_id: None,
        step_up_approved: false,
    };

    // Self-verify eagerly before writing to disk so invalid monetary strings or
    // caveat combinations fail closed without creating a broken file.
    let session = LiveMandateSession::from_bundle(bundle.clone())?;

    if let Some(parent) = opts.out.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
    }
    let body = serde_json::to_string_pretty(&bundle).map_err(|e| e.to_string())?;
    fs::write(opts.out, body)
        .map_err(|e| format!("could not write {}: {e}", opts.out.display()))?;

    let chain = session.verified_chain();
    if opts.as_json {
        let out = json!({
            "file": opts.out.display().to_string(),
            "root_jti": chain.root_jti,
            "principal": root_did,
            "agent": agent_did,
            "chain_hash": chain.chain_hash,
            "currency": currency,
            "priced_tools": session.pricing_entries().len(),
            "exp": chain.exp,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    } else {
        println!(
            "Wrote signed am/1 mandate bundle -> {}",
            opts.out.display()
        );
        println!("Root JTI:     {}", chain.root_jti);
        println!("Principal:    {root_did}");
        println!("Agent:        {agent_did}");
        println!("Chain Hash:   {}", chain.chain_hash);
        println!("Priced Tools: {}", session.pricing_entries().len());
    }
    Ok(())
}

/// `vitna mandate inspect <mandate_file> [--json]`
pub fn run_mandate_inspect(mandate_file: &Path, as_json: bool) -> Result<(), String> {
    let session = vitna_mcp::LiveMandateSession::from_file(mandate_file)?;
    let chain = session.verified_chain();
    let entries = session.pricing_entries();

    let pricing_json: Vec<Value> = entries
        .iter()
        .map(|entry| {
            json!({
                "server": entry.server,
                "tool": entry.tool,
                "resource_uri": entry.spec.resource_uri,
                "amount": entry.spec.amount,
                "effect": entry.spec.effect.as_str(),
                "purpose": entry.spec.purpose,
            })
        })
        .collect();

    if as_json {
        let out = json!({
            "file": mandate_file.display().to_string(),
            "valid": true,
            "root_jti": chain.root_jti,
            "path": chain.path,
            "principal": chain.principal,
            "agent": chain.agent,
            "ledger": chain.ledger,
            "currency": session.currency(),
            "chain_hash": chain.chain_hash,
            "nbf": chain.nbf,
            "exp": chain.exp,
            "task_id": session.task_id(),
            "step_up_approved": session.step_up_approved(),
            "effective_caveats": chain.effective,
            "pricing": pricing_json,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    println!("Vitna Agent Mandate (am/1) Inspection");
    println!("=====================================");
    println!("File:       {}", mandate_file.display());
    println!("Status:     VERIFIED");
    println!("Root JTI:   {}", chain.root_jti);
    println!("Path:       {}", chain.path.join(" -> "));
    println!("Principal:  {}", chain.principal);
    println!("Agent:      {}", chain.agent);
    println!("Ledger:     {}", chain.ledger);
    println!("Currency:   {}", session.currency());
    println!("Chain Hash: {}", chain.chain_hash);
    println!("Valid Window: nbf={} exp={}", chain.nbf, chain.exp);
    if let Some(max_total) = &chain.effective.max_total {
        println!("Max Total:  {} {}", max_total.amt, max_total.cur);
    }
    if let Some(max_per_txn) = &chain.effective.max_per_txn {
        println!("Max / Txn:  {} {}", max_per_txn.amt, max_per_txn.cur);
    }
    if let Some(effect) = chain.effective.effect {
        println!("Effect Cap: {}", effect.as_str());
    }
    if let Some(aud) = &chain.effective.aud {
        println!("Audience:   {}", aud.join(", "));
    }
    if let Some(resources) = &chain.effective.resources {
        println!("Resources:  {}", resources.join(", "));
    }
    if let Some(step_up) = &chain.effective.step_up {
        println!(
            "Step-Up:    over {} {} (approved: {})",
            step_up.over.amt,
            step_up.over.cur,
            session.step_up_approved()
        );
    }
    println!("Priced MCP Tools ({}):", entries.len());
    for entry in &entries {
        println!(
            "  - {}:{} -> {} ({} {}, effect: {})",
            entry.server,
            entry.tool,
            entry.spec.resource_uri,
            entry.spec.amount.amt,
            entry.spec.amount.cur,
            entry.spec.effect.as_str()
        );
    }
    Ok(())
}

/// `vitna mandate verify <mandate_file> [--server <name> --tool <tool>] [--step-up-approved] [--json]`
pub fn run_mandate_verify(
    mandate_file: &Path,
    server: Option<&str>,
    tool: Option<&str>,
    step_up_approved: bool,
    as_json: bool,
) -> Result<(), String> {
    if server.is_some() ^ tool.is_some() {
        return Err("pass both --server and --tool together to dry-run a tool action".to_string());
    }

    let session = vitna_mcp::LiveMandateSession::from_file(mandate_file)?;
    let chain = session.verified_chain();

    let dry_run = if let (Some(srv), Some(tl)) = (server, tool) {
        let override_step_up = if step_up_approved { Some(true) } else { None };
        Some((srv, tl, session.dry_run_tool(srv, tl, override_step_up)))
    } else {
        None
    };

    let (action_allowed, action_json, action_err) = match &dry_run {
        Some((srv, tl, Ok(spec))) => (
            true,
            Some(json!({
                "server": srv,
                "tool": tl,
                "allowed": true,
                "resource_uri": spec.resource_uri,
                "amount": spec.amount,
                "effect": spec.effect.as_str(),
            })),
            None,
        ),
        Some((srv, tl, Err(reason))) => (
            false,
            Some(json!({
                "server": srv,
                "tool": tl,
                "allowed": false,
                "reason": reason,
            })),
            Some(reason.clone()),
        ),
        None => (true, None, None),
    };

    if as_json {
        let out = json!({
            "file": mandate_file.display().to_string(),
            "chain_verified": true,
            "root_jti": chain.root_jti,
            "path": chain.path,
            "chain_hash": chain.chain_hash,
            "priced_tools": session.pricing_entries().len(),
            "action_check": action_json,
            "valid": action_allowed,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    } else {
        println!("Vitna Agent Mandate (am/1) Verification");
        println!("=======================================");
        println!("File:       {}", mandate_file.display());
        println!("Chain:      VALID ({} hop(s), root {})", chain.path.len(), chain.root_jti);
        println!("Chain Hash: {}", chain.chain_hash);
        if let Some((srv, tl, res)) = &dry_run {
            match res {
                Ok(spec) => println!(
                    "Action:     ALLOWED ({}:{} -> {} | {} {} | {})",
                    srv,
                    tl,
                    spec.resource_uri,
                    spec.amount.amt,
                    spec.amount.cur,
                    spec.effect.as_str()
                ),
                Err(reason) => println!("Action:     REFUSED ({}:{} -> {reason})", srv, tl),
            }
        }
    }

    if let Some(reason) = action_err {
        return Err(reason);
    }
    Ok(())
}

/// `vitna doctor [--workspace <dir>] [--sandbox-image <img_ref>] [--mandate <file>] [--json]`
pub async fn run_doctor_audit(
    workspace: &Path,
    sandbox_image: Option<&str>,
    mandate_file: Option<&Path>,
    as_json: bool,
) -> Result<(), String> {
    use vitna_providers::{CredentialError, CredentialResolver, KeyringStore, ProviderId};

    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let git_version = vitna_git_workspaces::host_git::version();

    let temp_db = std::env::temp_dir().join(format!(
        "vitna_doctor_check_{}.db",
        std::process::id()
    ));
    let sqlite_ok = vitna_store::EventStore::open(&temp_db).is_ok();
    let _ = fs::remove_file(&temp_db);

    let (device_key_status, device_public_key, device_key_dir) =
        match vitna_daemon::launch::vitna_home() {
            Ok(home) => match vitna_daemon::key::public_key_in(&home) {
                Ok(Some(pub_key)) => (
                    "present".to_string(),
                    Some(pub_key),
                    Some(home.display().to_string()),
                ),
                Ok(None) => (
                    "uninitialized".to_string(),
                    None,
                    Some(home.display().to_string()),
                ),
                Err(e) => (format!("error: {e}"), None, Some(home.display().to_string())),
            },
            Err(e) => (format!("error: {e}"), None, None),
        };

    let probe_dir = if workspace.exists() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir())
    };

    let guarded_sandbox = vitna_runner::sandbox_status_for(
        &probe_dir,
        vitna_runner::SandboxGuarantee::Guarded,
        None,
        false,
    );

    let probe_image = sandbox_image.unwrap_or("alpine:3.20");
    let strong_sandbox = vitna_runner::sandbox_status_for(
        &probe_dir,
        vitna_runner::SandboxGuarantee::Strong,
        Some(probe_image),
        false,
    );

    let (daemon_listening, daemon_status) = probe_daemon_endpoint().await;

    let (keychain_backend, provider_keys) = match KeyringStore::open() {
        Ok(store) => {
            let resolver = CredentialResolver::new(Arc::new(store));
            let name = resolver.source_name().to_string();
            let mut rows = Vec::new();
            for p in ProviderId::ALL {
                let status = match resolver.check(p) {
                    Ok(()) => "configured",
                    Err(CredentialError::AuthenticationRequired { .. }) => "not_configured",
                    Err(_) => "unavailable",
                };
                rows.push(json!({ "provider": p.as_str(), "status": status }));
            }
            (name, rows)
        }
        Err(_) => (
            "unavailable".to_string(),
            ProviderId::ALL
                .into_iter()
                .map(|p| json!({ "provider": p.as_str(), "status": "unavailable" }))
                .collect(),
        ),
    };

    let skills_info = if workspace.exists() {
        match vitna_skills::SkillRegistry::discover_in_workspace(workspace) {
            Ok(reg) => {
                let lock_path = workspace.join(".vitna").join("skills.lock.json");
                let lock_status = if lock_path.is_file() {
                    let ok = fs::read_to_string(&lock_path)
                        .ok()
                        .and_then(|raw| serde_json::from_str::<vitna_skills::SkillLock>(&raw).ok())
                        .is_some_and(|lock| {
                            lock.verify_all(reg.skills()).is_ok()
                                && lock.entries.keys().all(|name| reg.get(name).is_some())
                        });
                    if ok {
                        "verified"
                    } else {
                        "mismatch"
                    }
                } else {
                    "no_lockfile"
                };
                json!({
                    "discovered": reg.skills().len(),
                    "refused": reg.refused().len(),
                    "lockfile": lock_status,
                })
            }
            Err(e) => json!({ "error": e.to_string() }),
        }
    } else {
        json!({ "error": "workspace path does not exist" })
    };

    let mandate_check = mandate_file.map(|path| match vitna_mcp::LiveMandateSession::from_file(path) {
        Ok(s) => Ok((
            s.root_jti().to_string(),
            s.chain_hash().to_string(),
            s.pricing_entries().len(),
        )),
        Err(e) => Err(e),
    });

    if as_json {
        let out = json!({
            "os": os,
            "arch": arch,
            "git": git_version,
            "sqlite_wal": sqlite_ok,
            "device_key": {
                "status": device_key_status,
                "public_key": device_public_key,
                "dir": device_key_dir,
            },
            "sandbox_guarded": match &guarded_sandbox {
                Ok((backend, enforcement)) => json!({ "available": true, "backend": backend, "enforcement": enforcement }),
                Err(reason) => json!({ "available": false, "reason": reason }),
            },
            "sandbox_strong_oci": match &strong_sandbox {
                Ok((backend, enforcement)) => json!({ "available": true, "backend": backend, "enforcement": enforcement, "probe_image": probe_image }),
                Err(reason) => json!({ "available": false, "reason": reason, "probe_image": probe_image }),
            },
            "keychain": {
                "backend": keychain_backend,
                "providers": provider_keys,
            },
            "skills": skills_info,
            "daemon": {
                "listening": daemon_listening,
                "status": daemon_status,
            },
            "mandate": match &mandate_check {
                Some(Ok((root_jti, chain_hash, priced))) => json!({
                    "valid": true,
                    "root_jti": root_jti,
                    "chain_hash": chain_hash,
                    "priced_tools": priced,
                }),
                Some(Err(reason)) => json!({
                    "valid": false,
                    "error": reason,
                }),
                None => Value::Null,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?
        );
    } else {
        println!("Vitna Code Doctor - Environment Audit");
        println!("=====================================");
        println!("[OK] OS: {os} ({arch})");
        match &git_version {
            Some(version) => println!("[OK] Git: {version}"),
            None => println!("[WARN] Git executable not found on PATH."),
        }
        if sqlite_ok {
            println!("[OK] SQLite WAL Persistence: Operational");
        } else {
            println!("[FAIL] SQLite WAL Persistence: unavailable");
        }
        match (&device_key_status[..], &device_public_key, &device_key_dir) {
            ("present", Some(pub_key), Some(dir)) => {
                println!("[OK] Device Signing Key: Ed25519, in {dir}");
                println!("     Device Public Key: {pub_key}");
            }
            ("uninitialized", _, Some(dir)) => {
                println!("[OK] Device Signing Key: none yet. The daemon makes one in {dir} the first time it signs.");
            }
            _ => println!("[FAIL] Device Signing Key: {device_key_status}"),
        }
        match &guarded_sandbox {
            Ok((backend, enforcement)) => {
                println!("[OK] Guarded Sandbox Backend: {backend} ({enforcement})");
            }
            Err(reason) => {
                println!("[WARN] Guarded Sandbox Backend: unavailable ({reason})");
            }
        }
        match &strong_sandbox {
            Ok((backend, enforcement)) => {
                println!("[OK] Strong OCI Sandbox: {backend} ({enforcement}, probe image: {probe_image})");
            }
            Err(reason) => {
                println!("[INFO] Strong OCI Sandbox: unavailable ({reason})");
            }
        }
        let key_summary: Vec<String> = provider_keys
            .iter()
            .filter_map(|row| {
                Some(format!(
                    "{}={}",
                    row.get("provider")?.as_str()?,
                    row.get("status")?.as_str()?
                ))
            })
            .collect();
        println!(
            "[INFO] OS Keychain ({keychain_backend}): {}",
            key_summary.join(", ")
        );
        if let Some(count) = skills_info.get("discovered").and_then(|v| v.as_u64()) {
            let refused = skills_info
                .get("refused")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let lock = skills_info
                .get("lockfile")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            println!(
                "[OK] Workspace Skills: {count} discovered, {refused} refused (lockfile: {lock})"
            );
        }
        println!("{daemon_status}");
        if let Some(res) = &mandate_check {
            match res {
                Ok((root_jti, chain_hash, priced)) => {
                    println!(
                        "[OK] Mandate (am/1): verified (root {root_jti}, {priced} priced tool(s), {chain_hash})"
                    );
                }
                Err(reason) => {
                    println!("[FAIL] Mandate (am/1): {reason}");
                }
            }
        }
        println!("\nDoctor audit complete.");
    }

    if let Some(Err(reason)) = mandate_check {
        return Err(format!("mandate audit failed: {reason}"));
    }
    Ok(())
}

/// `vitna bench [--fast] [--out <file>] [--json]`
pub fn run_bench(fast: bool, out_path: Option<&Path>, as_json: bool) -> Result<(), String> {
    let report = if fast {
        vitna_evals::BenchmarkSuite::run_with_iterations(25, 10, 25, 25)
    } else {
        vitna_evals::BenchmarkSuite::run_all()
    };

    let json_body = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
    if let Some(path) = out_path {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
            }
        }
        fs::write(path, &json_body)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }

    if as_json {
        println!("{json_body}");
        return Ok(());
    }

    println!("Vitna Code Live Benchmark Suite ({})", report.suite_version);
    println!("======================================================");
    println!(
        "Host:      {} ({}, profile: {})",
        report.os, report.arch, report.build_profile
    );
    println!("Timestamp: {}", report.timestamp_rfc3339);
    for m in &report.metrics {
        println!(
            "  - {:<36} {:>6} ops | {:>10.1} ops/s | {:>8.1} us/op",
            m.name, m.operations, m.ops_per_sec, m.avg_latency_micros
        );
    }
    if let Some(path) = out_path {
        println!("Report written -> {}", path.display());
    }
    Ok(())
}




