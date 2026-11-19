//! End-to-end CLI subcommand tests for `vitna`.
//!
//! All git invocations go through `vitna_git_workspaces::host_git::command`
//! to preserve the workspace-wide no-bare-git invariant.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use sha2::{Digest, Sha256};
use vitna_git_broker::NULL_HASH;
use vitna_receipts::{
    generate_signing_key, ChangeSetRecord, EvidenceItemRecord, FileModificationRecord,
    ModelSelectionRecord, VitnaRunReceiptV1,
};

static COUNTER: AtomicU64 = AtomicU64::new(1);

fn temp_dir(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("vitna_cli_test_{tag}_{}_{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn vitna_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_vitna"))
}

fn run_vitna(args: &[&str], home: &Path, cwd: &Path) -> Output {
    Command::new(vitna_bin())
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .output()
        .expect("spawn vitna binary")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_signed_receipt(
    workspace: &Path,
    run_id: &str,
    files_modified: Vec<FileModificationRecord>,
    evidence_items: Vec<EvidenceItemRecord>,
) -> (VitnaRunReceiptV1, String) {
    let key = generate_signing_key();
    let pub_hex = hex::encode(key.verifying_key().to_bytes());
    let mut receipt = VitnaRunReceiptV1 {
        schema_version: "vitna-run-receipt-v1".to_string(),
        run_id: run_id.to_string(),
        session_id: "sess-cli-test".to_string(),
        workspace_fingerprint: "ws-secret-fingerprint".to_string(),
        base_commit_sha: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
        model_selection: ModelSelectionRecord {
            provider: "vitna-runtime".to_string(),
            model_sku: "vitna/code".to_string(),
            routing_reason: "runtime_lane:vitna/code".to_string(),
            policy_digest: None,
        },
        event_hash_chain_root: "0".repeat(64),
        isolation_label: "guarded".to_string(),
        completion_state: "completed_with_evidence".to_string(),
        evidence_items,
        changeset: ChangeSetRecord {
            files_modified,
            diff_digest: "b".repeat(64),
        },
        runner_execution_statements: Vec::new(),
        child_receipt_roots: Vec::new(),
        device_public_key: None,
        device_signature: String::new(),
    };
    receipt.sign(&key).expect("sign receipt");

    let receipts_dir = workspace.join(".vitna").join("receipts");
    fs::create_dir_all(&receipts_dir).expect("create receipts dir");
    fs::write(
        receipts_dir.join(format!("{run_id}.json")),
        serde_json::to_string_pretty(&receipt).unwrap(),
    )
    .expect("write receipt");
    (receipt, pub_hex)
}

#[test]
fn doctor_reports_sandbox_and_daemon_probes() {
    let base = temp_dir("doctor");
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();

    let out = run_vitna(&["doctor"], &home, &base);
    assert!(
        out.status.success(),
        "doctor failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("OS:"), "{stdout}");
    assert!(stdout.contains("SQLite WAL Persistence:"), "{stdout}");
    assert!(stdout.contains("Device Signing Key:"), "{stdout}");
    assert!(stdout.contains("Sandbox Backend:"), "{stdout}");
    assert!(stdout.contains("Daemon"), "{stdout}");

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn run_sessions_and_resume_persist_and_replay() {
    let base = temp_dir("sessions_resume");
    let home = base.join("home");
    let ws = base.join("ws");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&ws).unwrap();
    let store = ws.join(".vitna").join("store.db");

    // Run an offline task that creates vitna_output.txt and persists session/run.
    let run_out = run_vitna(
        &[
            "run",
            "create hello world note",
            "--workspace",
            ws.to_str().unwrap(),
            "--store",
            store.to_str().unwrap(),
            "--auto-approve",
        ],
        &home,
        &ws,
    );
    assert!(
        run_out.status.success(),
        "run failed: {}",
        String::from_utf8_lossy(&run_out.stderr)
    );

    // List persisted sessions as JSON.
    let sess_out = run_vitna(
        &["sessions", "--store", store.to_str().unwrap(), "--json"],
        &home,
        &ws,
    );
    assert!(
        sess_out.status.success(),
        "sessions failed: {}",
        String::from_utf8_lossy(&sess_out.stderr)
    );
    let sessions: Value =
        serde_json::from_slice(&sess_out.stdout).expect("sessions JSON output");
    let arr = sessions.as_array().expect("sessions array");
    assert_eq!(arr.len(), 1);
    let session_id = arr[0]["session_id"].as_str().unwrap().to_string();
    assert_eq!(arr[0]["run_count"].as_u64(), Some(1));

    // Resume the recorded session and verify replayed run metadata.
    let resume_out = run_vitna(
        &[
            "resume",
            &session_id,
            "--store",
            store.to_str().unwrap(),
            "--json",
        ],
        &home,
        &ws,
    );
    assert!(
        resume_out.status.success(),
        "resume failed: {}",
        String::from_utf8_lossy(&resume_out.stderr)
    );
    let resumed: Value =
        serde_json::from_slice(&resume_out.stdout).expect("resume JSON output");
    assert_eq!(resumed["session_id"].as_str(), Some(session_id.as_str()));
    assert!(resumed["latest_run"]["total_events"].as_u64().unwrap_or(0) > 0);

    // Passing --runtime-url without --lane must fail cleanly.
    let bad_run = run_vitna(
        &[
            "run",
            "check",
            "--workspace",
            ws.to_str().unwrap(),
            "--runtime-url",
            "http://127.0.0.1:8787",
        ],
        &home,
        &ws,
    );
    assert!(!bad_run.status.success());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn receipt_show_and_export_respect_redaction_and_traversal_guards() {
    let base = temp_dir("receipt_ops");
    let home = base.join("home");
    let ws = base.join("ws");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&ws).unwrap();

    let (_receipt, pub_hex) = write_signed_receipt(
        &ws,
        "run-exp-1",
        vec![FileModificationRecord {
            path: "secret_dir/internal.rs".to_string(),
            preimage_hash: NULL_HASH.to_string(),
            postimage_hash: "c".repeat(64),
        }],
        vec![EvidenceItemRecord {
            evidence_id: "ev-1".to_string(),
            grade: "sandbox_captured".to_string(),
            description: "cargo test passed in /private/path".to_string(),
            artifact_digest: Some("d".repeat(64)),
        }],
    );

    // 1. receipt show with explicit --key succeeds and reports verified.
    let show = run_vitna(
        &[
            "receipt",
            "show",
            "run-exp-1",
            "--workspace",
            ws.to_str().unwrap(),
            "--key",
            &pub_hex,
            "--json",
        ],
        &home,
        &ws,
    );
    assert!(
        show.status.success(),
        "receipt show failed: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let shown: Value = serde_json::from_slice(&show.stdout).unwrap();
    assert_eq!(shown["is_valid"].as_bool(), Some(true));
    assert_eq!(shown["receipt"]["run_id"].as_str(), Some("run-exp-1"));

    // 2. Traversal run_id is refused.
    let evil = run_vitna(
        &[
            "receipt",
            "show",
            "../evil",
            "--workspace",
            ws.to_str().unwrap(),
        ],
        &home,
        &ws,
    );
    assert!(!evil.status.success());

    // 3. export --redaction none produces a verbatim receipt that passes `vitna verify --key`.
    let raw_out_file = base.join("exported-raw.json");
    let exp_none = run_vitna(
        &[
            "receipt",
            "export",
            "run-exp-1",
            "--workspace",
            ws.to_str().unwrap(),
            "--redaction",
            "none",
            "--out",
            raw_out_file.to_str().unwrap(),
        ],
        &home,
        &ws,
    );
    assert!(
        exp_none.status.success(),
        "export none failed: {}",
        String::from_utf8_lossy(&exp_none.stderr)
    );
    let verified = run_vitna(
        &["verify", raw_out_file.to_str().unwrap(), "--key", &pub_hex],
        &home,
        &ws,
    );
    assert!(
        verified.status.success(),
        "verify exported raw receipt failed: {}",
        String::from_utf8_lossy(&verified.stderr)
    );

    // 4. export --redaction standard strips workspace fingerprint, directory prefixes, and signature.
    let exp_std = run_vitna(
        &[
            "receipt",
            "export",
            "run-exp-1",
            "--workspace",
            ws.to_str().unwrap(),
            "--redaction",
            "standard",
        ],
        &home,
        &ws,
    );
    assert!(exp_std.status.success());
    let std_val: Value = serde_json::from_slice(&exp_std.stdout).unwrap();
    assert_eq!(std_val["redaction"].as_str(), Some("standard"));
    assert_eq!(std_val["signature_stripped"].as_bool(), Some(true));
    assert_eq!(
        std_val["receipt"]["workspace_fingerprint"].as_str(),
        Some("[REDACTED]")
    );
    assert_eq!(
        std_val["receipt"]["changeset"]["files_modified"][0]["path"].as_str(),
        Some("<redacted>/internal.rs")
    );
    assert_eq!(std_val["receipt"]["device_signature"].as_str(), Some(""));
    assert!(std_val["receipt"].get("device_public_key").is_none());

    // 5. export --redaction strict also redacts filenames and evidence descriptions.
    let exp_strict = run_vitna(
        &[
            "receipt",
            "export",
            "run-exp-1",
            "--workspace",
            ws.to_str().unwrap(),
            "--redaction",
            "strict",
        ],
        &home,
        &ws,
    );
    assert!(exp_strict.status.success());
    let strict_val: Value = serde_json::from_slice(&exp_strict.stdout).unwrap();
    assert_eq!(strict_val["redaction"].as_str(), Some("strict"));
    assert_eq!(
        strict_val["receipt"]["changeset"]["files_modified"][0]["path"].as_str(),
        Some("redacted-file-0")
    );
    assert_eq!(
        strict_val["receipt"]["evidence_items"][0]["description"].as_str(),
        Some("[REDACTED]")
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn review_correlates_git_changes_with_receipts() {
    let base = temp_dir("review");
    let home = base.join("home");
    let ws = base.join("repo");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(ws.join("src")).unwrap();

    // Initialize git repo using hardened host_git::command.
    let init = vitna_git_workspaces::host_git::command(&ws)
        .unwrap()
        .args(["init", "-q"])
        .status()
        .expect("git init");
    assert!(init.success());

    fs::write(ws.join("src/attested.rs"), "pub fn v1() {}\n").unwrap();
    fs::write(ws.join("src/unattested.rs"), "pub fn unattested() {}\n").unwrap();

    let post_hash = sha256_hex(b"pub fn v1() {}\n");
    write_signed_receipt(
        &ws,
        "run-rev-1",
        vec![FileModificationRecord {
            path: "src/attested.rs".to_string(),
            preimage_hash: NULL_HASH.to_string(),
            postimage_hash: post_hash,
        }],
        vec![EvidenceItemRecord {
            evidence_id: "ev-1".to_string(),
            grade: "sandbox_captured".to_string(),
            description: "unit tests passed".to_string(),
            artifact_digest: None,
        }],
    );

    let review = run_vitna(
        &["review", "--workspace", ws.to_str().unwrap(), "--json"],
        &home,
        &ws,
    );
    assert!(
        review.status.success(),
        "review failed: {}",
        String::from_utf8_lossy(&review.stderr)
    );
    let parsed: Value = serde_json::from_slice(&review.stdout).unwrap();
    assert_eq!(parsed["is_git_repo"].as_bool(), Some(true));
    let files = parsed["changed_files"].as_array().unwrap();

    let attested = files
        .iter()
        .find(|f| f["path"].as_str() == Some("src/attested.rs"))
        .expect("attested file listed");
    assert_eq!(attested["attested_by_receipt"].as_bool(), Some(true));

    let unattested = files
        .iter()
        .find(|f| f["path"].as_str() == Some("src/unattested.rs"))
        .expect("unattested file listed");
    assert_eq!(unattested["attested_by_receipt"].as_bool(), Some(false));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn apply_enforces_preimages_and_applies_verified_postimages() {
    let base = temp_dir("apply");
    let home = base.join("home");
    let target = base.join("target");
    let source = base.join("source");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(target.join("src")).unwrap();
    fs::create_dir_all(source.join("src")).unwrap();

    let before = b"fn answer() -> i32 { 41 }\n";
    let after = b"fn answer() -> i32 { 42 }\n";
    fs::write(target.join("src/lib.rs"), before).unwrap();
    fs::write(source.join("src/lib.rs"), after).unwrap();

    write_signed_receipt(
        &target,
        "run-apply-1",
        vec![FileModificationRecord {
            path: "src/lib.rs".to_string(),
            preimage_hash: sha256_hex(before),
            postimage_hash: sha256_hex(after),
        }],
        vec![],
    );

    // 1. --check succeeds without modifying disk.
    let check = run_vitna(
        &[
            "apply",
            "run-apply-1",
            "--workspace",
            target.to_str().unwrap(),
            "--check",
        ],
        &home,
        &target,
    );
    assert!(
        check.status.success(),
        "apply --check failed: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert_eq!(fs::read(target.join("src/lib.rs")).unwrap(), before);

    // 2. Concurrent modification causes conflict refusal.
    fs::write(target.join("src/lib.rs"), b"fn answer() -> i32 { 999 }\n").unwrap();
    let conflict = run_vitna(
        &[
            "apply",
            "run-apply-1",
            "--workspace",
            target.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
        ],
        &home,
        &target,
    );
    assert!(!conflict.status.success());
    assert!(
        String::from_utf8_lossy(&conflict.stderr).contains("preimage mismatch"),
        "{}",
        String::from_utf8_lossy(&conflict.stderr)
    );

    // 3. Restore preimage and apply cleanly from --source.
    fs::write(target.join("src/lib.rs"), before).unwrap();
    let applied = run_vitna(
        &[
            "apply",
            "run-apply-1",
            "--workspace",
            target.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
        ],
        &home,
        &target,
    );
    assert!(
        applied.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&applied.stderr)
    );
    assert_eq!(fs::read(target.join("src/lib.rs")).unwrap(), after);

    let _ = fs::remove_dir_all(&base);
}
