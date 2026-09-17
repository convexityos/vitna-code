//! Turn execution loop, agent state machine, approval gating, and receipt emission.

pub mod dag;
pub mod engine;

pub use dag::{TaskGraph, TaskNode, TaskStatus};
pub use engine::{OrchestrationConfig, OrchestrationEngine, StepType};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use vitna_receipts::generate_signing_key;
    use vitna_runner::FakeRunner;
    use vitna_store::EventStore;

    #[tokio::test]
    async fn test_orchestration_engine_turn_and_receipt() {
        let temp_dir = std::env::temp_dir().join(format!("vitna_orch_test_{}", std::process::id()));
        fs::create_dir_all(&temp_dir).expect("create temp dir");

        let store = Arc::new(Mutex::new(EventStore::open_in_memory().expect("open store")));
        let journal_path = temp_dir.join("test.journal");
        let fake_runner = FakeRunner::new(&journal_path).expect("open fake runner");
        let runner = Arc::new(Mutex::new(fake_runner));
        let signing_key = generate_signing_key();
        let verifying_key = signing_key.verifying_key();

        let config = OrchestrationConfig {
            session_id: "sess-01".to_string(),
            run_id: "run-01".to_string(),
            workspace_root: temp_dir.clone(),
            auto_approve: true,
            model_sku: "claude-3-7-sonnet".to_string(),
            provider_name: "fake".to_string(),
            sandbox_guarantee: "strong".to_string(),
            verification_command: Some("test-verify-command".to_string()),
        };

        let mut engine = OrchestrationEngine::new(config, store.clone(), runner, signing_key);

        // Step 1: Inspect workspace
        let inspect_res = engine
            .execute_task_step(StepType::InspectWorkspace)
            .await
            .expect("inspect succeeds");
        assert!(inspect_res.success);

        // Step 2: Propose edit
        let edit_res = engine
            .execute_task_step(StepType::ProposeEdit {
                path: "test.py".to_string(),
                content: "def add(a, b):\n    return a + b\n".to_string(),
            })
            .await
            .expect("edit succeeds");
        assert!(edit_res.success);
        assert!(edit_res.diff.is_some());

        // Step 3: Finalize run (executes verification and generates receipt)
        let receipt = engine.finalize_run().await.expect("finalize succeeds");

        assert_eq!(receipt.run_id, "run-01");
        assert_eq!(receipt.session_id, "sess-01");
        assert_eq!(receipt.changeset.files_modified.len(), 1);
        assert_eq!(receipt.changeset.files_modified[0].path, "test.py");
        assert!(!receipt.evidence_items.is_empty());
        assert_eq!(receipt.evidence_items[0].grade, "sandbox_captured");
        assert!(!receipt.device_signature.is_empty());

        // Verify receipt cryptographically
        let is_valid = receipt.verify_signature(&verifying_key).expect("verify");
        assert!(is_valid, "Generated receipt must have valid cryptographic signature");

        // Verify receipt file exists on disk
        let receipt_file = temp_dir.join(".vitna").join("receipts").join("run-01.json");
        assert!(receipt_file.exists(), "Receipt file must be saved on disk");

        let _ = fs::remove_dir_all(&temp_dir);
    }
}