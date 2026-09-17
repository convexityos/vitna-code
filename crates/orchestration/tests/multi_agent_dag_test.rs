use std::fs;
use std::sync::{Arc, Mutex};
use vitna_git_broker::{GitBroker, MergeQueue, MergeQueueItem, MergeQueueResult};
use vitna_git_workspaces::AgentWorkspaceManager;
use vitna_orchestration::{OrchestrationConfig, OrchestrationEngine, StepType, TaskGraph, TaskNode, TaskStatus};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::{generate_signing_key, ChangeSetRecord, FileModificationRecord, ModelSelectionRecord, VitnaRunReceiptV1};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

#[tokio::test]
async fn test_multi_agent_dag_execution_and_receipt_aggregation() {
    let temp_dir = std::env::temp_dir().join(format!("vitna_dag_test_{}", std::process::id()));
    let primary_dir = temp_dir.join("repo");
    fs::create_dir_all(primary_dir.join("src")).expect("create primary dir");
    fs::write(primary_dir.join("README.md"), "# Multi-Agent Test Repo\n").expect("write readme");
    fs::write(primary_dir.join("src/main.py"), "print('hello')\n").expect("write main");

    // 1. Build Multi-Agent DAG:
    //    research -> backend -> tester
    let mut graph = TaskGraph::new();
    let t_research = TaskNode::new("task-research", "researcher", "Inspect workspace structure");
    let t_backend = TaskNode::new("task-backend", "backend_engineer", "Implement backend service")
        .with_dependency("task-research");
    let t_tester = TaskNode::new("task-tester", "tester", "Run verification suite")
        .with_dependency("task-backend");

    graph.add_node(t_research).expect("add research");
    graph.add_node(t_backend).expect("add backend");
    graph.add_node(t_tester).expect("add tester");

    graph.validate().expect("DAG must validate without cycles");

    let coordinator_key = generate_signing_key();
    let coordinator_pub_hex = hex::encode(coordinator_key.verifying_key().to_bytes());

    // -------------------------------------------------------------
    // Step A: Execute Researcher Agent
    // -------------------------------------------------------------
    let ready_step1 = graph.get_ready_tasks();
    assert_eq!(ready_step1, vec!["task-research".to_string()]);

    graph.mark_running("task-research", "run-res-01").expect("mark running");

    let store1 = Arc::new(Mutex::new(EventStore::open_in_memory().expect("store1")));
    let journal1 = temp_dir.join("researcher.journal");
    let runner1 = Arc::new(Mutex::new(FakeRunner::new(&journal1).expect("runner1")));
    let key1 = generate_signing_key();

    let config1 = OrchestrationConfig {
        session_id: "multi-sess-01".to_string(),
        run_id: "run-res-01".to_string(),
        workspace_root: primary_dir.clone(),
        auto_approve: true,
        model_sku: "claude-3-7-sonnet".to_string(),
        provider_name: "fake".to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: None,
    };

    let mut engine1 = OrchestrationEngine::new(config1, store1, runner1, key1);
    let inspect_res = engine1
        .execute_task_step(StepType::InspectWorkspace)
        .await
        .expect("research inspect");
    assert!(inspect_res.success);

    let receipt_research = engine1.finalize_run().await.expect("research finalize");
    graph
        .mark_succeeded("task-research", "Workspace inspected: main.py present")
        .expect("mark succeeded");

    // -------------------------------------------------------------
    // Step B: Execute Backend Agent in Isolated Workspace
    // -------------------------------------------------------------
    let ready_step2 = graph.get_ready_tasks();
    assert_eq!(ready_step2, vec!["task-backend".to_string()]);

    graph.mark_running("task-backend", "run-back-02").expect("mark running");

    // Create isolated branch clone
    let backend_ws = AgentWorkspaceManager::create_isolated_workspace(&primary_dir, "run-back-02")
        .expect("create isolated backend ws");

    let store2 = Arc::new(Mutex::new(EventStore::open_in_memory().expect("store2")));
    let journal2 = temp_dir.join("backend.journal");
    let runner2 = Arc::new(Mutex::new(FakeRunner::new(&journal2).expect("runner2")));
    let key2 = generate_signing_key();

    let config2 = OrchestrationConfig {
        session_id: "multi-sess-01".to_string(),
        run_id: "run-back-02".to_string(),
        workspace_root: backend_ws.agent_workspace_root.clone(),
        auto_approve: true,
        model_sku: "claude-3-7-sonnet".to_string(),
        provider_name: "fake".to_string(),
        sandbox_guarantee: "strong".to_string(),
        verification_command: None,
    };

    let mut engine2 = OrchestrationEngine::new(config2, store2, runner2, key2);

    // Backend agent creates service.py in isolated workspace
    let edit_res = engine2
        .execute_task_step(StepType::ProposeEdit {
            path: "src/service.py".to_string(),
            content: "def serve():\n    return {'status': 'ok'}\n".to_string(),
        })
        .await
        .expect("backend edit");
    assert!(edit_res.success);

    let receipt_backend = engine2.finalize_run().await.expect("backend finalize");

    // Inspect isolated changes and enqueue to MergeQueue
    let backend_changeset = GitBroker::inspect_changes(&backend_ws).expect("inspect changes");
    assert_eq!(backend_changeset.files.len(), 1);
    assert_eq!(backend_changeset.files[0].path, "src/service.py");

    let mut merge_queue = MergeQueue::new(&primary_dir);
    merge_queue.enqueue(MergeQueueItem {
        queue_id: "mq-01".to_string(),
        task_id: "task-backend".to_string(),
        agent_workspace_root: backend_ws.agent_workspace_root.clone(),
        changeset: backend_changeset,
    });

    let (merged_task, merge_res) = merge_queue.process_next().expect("process merge");
    assert_eq!(merged_task, "task-backend");
    match merge_res {
        MergeQueueResult::Merged { applied_files } => {
            assert_eq!(applied_files, vec!["src/service.py"]);
        }
        other => panic!("Unexpected merge result: {:?}", other),
    }

    // Verify service.py now exists in primary repository
    assert!(primary_dir.join("src/service.py").exists());

    AgentWorkspaceManager::cleanup_workspace(&backend_ws).expect("clean backend ws");
    graph
        .mark_succeeded("task-backend", "src/service.py implemented and merged")
        .expect("mark backend succeeded");

    // -------------------------------------------------------------
    // Step C: Execute Tester Agent on Primary
    // -------------------------------------------------------------
    let ready_step3 = graph.get_ready_tasks();
    assert_eq!(ready_step3, vec!["task-tester".to_string()]);

    graph.mark_running("task-tester", "run-test-03").expect("mark running");

    let store3 = Arc::new(Mutex::new(EventStore::open_in_memory().expect("store3")));
    let journal3 = temp_dir.join("tester.journal");
    let runner3 = Arc::new(Mutex::new(FakeRunner::new(&journal3).expect("runner3")));
    let key3 = generate_signing_key();

    let config3 = OrchestrationConfig {
        session_id: "multi-sess-01".to_string(),
        run_id: "run-test-03".to_string(),
        workspace_root: primary_dir.clone(),
        auto_approve: true,
        model_sku: "claude-3-7-sonnet".to_string(),
        provider_name: "fake".to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: Some("test-all".to_string()),
    };

    let mut engine3 = OrchestrationEngine::new(config3, store3, runner3, key3);
    let test_inspect = engine3
        .execute_task_step(StepType::InspectWorkspace)
        .await
        .expect("tester inspect");
    assert!(test_inspect.success);

    let receipt_tester = engine3.finalize_run().await.expect("tester finalize");
    graph
        .mark_succeeded("task-tester", "All verification tests passed")
        .expect("mark tester succeeded");

    // -------------------------------------------------------------
    // Step D: Coordinator Verification and Receipt Hierarchy
    // -------------------------------------------------------------
    assert!(graph.is_complete(), "Entire DAG pipeline must be complete");

    // Create aggregated parent receipt
    let mut parent_receipt = VitnaRunReceiptV1 {
        schema_version: "vitna-run-receipt-v1".to_string(),
        run_id: "run-coordinator-00".to_string(),
        session_id: "multi-sess-01".to_string(),
        workspace_fingerprint: "ws-dag-root".to_string(),
        base_commit_sha: "0000000000000000000000000000000000000000".to_string(),
        model_selection: ModelSelectionRecord {
            provider: "fake".to_string(),
            model_sku: "dag-coordinator".to_string(),
            routing_reason: "multi_agent_dag_pipeline".to_string(),
            policy_digest: None,
        },
        event_hash_chain_root: "merkle-coordinator-root".to_string(),
        isolation_label: "strong".to_string(),
        completion_state: "completed_with_evidence".to_string(),
        evidence_items: Vec::new(),
        changeset: ChangeSetRecord {
            files_modified: vec![FileModificationRecord {
                path: "src/service.py".to_string(),
                preimage_hash: vitna_git_broker::NULL_HASH.to_string(),
                postimage_hash: receipt_backend.changeset.files_modified[0].postimage_hash.clone(),
            }],
            diff_digest: receipt_backend.changeset.diff_digest.clone(),
        },
        runner_execution_statements: Vec::new(),
        child_receipt_roots: Vec::new(),
        device_signature: String::new(),
    };

    // Aggregate child receipts
    parent_receipt.aggregate_child_receipt(&receipt_research);
    parent_receipt.aggregate_child_receipt(&receipt_backend);
    parent_receipt.aggregate_child_receipt(&receipt_tester);

    assert_eq!(parent_receipt.child_receipt_roots.len(), 3);
    assert!(!parent_receipt.evidence_items.is_empty());

    // Sign parent receipt
    parent_receipt
        .sign(&coordinator_key)
        .expect("parent receipt must sign cleanly");

    // Cryptographic validation
    let report = ReceiptVerifier::verify_receipt(&parent_receipt, Some(&coordinator_pub_hex))
        .expect("verify receipt report");
    assert!(report.is_valid, "Report must be valid");
    assert_eq!(report.errors.len(), 0);
    assert_eq!(report.run_id, "run-coordinator-00");

    let _ = fs::remove_dir_all(&temp_dir);
}
