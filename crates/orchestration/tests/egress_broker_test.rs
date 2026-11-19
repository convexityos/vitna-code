//! End-to-end integration tests for brokered `net:http_fetch` egress through
//! `OrchestrationEngine`: exact-action approval with `effect_class == "network"`,
//! fail-closed allowlist enforcement, redacted `vitna.audit.v1.EgressCall`
//! hash-chained audit events, `broker_observed` evidence, and signed receipt
//! verification via `ReceiptVerifier`.

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vitna_orchestration::audit;
use vitna_orchestration::{
    ApprovalDecision, ApprovalRequest, Approver, OrchestrationConfig, OrchestrationEngine,
    MAX_ROUNDS,
};
use vitna_protocol::messages::ToolProposed;
use vitna_protocol::type_url;
use vitna_providers::{
    Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall,
};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::{generate_signing_key, EgressCallRecord};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch_workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitna_egress_orch_{}_{}_{}",
        tag,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("workspace");
    fs::write(dir.join("index.html"), "<html><body>Local App Ready</body></html>")
        .expect("seed html");
    dir
}

struct ScriptedProvider {
    replies: Mutex<VecDeque<Result<ProviderResponse, String>>>,
}

impl ScriptedProvider {
    fn new(replies: Vec<Result<ProviderResponse, String>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
        }
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }
    fn parse_sse_event(&self, _: &str, _: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        Ok(Vec::new())
    }
    async fn complete(&self, _req: &ProviderRequest) -> Result<ProviderResponse, String> {
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("script exhausted".to_string()))
    }
}

fn say(text: &str) -> Result<ProviderResponse, String> {
    Ok(ProviderResponse {
        content: text.to_string(),
        finish_reason: "stop".to_string(),
        prompt_tokens: 10,
        completion_tokens: 5,
        model: Some("vitna/code".to_string()),
        ..Default::default()
    })
}

fn call(id: &str, name: &str, arguments: serde_json::Value) -> Result<ProviderResponse, String> {
    Ok(ProviderResponse {
        tool_calls: vec![ProviderToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
        }],
        finish_reason: "tool_calls".to_string(),
        prompt_tokens: 10,
        completion_tokens: 5,
        model: Some("vitna/code".to_string()),
        ..Default::default()
    })
}

struct RecordingApprover {
    decision: ApprovalDecision,
    asked: Mutex<Vec<ApprovalRequest>>,
}

impl RecordingApprover {
    fn new(decision: ApprovalDecision) -> Arc<Self> {
        Arc::new(Self {
            decision,
            asked: Mutex::new(Vec::new()),
        })
    }
}

#[async_trait]
impl Approver for RecordingApprover {
    fn asks_a_person(&self) -> bool {
        true
    }
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        self.asked.lock().unwrap().push(request.clone());
        self.decision.clone()
    }
}

async fn spawn_http_server(body: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = hits.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            hits_clone.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://127.0.0.1:{}", addr.port()), hits)
}

fn build_engine(
    tag: &str,
    approver: Arc<RecordingApprover>,
    allowlist: Vec<String>,
) -> (OrchestrationEngine, Arc<Mutex<EventStore>>, PathBuf, String) {
    let root = scratch_workspace(tag);
    let store = Arc::new(Mutex::new(EventStore::open_in_memory().expect("store")));
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(root.join("runner.journal")).expect("runner"),
    ));
    let key = generate_signing_key();
    let public_key = hex::encode(key.verifying_key().to_bytes());
    let config = OrchestrationConfig {
        session_id: format!("sess-{tag}"),
        run_id: format!("run-{tag}"),
        workspace_root: root.clone(),
        auto_approve: false,
        model_sku: "vitna/code".to_string(),
        provider_name: "scripted".to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: None,
        allow_unsandboxed: false,
    };
    let mut engine = OrchestrationEngine::new(config, store.clone(), runner, key)
        .with_approver(approver)
        .with_egress_allowlist(allowlist);
    engine.routing_reason = "runtime_lane:vitna/code".to_string();
    (engine, store, root, public_key)
}

#[tokio::test]
async fn http_fetch_requires_network_approval_redacts_query_and_signs_egress_receipt() {
    let body = "vitna-egress-verified-payload";
    let (base_url, hits) = spawn_http_server(body).await;
    let approver = RecordingApprover::new(ApprovalDecision::Approved);
    let (mut engine, store, root, public_key) = build_engine(
        "http_fetch_ok",
        approver.clone(),
        vec!["127.0.0.1".to_string()],
    );

    let url_with_secret = format!("{base_url}/status?api_key=should-never-appear-in-receipt");
    let provider = ScriptedProvider::new(vec![
        call(
            "call-fetch-1",
            "http_fetch",
            json!({ "url": url_with_secret, "method": "GET" }),
        ),
        say("Fetched status cleanly."),
    ]);

    let turn = engine
        .run_agent_turn(&provider, &[], "Check local status endpoint", MAX_ROUNDS)
        .await
        .expect("turn should complete");
    assert_eq!(turn.text, "Fetched status cleanly.");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Operator was prompted for exact-action approval on http_fetch.
    {
        let asked = approver.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].tool_name, "http_fetch");
        assert!(asked[0].description.contains("127.0.0.1"));
    }

    let receipt = engine.finalize_run().await.expect("finalize receipt");

    // Verify hash-chained events include ToolProposed(network) and EgressCall.
    let events = store
        .lock()
        .unwrap()
        .get_events(&receipt.run_id, 0)
        .expect("events");
    let proposed = events
        .iter()
        .find(|e| e.type_url == type_url::event::TOOL_PROPOSED)
        .expect("ToolProposed event");
    let proposed_msg: ToolProposed = serde_json::from_slice(&proposed.encrypted_payload).unwrap();
    assert_eq!(proposed_msg.effect_class, "network");

    let egress_event = events
        .iter()
        .find(|e| e.type_url == audit::EGRESS_CALL)
        .expect("vitna.audit.v1.EgressCall event must be recorded");
    let payload_text = String::from_utf8_lossy(&egress_event.encrypted_payload);
    assert!(
        !payload_text.contains("should-never-appear-in-receipt"),
        "query secret must be stripped from audit event: {payload_text}"
    );
    let event_rec: EgressCallRecord =
        serde_json::from_slice(&egress_event.encrypted_payload).unwrap();
    assert_eq!(event_rec.target_domain, "127.0.0.1");
    assert_eq!(event_rec.status_code, 200);

    // Verify signed receipt carries the exact EgressCallRecord and broker_observed evidence.
    let expected_sha = hex::encode(Sha256::digest(body.as_bytes()));
    assert_eq!(receipt.egress_calls.len(), 1);
    assert_eq!(receipt.egress_calls[0].target_domain, "127.0.0.1");
    assert_eq!(receipt.egress_calls[0].method, "GET");
    assert_eq!(receipt.egress_calls[0].status_code, 200);
    assert_eq!(receipt.egress_calls[0].response_sha256, expected_sha);
    assert!(
        !receipt.egress_calls[0]
            .redacted_url
            .contains("should-never-appear-in-receipt"),
        "query secret must be stripped from receipt"
    );
    assert!(receipt.evidence_items.iter().any(|ev| {
        ev.grade == "broker_observed"
            && ev.description.contains("Brokered HTTP GET")
            && ev.artifact_digest.as_deref() == Some(expected_sha.as_str())
    }));

    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&public_key))
        .expect("verifier must succeed");
    assert!(report.is_valid, "errors: {:?}", report.errors);
    assert!(report.signature_verified);

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn http_fetch_denied_by_operator_makes_zero_network_requests() {
    let (base_url, hits) = spawn_http_server("unreachable").await;
    let approver = RecordingApprover::new(ApprovalDecision::Rejected(
        "operator declined network fetch".to_string(),
    ));
    let (mut engine, store, root, public_key) = build_engine(
        "http_fetch_rejected",
        approver.clone(),
        vec!["127.0.0.1".to_string()],
    );

    let provider = ScriptedProvider::new(vec![
        call(
            "call-fetch-reject",
            "http_fetch",
            json!({ "url": format!("{base_url}/status") }),
        ),
        say("Understood, network fetch was declined."),
    ]);

    engine
        .run_agent_turn(&provider, &[], "Fetch status", MAX_ROUNDS)
        .await
        .expect("turn completes cleanly after rejection");
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    let receipt = engine.finalize_run().await.expect("receipt");
    assert!(receipt.egress_calls.is_empty());
    let events = store
        .lock()
        .unwrap()
        .get_events(&receipt.run_id, 0)
        .expect("events");
    assert!(events.iter().all(|e| e.type_url != audit::EGRESS_CALL));

    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&public_key)).unwrap();
    assert!(report.is_valid, "errors: {:?}", report.errors);

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn http_fetch_fails_closed_without_allowlist_even_when_approved() {
    let (base_url, hits) = spawn_http_server("unreachable").await;
    let approver = RecordingApprover::new(ApprovalDecision::Approved);
    let (mut engine, _store, root, public_key) =
        build_engine("http_fetch_unallowlisted", approver.clone(), Vec::new());

    let provider = ScriptedProvider::new(vec![
        call(
            "call-fetch-denied",
            "http_fetch",
            json!({ "url": format!("{base_url}/status") }),
        ),
        say("Host is not allowlisted."),
    ]);

    engine
        .run_agent_turn(&provider, &[], "Fetch unallowlisted", MAX_ROUNDS)
        .await
        .expect("turn completes cleanly after broker denial");
    assert_eq!(hits.load(Ordering::SeqCst), 0);

    let receipt = engine.finalize_run().await.expect("receipt");
    assert!(receipt.egress_calls.is_empty());
    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&public_key)).unwrap();
    assert!(report.is_valid, "errors: {:?}", report.errors);

    let _ = fs::remove_dir_all(&root);
}

#[tokio::test]
async fn browser_verify_remote_requires_network_approval_while_local_file_stays_read_only() {
    let (base_url, hits) = spawn_http_server("<html><body>Remote Health OK</body></html>").await;
    let approver = RecordingApprover::new(ApprovalDecision::Approved);
    let (mut engine, store, root, public_key) = build_engine(
        "browser_verify_split",
        approver.clone(),
        vec!["127.0.0.1".to_string()],
    );

    let provider = ScriptedProvider::new(vec![
        call(
            "call-bv-local",
            "browser_verify",
            json!({ "url_or_path": "index.html", "expected_text": "Local App Ready" }),
        ),
        call(
            "call-bv-remote",
            "browser_verify",
            json!({ "url_or_path": format!("{base_url}/health"), "expected_text": "Remote Health OK" }),
        ),
        say("Both local and remote checks passed."),
    ]);

    engine
        .run_agent_turn(&provider, &[], "Verify local and remote", MAX_ROUNDS)
        .await
        .expect("turn completes");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // Only the remote browser_verify call should have asked the operator for approval.
    {
        let asked = approver.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].tool_name, "browser_verify");
    }

    let receipt = engine.finalize_run().await.expect("receipt");
    assert_eq!(receipt.egress_calls.len(), 1);
    assert_eq!(receipt.egress_calls[0].target_domain, "127.0.0.1");

    // Verify ToolProposed effect classes: local was "read", remote was "network".
    let events = store
        .lock()
        .unwrap()
        .get_events(&receipt.run_id, 0)
        .expect("events");
    let proposed_classes: Vec<String> = events
        .iter()
        .filter(|e| e.type_url == type_url::event::TOOL_PROPOSED)
        .map(|e| {
            serde_json::from_slice::<ToolProposed>(&e.encrypted_payload)
                .unwrap()
                .effect_class
        })
        .collect();
    assert_eq!(proposed_classes, vec!["read", "network"]);

    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&public_key)).unwrap();
    assert!(report.is_valid, "errors: {:?}", report.errors);
    assert!(report.signature_verified);

    let _ = fs::remove_dir_all(&root);
}
