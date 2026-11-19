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

#[test]
fn skills_list_and_verify_round_trip_and_tamper_detection() {
    let base = temp_dir("skills_cli");
    let home = base.join("home");
    let ws = base.join("ws");
    let skill_dir = ws.join(".vitna").join("skills").join("audit");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&skill_dir).unwrap();

    let skill_path = skill_dir.join("SKILL.md");
    let original = "---\nname: audit\ndescription: Review diffs for security regressions\nversion: 1.0.0\n---\nCheck every file modification preimage.\n";
    fs::write(&skill_path, original).unwrap();

    // 1. `vitna skills list --json` discovers the valid skill.
    let list_out = run_vitna(
        &["skills", "list", "--workspace", ws.to_str().unwrap(), "--json"],
        &home,
        &ws,
    );
    assert!(
        list_out.status.success(),
        "skills list failed: {}",
        String::from_utf8_lossy(&list_out.stderr)
    );
    let listed: Value = serde_json::from_slice(&list_out.stdout).expect("skills list json");
    let skills = listed["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0]["name"].as_str(), Some("audit"));
    assert_eq!(skills[0]["version"].as_str(), Some("1.0.0"));
    assert!(listed["refused"].as_array().unwrap().is_empty());

    // 2. `vitna skills verify --write-lock` writes `.vitna/skills.lock.json`.
    let write_lock = run_vitna(
        &[
            "skills",
            "verify",
            "--workspace",
            ws.to_str().unwrap(),
            "--write-lock",
            "--json",
        ],
        &home,
        &ws,
    );
    assert!(
        write_lock.status.success(),
        "skills verify --write-lock failed: {}",
        String::from_utf8_lossy(&write_lock.stderr)
    );
    let lock_json: Value = serde_json::from_slice(&write_lock.stdout).unwrap();
    assert_eq!(lock_json["skills_locked"].as_u64(), Some(1));
    assert!(ws.join(".vitna/skills.lock.json").exists());

    // 3. `vitna skills verify` passes against the written lockfile.
    let verified = run_vitna(
        &[
            "skills",
            "verify",
            "--workspace",
            ws.to_str().unwrap(),
            "--json",
        ],
        &home,
        &ws,
    );
    assert!(
        verified.status.success(),
        "skills verify failed: {}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let verified_json: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verified_json["verified"].as_bool(), Some(true));

    // 4. Tampering with the skill file causes fail-closed digest mismatch.
    fs::write(
        &skill_path,
        "---\nname: audit\ndescription: Review diffs for security regressions\n---\nTAMPERED\n",
    )
    .unwrap();
    let tampered = run_vitna(
        &["skills", "verify", "--workspace", ws.to_str().unwrap()],
        &home,
        &ws,
    );
    assert!(!tampered.status.success());
    let stderr = String::from_utf8_lossy(&tampered.stderr);
    assert!(
        stderr.contains("digest mismatch"),
        "expected digest mismatch in stderr, got: {stderr}"
    );

    // 5. Restore valid skill and add a malformed skill; list reports refused and verify fails closed.
    fs::write(&skill_path, original).unwrap();
    let broken_dir = ws.join(".vitna").join("skills").join("broken");
    fs::create_dir_all(&broken_dir).unwrap();
    fs::write(broken_dir.join("SKILL.md"), "missing frontmatter").unwrap();

    let list_with_broken = run_vitna(
        &["skills", "list", "--workspace", ws.to_str().unwrap(), "--json"],
        &home,
        &ws,
    );
    assert!(list_with_broken.status.success());
    let broken_json: Value = serde_json::from_slice(&list_with_broken.stdout).unwrap();
    assert_eq!(broken_json["refused"].as_array().unwrap().len(), 1);

    let verify_with_broken = run_vitna(
        &["skills", "verify", "--workspace", ws.to_str().unwrap()],
        &home,
        &ws,
    );
    assert!(!verify_with_broken.status.success());
    assert!(String::from_utf8_lossy(&verify_with_broken.stderr).contains("refused"));

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn auth_and_serve_flag_validation_fail_closed() {
    use std::process::Stdio;

    let base = temp_dir("auth_serve_flags");
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();

    // 1. `vitna auth status --json` always reports ambient_env_scanning: false.
    let status_out = Command::new(vitna_bin())
        .args(["auth", "status", "--json"])
        .current_dir(&base)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .output()
        .expect("spawn vitna auth status");
    assert!(
        status_out.status.success(),
        "auth status failed: {}",
        String::from_utf8_lossy(&status_out.stderr)
    );
    let status_json: Value = serde_json::from_slice(&status_out.stdout).unwrap();
    assert_eq!(status_json["ambient_env_scanning"].as_bool(), Some(false));
    assert_eq!(status_json["providers"].as_array().unwrap().len(), 2);

    // 2. Unknown provider is refused before touching keychain or stdin.
    let bad_provider = run_vitna(&["auth", "set", "bogus-provider"], &home, &base);
    assert!(!bad_provider.status.success());
    assert!(String::from_utf8_lossy(&bad_provider.stderr).contains("unknown provider"));

    // 3. Empty stdin to `vitna auth set anthropic` is refused cleanly.
    let mut child = Command::new(vitna_bin())
        .args(["auth", "set", "anthropic"])
        .current_dir(&base)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn vitna auth set");
    drop(child.stdin.take());
    let empty_out = child.wait_with_output().expect("wait vitna auth set");
    assert!(!empty_out.status.success());
    assert!(String::from_utf8_lossy(&empty_out.stderr).contains("invalid API key"));

    // 4. `vitna serve --provider anthropic` without `--model` is refused cleanly.
    let missing_model = run_vitna(&["serve", "--provider", "anthropic"], &home, &base);
    assert!(!missing_model.status.success());
    assert!(String::from_utf8_lossy(&missing_model.stderr).contains("--provider needs --model"));

    // 5. Mixing `--runtime-url/--lane` with `--provider/--model` is refused.
    let mixed_routes = run_vitna(
        &[
            "serve",
            "--runtime-url",
            "http://127.0.0.1:8787",
            "--lane",
            "vitna/code",
            "--provider",
            "openai",
            "--model",
            "gpt-5",
        ],
        &home,
        &base,
    );
    assert!(!mixed_routes.status.success());
    assert!(String::from_utf8_lossy(&mixed_routes.stderr).contains("cannot combine"));

    // 6. Malformed `--mcp-http` spec is refused.
    let bad_mcp = run_vitna(&["serve", "--mcp-http", "broken-no-equals"], &home, &base);
    assert!(!bad_mcp.status.success());
    assert!(String::from_utf8_lossy(&bad_mcp.stderr).contains("<name>=<url>"));

    // 7. `vitna run` refuses partial/mixed route flags and orphan MCP flags.
    let run_missing_model = run_vitna(
        &["run", "hello", "--provider", "anthropic"],
        &home,
        &base,
    );
    assert!(!run_missing_model.status.success());
    assert!(String::from_utf8_lossy(&run_missing_model.stderr).contains("--provider needs --model"));

    let run_orphan_mcp = run_vitna(
        &["run", "hello", "--mcp-http", "ops=http://127.0.0.1:9999/mcp"],
        &home,
        &base,
    );
    assert!(!run_orphan_mcp.status.success());
    assert!(String::from_utf8_lossy(&run_orphan_mcp.stderr).contains("require a model route"));

    let _ = fs::remove_dir_all(&base);
}

/// Minimal HTTP/1.1 reader for loopback test servers (reads headers + exact Content-Length body).
fn read_http_request(stream: &mut std::net::TcpStream) -> Option<(String, String)> {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_len = head
        .lines()
        .find_map(|line| {
            let (k, v) = line.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_len {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..header_end + content_len]).to_string();
    Some((head, body))
}

fn write_http_response(
    stream: &mut std::net::TcpStream,
    content_type: &str,
    body: &str,
) {
    use std::io::Write;
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

#[test]
fn vitna_run_live_turn_executes_mcp_tool_and_injects_workspace_skill() {
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    let base = temp_dir("live_run_mcp_skills");
    let home = base.join("home");
    let ws = base.join("ws");
    let skill_dir = ws.join(".vitna").join("skills").join("ops-check");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: ops-check\ndescription: Verify service liveness via ops ping\n---\nAlways cite SKILL-MARKER-99.\n",
    )
    .unwrap();

    // 1. Spawn a loopback Streamable HTTP JSON-RPC MCP server.
    let mcp_listener = TcpListener::bind("127.0.0.1:0").expect("bind mcp");
    let mcp_addr = mcp_listener.local_addr().unwrap();
    let mcp_calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let mcp_calls_bg = mcp_calls.clone();
    std::thread::spawn(move || {
        for stream in mcp_listener.incoming().flatten() {
            let mut stream = stream;
            let Some((_head, body)) = read_http_request(&mut stream) else {
                continue;
            };
            let req: Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            let method = req["method"].as_str().unwrap_or("");
            let result = match method {
                "initialize" => serde_json::json!({
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "ops-mcp", "version": "1.0.0" }
                }),
                "tools/list" => serde_json::json!({
                    "tools": [{
                        "name": "ping",
                        "description": "Check ops liveness",
                        "inputSchema": { "type": "object", "properties": { "target": { "type": "string" } } }
                    }]
                }),
                "tools/call" => {
                    let tool_name = req["params"]["name"].as_str().unwrap_or("").to_string();
                    mcp_calls_bg.lock().unwrap().push(tool_name);
                    serde_json::json!({
                        "content": [{ "type": "text", "text": "pong-from-mcp" }],
                        "isError": false
                    })
                }
                _ => serde_json::json!({}),
            };
            let reply = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result
            })
            .to_string();
            write_http_response(&mut stream, "application/json", &reply);
        }
    });

    // 2. Spawn a loopback OpenAI-compatible SSE runtime server.
    let llm_listener = TcpListener::bind("127.0.0.1:0").expect("bind llm");
    let llm_addr = llm_listener.local_addr().unwrap();
    let seen_bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_bodies_bg = seen_bodies.clone();
    std::thread::spawn(move || {
        let mut round = 0usize;
        for stream in llm_listener.incoming().flatten() {
            let mut stream = stream;
            let Some((_head, body)) = read_http_request(&mut stream) else {
                continue;
            };
            seen_bodies_bg.lock().unwrap().push(body);
            round += 1;
            if round == 1 {
                let sse = concat!(
                    "data: {\"id\":\"chatcmpl-1\",\"model\":\"vitna/code\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"mcp__ops__ping\",\"arguments\":\"{\\\"target\\\":\\\"api\\\"}\"}}]},\"finish_reason\":null}]}\n\n",
                    "data: {\"id\":\"chatcmpl-1\",\"model\":\"vitna/code\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
                    "data: [DONE]\n\n"
                );
                write_http_response(&mut stream, "text/event-stream", sse);
            } else {
                let sse = concat!(
                    "data: {\"id\":\"chatcmpl-2\",\"model\":\"vitna/code\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"MCP + Skills \"},\"finish_reason\":null}]}\n\n",
                    "data: {\"id\":\"chatcmpl-2\",\"model\":\"vitna/code\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"verified!\"},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: [DONE]\n\n"
                );
                write_http_response(&mut stream, "text/event-stream", sse);
            }
        }
    });

    let runtime_url = format!("http://{llm_addr}");
    let mcp_spec = format!("ops=http://{mcp_addr}/mcp");
    let out = run_vitna(
        &[
            "run",
            "check ops liveness",
            "--workspace",
            ws.to_str().unwrap(),
            "--runtime-url",
            &runtime_url,
            "--lane",
            "vitna/code",
            "--mcp-http",
            &mcp_spec,
            "--auto-approve",
        ],
        &home,
        &ws,
    );
    assert!(
        out.status.success(),
        "live vitna run failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("MCP Tools:  1 attached"), "{stdout}");
    assert!(stdout.contains("Skills:     ops-check"), "{stdout}");
    assert!(stdout.contains("MCP + Skills verified!"), "{stdout}");

    // Confirm MCP server actually received the remote `ping` call.
    assert_eq!(*mcp_calls.lock().unwrap(), vec!["ping".to_string()]);

    // Confirm round 1 request carried the workspace skill + namespaced MCP tool,
    // and round 2 carried the MCP tool output.
    let bodies = seen_bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[0].contains("SKILL-MARKER-99"), "missing skill in round 1");
    assert!(bodies[0].contains("mcp__ops__ping"), "missing MCP tool in round 1");
    assert!(bodies[1].contains("pong-from-mcp"), "missing MCP output in round 2");

    // Verify the signed receipt on disk using `vitna verify`.
    let receipts_dir = ws.join(".vitna").join("receipts");
    let receipt_path = fs::read_dir(&receipts_dir)
        .expect("receipts dir")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .expect("signed receipt file");
    let verify = run_vitna(&["verify", receipt_path.to_str().unwrap()], &home, &ws);
    assert!(
        verify.status.success(),
        "vitna verify failed: {}",
        String::from_utf8_lossy(&verify.stderr)
    );

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn release_manifest_sign_and_verify_cli_round_trip() {
    let base = temp_dir("release_manifest_cli");
    let home = base.join("home");
    let dist = base.join("dist");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&dist).unwrap();

    let archive = dist.join("vitna-code-v0.2.0-x86_64-unknown-linux-gnu.tar.gz");
    fs::write(&archive, b"fake-tarball-payload-for-release-manifest-test").unwrap();
    let manifest_path = dist.join("manifest.json");
    let asset_arg = format!("x86_64-unknown-linux-gnu={}", archive.display());

    // 1. Sign a manifest over the real file on disk.
    let signed = run_vitna(
        &[
            "release",
            "sign",
            "--version",
            "0.2.0",
            "--channel",
            "beta",
            "--asset",
            &asset_arg,
            "--out",
            manifest_path.to_str().unwrap(),
        ],
        &home,
        &base,
    );
    assert!(
        signed.status.success(),
        "release sign failed: {}",
        String::from_utf8_lossy(&signed.stderr)
    );
    let manifest_raw = fs::read_to_string(&manifest_path).expect("read manifest");
    let manifest_json: Value = serde_json::from_str(&manifest_raw).unwrap();
    let pub_key = manifest_json["publisher_public_key"]
        .as_str()
        .expect("publisher_public_key")
        .to_string();

    // 2. Offline verify with pinned --key and --assets-dir succeeds.
    let verified = run_vitna(
        &[
            "release",
            "verify",
            manifest_path.to_str().unwrap(),
            "--key",
            &pub_key,
            "--assets-dir",
            dist.to_str().unwrap(),
            "--json",
        ],
        &home,
        &base,
    );
    assert!(
        verified.status.success(),
        "release verify failed: {}",
        String::from_utf8_lossy(&verified.stderr)
    );
    let report: Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(report["report"]["is_valid"].as_bool(), Some(true));
    assert_eq!(report["report"]["signature_verified"].as_bool(), Some(true));
    assert_eq!(report["disk_assets_verified"].as_array().unwrap().len(), 1);

    // 3. Tampering with the archive on disk fails verification.
    fs::write(&archive, b"corrupted-archive-bytes").unwrap();
    let tampered_disk = run_vitna(
        &[
            "release",
            "verify",
            manifest_path.to_str().unwrap(),
            "--key",
            &pub_key,
            "--assets-dir",
            dist.to_str().unwrap(),
        ],
        &home,
        &base,
    );
    assert!(!tampered_disk.status.success());

    // 4. Wrong --key fails closed even when manifest signature is self-consistent.
    let wrong_key = "0".repeat(64);
    let wrong_key_out = run_vitna(
        &[
            "release",
            "verify",
            manifest_path.to_str().unwrap(),
            "--key",
            &wrong_key,
        ],
        &home,
        &base,
    );
    assert!(!wrong_key_out.status.success());

    let _ = fs::remove_dir_all(&base);
}



