use std::fs;
use std::path::PathBuf;
use std::process::Command;
use vitna_receipt_verify::{
    compute_checkpoint_digest, verify_provenance_bundle, ProvenanceAuditOptions,
};
use vitna_receipts::{
    generate_signing_key, ChangeSetRecord, ModelSelectionRecord, VitnaRunReceiptV1,
};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitna_prov_cli_{}_{}",
        tag,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn signed_receipt_with_policy_digest(policy_digest: Option<String>) -> (VitnaRunReceiptV1, String) {
    let key = generate_signing_key();
    let pub_hex = hex::encode(key.verifying_key().to_bytes());
    let mut receipt = VitnaRunReceiptV1 {
        schema_version: "vitna-run-receipt-v1".to_string(),
        run_id: "run-bundle-01".to_string(),
        session_id: "sess-bundle-01".to_string(),
        workspace_fingerprint: "ws-fp-bundle".to_string(),
        base_commit_sha: "abcd1234abcd1234abcd1234abcd1234abcd1234".to_string(),
        model_selection: ModelSelectionRecord {
            provider: "vitna-anchor".to_string(),
            model_sku: "qwen3-coder-30b-a3b".to_string(),
            routing_reason: "local_attested_checkpoint".to_string(),
            policy_digest,
        },
        event_hash_chain_root: "merkle-root-bundle".to_string(),
        isolation_label: "strong".to_string(),
        completion_state: "completed_with_evidence".to_string(),
        evidence_items: Vec::new(),
        changeset: ChangeSetRecord {
            files_modified: Vec::new(),
            diff_digest: "diff-bundle".to_string(),
        },
        runner_execution_statements: Vec::new(),
        egress_calls: Vec::new(),
        child_receipt_roots: Vec::new(),
        device_public_key: None,
        mandate_provenance: None,
        device_signature: String::new(),
    };
    receipt.sign(&key).expect("sign receipt");
    (receipt, pub_hex)
}

#[test]
fn multi_shard_checkpoint_digest_matches_cross_repo_reference_vector() {
    let dir = temp_dir("shards");
    fs::write(dir.join("model-00002-of-00002.gguf"), b"GGUF-shard-two-bytes").unwrap();
    fs::write(dir.join("config.json"), b"{\"model_type\":\"qwen3\"}").unwrap();
    fs::write(dir.join("model-00001-of-00002.gguf"), b"GGUF-shard-one-bytes").unwrap();
    fs::write(dir.join("README.txt"), b"ignored readme file").unwrap();

    // Matches vitna-anchor's C vitna_checkpoint_sha256 & Python reference vector byte for byte.
    let hex = compute_checkpoint_digest(&dir).expect("compute manifest digest");
    assert_eq!(
        hex,
        "74ccf259c3cbe8bd688ae29a61c9784b13f4987377389b24684b12f7f232a01d"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn verifies_bundle_and_fails_closed_on_tampered_checkpoint_or_stale_strict_envelope() {
    let dir = temp_dir("bundle");
    let ckpt = dir.join("model.gguf");
    fs::write(&ckpt, b"expected-gguf-payload").unwrap();
    let ckpt_hex = compute_checkpoint_digest(&ckpt).unwrap();
    assert_eq!(
        ckpt_hex,
        "0eb9b673e1fa8f8abe1e1a378fd84e5886fd0d7bce7176dab0e928bfce8a7ae1"
    );

    let (receipt, pub_hex) = signed_receipt_with_policy_digest(Some(ckpt_hex.clone()));
    let receipt_path = dir.join("receipt.json");
    fs::write(&receipt_path, serde_json::to_string_pretty(&receipt).unwrap()).unwrap();

    let healthy_env = dir.join("quote_healthy.json");
    fs::write(
        &healthy_env,
        r#"{"data":{"symbol":"AAPL","price":214.32},"meta":{"data_as_of":"2026-07-10T20:00:04Z","sync_status":"healthy","source":"convexity"}}"#,
    )
    .unwrap();

    let stale_env = dir.join("quote_stale.json");
    fs::write(
        &stale_env,
        r#"{"data":{"symbol":"NVDA","price":128.4},"meta":{"data_as_of":"2026-07-07T20:00:00Z","sync_status":"stale","source":"convexity"}}"#,
    )
    .unwrap();

    // 1. Happy path: signed receipt + matching checkpoint + healthy envelope.
    let ok = verify_provenance_bundle(&ProvenanceAuditOptions {
        receipt_path: receipt_path.clone(),
        public_key_hex: Some(pub_hex.clone()),
        require_signature: true,
        checkpoint_path: Some(ckpt.clone()),
        envelope_paths: vec![healthy_env.clone()],
        strict_freshness: true,
    })
    .expect("audit runs");
    assert!(ok.overall_valid, "expected valid bundle, errors: {:?}", ok.errors);
    assert!(ok.receipt.signature_verified);
    assert!(ok.checkpoint.as_ref().unwrap().matched);
    assert_eq!(ok.envelopes.len(), 1);
    assert!(ok.envelopes[0].is_valid);

    // 2. --require-signature without public key fails closed.
    let no_key = verify_provenance_bundle(&ProvenanceAuditOptions {
        receipt_path: receipt_path.clone(),
        public_key_hex: None,
        require_signature: true,
        checkpoint_path: None,
        envelope_paths: vec![],
        strict_freshness: false,
    })
    .expect("audit runs");
    assert!(!no_key.overall_valid);

    // 3. Stale envelope warns by default, fails closed under strict_freshness.
    let stale_warn = verify_provenance_bundle(&ProvenanceAuditOptions {
        receipt_path: receipt_path.clone(),
        public_key_hex: Some(pub_hex.clone()),
        require_signature: true,
        checkpoint_path: None,
        envelope_paths: vec![stale_env.clone()],
        strict_freshness: false,
    })
    .expect("audit runs");
    assert!(stale_warn.overall_valid);
    assert!(!stale_warn.envelopes[0].warnings.is_empty());

    let stale_fail = verify_provenance_bundle(&ProvenanceAuditOptions {
        receipt_path: receipt_path.clone(),
        public_key_hex: Some(pub_hex.clone()),
        require_signature: true,
        checkpoint_path: None,
        envelope_paths: vec![stale_env.clone()],
        strict_freshness: true,
    })
    .expect("audit runs");
    assert!(!stale_fail.overall_valid);

    // 4. End-to-end CLI binary check with --json (passes before tampering, fails after tampering).
    let bin = env!("CARGO_BIN_EXE_vitna-receipt-verify");
    let out = Command::new(bin)
        .arg(&receipt_path)
        .args(["--public-key", &pub_hex, "--require-signature", "--json"])
        .arg("--checkpoint-path")
        .arg(&ckpt)
        .arg("--envelope")
        .arg(&healthy_env)
        .output()
        .expect("run CLI");
    assert!(out.status.success(), "CLI failed: {}", String::from_utf8_lossy(&out.stderr));
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(parsed["overall_valid"], true);
    assert_eq!(parsed["checkpoint"]["matched"], true);

    // Tamper checkpoint -> CLI must exit non-zero and report overall_valid=false.
    fs::write(&ckpt, b"tampered-gguf-payload").unwrap();
    let bad_out = Command::new(bin)
        .arg(&receipt_path)
        .args(["--public-key", &pub_hex, "--require-signature", "--json"])
        .arg("--checkpoint-path")
        .arg(&ckpt)
        .output()
        .expect("run CLI on tampered checkpoint");
    assert!(!bad_out.status.success(), "CLI must exit non-zero on checkpoint mismatch");
    let bad_parsed: serde_json::Value = serde_json::from_slice(&bad_out.stdout).expect("valid JSON");
    assert_eq!(bad_parsed["overall_valid"], false);
    assert_eq!(bad_parsed["checkpoint"]["matched"], false);

    let _ = fs::remove_dir_all(&dir);
}
