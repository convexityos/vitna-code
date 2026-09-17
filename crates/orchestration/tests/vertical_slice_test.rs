use std::fs;
use std::sync::{Arc, Mutex};
use vitna_orchestration::{OrchestrationConfig, OrchestrationEngine, StepType};
use vitna_receipt_verify::verify_receipt_json;
use vitna_receipts::generate_signing_key;
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

#[tokio::test]
async fn test_durable_vertical_slice_end_to_end() {
    let temp_dir = std::env::temp_dir().join(format!("vitna_vslice_test_{}", std::process::id()));
    fs::create_dir_all(temp_dir.join("src")).expect("create test workspace src dir");

    // 1. Pre-seed initial workspace file
    let initial_lib_content = "// Vitna initial lib\npub fn hello() -> &'static str { \"hello\" }\n";
    fs::write(temp_dir.join("src/lib.rs"), initial_lib_content).expect("write initial lib.rs");

    // 2. Setup event store, runner, and signing key
    let store = Arc::new(Mutex::new(EventStore::open_in_memory().expect("open store")));
    let journal_path = temp_dir.join("runner.journal");
    let fake_runner = FakeRunner::new(&journal_path).expect("open fake runner");
    let runner = Arc::new(Mutex::new(fake_runner));

    let signing_key = generate_signing_key();
    let verifying_key = signing_key.verifying_key();

    let config = OrchestrationConfig {
        session_id: "sess-vslice-alpha".to_string(),
        run_id: "run-vslice-001".to_string(),
        workspace_root: temp_dir.clone(),
        auto_approve: true,
        model_sku: "claude-3-7-sonnet".to_string(),
        provider_name: "fake".to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: Some("cargo test".to_string()),
    };

    let mut engine = OrchestrationEngine::new(config, store.clone(), runner, signing_key);

    // 3. Step 1: Inspect workspace
    let inspect_res = engine
        .execute_task_step(StepType::InspectWorkspace)
        .await
        .expect("inspect workspace");
    assert!(inspect_res.success);
    assert!(inspect_res.output.contains("src"));

    // 4. Step 2: Propose new file creation (src/utils.rs)
    let new_file_content = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";
    let edit1_res = engine
        .execute_task_step(StepType::ProposeEdit {
            path: "src/utils.rs".to_string(),
            content: new_file_content.to_string(),
        })
        .await
        .expect("write utils.rs");
    assert!(edit1_res.success);
    assert_eq!(
        edit1_res.preimage_hash.unwrap(),
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
    assert!(edit1_res.postimage_hash.is_some());

    // 5. Step 3: Propose modification of existing file (src/lib.rs)
    let updated_lib_content = format!("pub mod utils;\n{}", initial_lib_content);
    let edit2_res = engine
        .execute_task_step(StepType::ProposeEdit {
            path: "src/lib.rs".to_string(),
            content: updated_lib_content.clone(),
        })
        .await
        .expect("edit lib.rs");
    assert!(edit2_res.success);
    assert_ne!(
        edit2_res.preimage_hash.as_deref().unwrap(),
        "0000000000000000000000000000000000000000000000000000000000000000"
    );
    assert!(edit2_res.diff.is_some());

    // 6. Step 4: Finalize run (executes verification and signs receipt)
    let receipt = engine.finalize_run().await.expect("finalize run");

    // 7. Verify receipt invariants
    assert_eq!(receipt.run_id, "run-vslice-001");
    assert_eq!(receipt.session_id, "sess-vslice-alpha");
    assert_eq!(receipt.isolation_label, "guarded");
    assert_eq!(receipt.completion_state, "completed_with_evidence");

    // Verify changeset preimage/postimage tracking
    assert_eq!(receipt.changeset.files_modified.len(), 2);
    let paths: Vec<&str> = receipt.changeset.files_modified.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"src/utils.rs"));
    assert!(paths.contains(&"src/lib.rs"));

    // Verify evidence items
    assert!(!receipt.evidence_items.is_empty());
    assert_eq!(receipt.evidence_items[0].grade, "sandbox_captured");
    assert!(receipt.evidence_items[0].description.contains("cargo test"));

    // Verify cryptographic signature
    let sig_valid = receipt.verify_signature(&verifying_key).expect("verify signature");
    assert!(sig_valid, "Cryptographic receipt signature must verify strictly");

    // 8. Verify via standalone offline receipt verifier
    let receipt_path = temp_dir.join(".vitna/receipts/run-vslice-001.json");
    assert!(receipt_path.exists(), "Receipt file must be saved on disk");
    let receipt_json = fs::read_to_string(&receipt_path).expect("read receipt JSON");
    let verification_report = verify_receipt_json(&receipt_json).expect("offline verify receipt");
    assert_eq!(verification_report.receipt.run_id, "run-vslice-001");

    // 9. Verify event store replay continuity
    let store_lock = store.lock().expect("store lock");
    let events = store_lock.get_events("run-vslice-001", 0).expect("get events");
    assert!(events.len() >= 6, "Must have recorded complete event trail");

    let _ = fs::remove_dir_all(&temp_dir);
}
