//! Integration tests proving fail-closed read-only `inspect` mode enforcement
//! across `DaemonServer`, `OrchestrationEngine`, and `ApplyChangeSet`.

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use vitna_daemon::{DaemonServer, ModelRoute};
use vitna_git_broker::GitBroker;
use vitna_git_workspaces::AgentWorkspaceManager;
use vitna_protocol::messages::{ApplyChangeSet, RunStateChanged};
use vitna_protocol::type_url;
use vitna_providers::{
    Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall,
};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::{generate_signing_key, VitnaRunReceiptV1};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitna_inspect_mode_{}_{}_{}",
        tag,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

struct ScriptedProvider {
    replies: Mutex<VecDeque<ProviderResponse>>,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

impl ScriptedProvider {
    fn new(replies: Vec<ProviderResponse>) -> (Arc<Self>, Arc<Mutex<Vec<ProviderRequest>>>) {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(Self {
            replies: Mutex::new(replies.into()),
            requests: Arc::clone(&requests),
        });
        (provider, requests)
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted-inspect"
    }

    fn parse_sse_event(&self, _: &str, _: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        Ok(Vec::new())
    }

    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String> {
        self.requests.lock().unwrap().push(req.clone());
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "scripted replies exhausted".to_string())
    }
}

#[tokio::test]
async fn inspect_mode_allows_reads_and_refuses_mutating_tools_without_prompting() {
    let base = scratch_dir("tools");
    let ws = base.join("ws");
    fs::create_dir_all(&ws).unwrap();
    fs::write(ws.join("notes.txt"), "read-only-evidence-42\n").unwrap();

    let (provider, requests) = ScriptedProvider::new(vec![
        // 1. Read-only tool must succeed without approval.
        ProviderResponse {
            tool_calls: vec![ProviderToolCall {
                id: "tc-read".to_string(),
                name: "read_file".to_string(),
                arguments: json!({ "path": "notes.txt" }),
            }],
            finish_reason: "tool_calls".to_string(),
            model: Some("scripted/v1".to_string()),
            ..Default::default()
        },
        // 2. Mutating file write must be refused by inspect policy (never prompted, never written).
        ProviderResponse {
            tool_calls: vec![ProviderToolCall {
                id: "tc-write".to_string(),
                name: "write_file".to_string(),
                arguments: json!({ "path": "evil.txt", "content": "should not exist" }),
            }],
            finish_reason: "tool_calls".to_string(),
            model: Some("scripted/v1".to_string()),
            ..Default::default()
        },
        // 3. Command execution must also be refused by inspect policy.
        ProviderResponse {
            tool_calls: vec![ProviderToolCall {
                id: "tc-cmd".to_string(),
                name: "run_command".to_string(),
                arguments: json!({ "command": "echo hi" }),
            }],
            finish_reason: "tool_calls".to_string(),
            model: Some("scripted/v1".to_string()),
            ..Default::default()
        },
        ProviderResponse {
            content: "Inspected notes.txt; writes were refused in inspect mode.".to_string(),
            finish_reason: "stop".to_string(),
            model: Some("scripted/v1".to_string()),
            ..Default::default()
        },
    ]);

    let store = EventStore::open_in_memory().expect("store");
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(base.join("runner.journal")).expect("runner"),
    ));
    let key = generate_signing_key();
    let pub_hex = hex::encode(key.verifying_key().to_bytes());
    let daemon = DaemonServer::new(store, runner, key).with_route(ModelRoute {
        provider,
        model: "scripted/v1".to_string(),
        routing_reason: "test:inspect".to_string(),
    });

    // Reject unknown session modes fail-closed.
    assert!(daemon.create_session_with_mode(&ws, "turbo_yolo").is_err());

    let session = daemon
        .create_session_with_mode(&ws, "inspect")
        .expect("create inspect session");
    assert_eq!(session.mode, "inspect");

    let run_id = daemon
        .submit_turn(&session.session_id, "Inspect this workspace")
        .expect("submit turn");

    // Wait for terminal state WITHOUT answering any approvals: inspect mode
    // must refuse mutating tools immediately without blocking on ApprovalRequested.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let (final_state, approval_events) = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "inspect-mode turn timed out (likely blocked waiting on approval)"
        );
        let events = daemon
            .store
            .lock()
            .unwrap()
            .get_events(&run_id, 0)
            .expect("events");
        let count = events
            .iter()
            .filter(|e| e.type_url == type_url::event::APPROVAL_REQUESTED)
            .count();
        let done = events.iter().find_map(|ev| {
            if ev.type_url == type_url::event::RUN_STATE_CHANGED {
                let st: RunStateChanged = serde_json::from_slice(&ev.encrypted_payload).ok()?;
                if matches!(st.new_state.as_str(), "completed" | "failed" | "cancelled") {
                    return Some(st.new_state);
                }
            }
            None
        });
        if let Some(state) = done {
            break (state, count);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };

    assert_eq!(final_state, "completed");
    assert_eq!(
        approval_events, 0,
        "inspect mode must refuse mutating tools without emitting ApprovalRequested"
    );
    assert!(
        !ws.join("evil.txt").exists(),
        "mutating write_file must never touch disk in inspect mode"
    );

    // Verify model received read_file contents and inspect-mode refusals.
    let reqs = requests.lock().unwrap();
    assert_eq!(reqs.len(), 4);
    assert!(reqs[1]
        .messages
        .iter()
        .any(|m| m.role == "tool" && m.content.contains("read-only-evidence-42")));
    assert!(reqs[2]
        .messages
        .iter()
        .any(|m| m.role == "tool" && m.content.contains("Mode 'inspect' is read-only")));
    assert!(reqs[3]
        .messages
        .iter()
        .any(|m| m.role == "tool" && m.content.contains("Mode 'inspect' is read-only")));

    // Verify signed receipt on disk records read_only isolation and zero edits.
    let receipt_path = ws.join(".vitna").join("receipts").join(format!("{run_id}.json"));
    let receipt: VitnaRunReceiptV1 =
        serde_json::from_str(&fs::read_to_string(&receipt_path).expect("read receipt"))
            .expect("parse receipt");
    assert_eq!(receipt.isolation_label, "read_only");
    assert!(receipt.changeset.files_modified.is_empty());
    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex)).expect("verify");
    assert!(report.is_valid, "errors: {:?}", report.errors);
    assert!(report.signature_verified);

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn inspect_mode_refuses_apply_changeset() {
    let base = scratch_dir("apply_refuse");
    let ws = base.join("ws");
    fs::create_dir_all(&ws).unwrap();
    fs::write(ws.join("hello.txt"), "before\n").unwrap();

    let store = EventStore::open_in_memory().expect("store");
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(base.join("runner.journal")).expect("runner"),
    ));
    let daemon = DaemonServer::new(store, runner, generate_signing_key());

    let session = daemon
        .create_session_with_mode(&ws, "inspect")
        .expect("inspect session");

    let isolated =
        AgentWorkspaceManager::create_isolated_workspace(&ws, "run-child-inspect").unwrap();
    fs::write(isolated.agent_workspace_root.join("hello.txt"), "after\n").unwrap();

    let changeset = GitBroker::inspect_changes(&isolated).expect("inspect changes");
    let cs_id = daemon
        .stage_changeset(&session.session_id, "run-child-inspect", changeset, isolated)
        .expect("stage");

    let err = daemon
        .apply_changeset(&ApplyChangeSet {
            change_set_id: cs_id,
            target_checkout_path: ws.to_string_lossy().into_owned(),
            expected_base_commit: String::new(),
            idempotency_key: String::new(),
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("inspect"),
        "expected inspect-mode refusal, got: {err}"
    );
    assert_eq!(fs::read_to_string(ws.join("hello.txt")).unwrap(), "before\n");

    let _ = fs::remove_dir_all(&base);
}
