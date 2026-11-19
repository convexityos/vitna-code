//! The agent loop against a scripted model.
//!
//! A real provider is never called here: a test that needed a key would be
//! skipped in CI, and a skipped test proves nothing. The scripted provider
//! answers from a list and records every request, so each test can check what
//! the model was shown as well as what the daemon did with its answer.

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;
use vitna_orchestration::{
    ApprovalDecision, ApprovalRequest, Approver, OrchestrationConfig, OrchestrationEngine,
    MAX_ROUNDS,
};
use vitna_protocol::messages::{ApprovalRequested, RunStateChanged, ToolFinished, ToolProposed};
use vitna_protocol::type_url;
use vitna_providers::{
    Provider, ProviderMessage, ProviderRequest, ProviderResponse, ProviderStreamEvent,
    ProviderToolCall,
};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::generate_signing_key;
use vitna_runner::FakeRunner;
use vitna_store::{EventRecord, EventStore};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitna_agent_turn_{}_{}_{}",
        tag,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("workspace");
    fs::write(dir.join("README.md"), "hello\n").expect("seed file");
    dir
}

/// Answers from a script, in order, and keeps every request it was sent.
struct Scripted {
    replies: Mutex<VecDeque<Result<ProviderResponse, String>>>,
    seen: Mutex<Vec<ProviderRequest>>,
}

impl Scripted {
    fn new(replies: Vec<Result<ProviderResponse, String>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }
    fn parse_sse_event(&self, _: &str, _: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        Ok(Vec::new())
    }
    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String> {
        self.seen.lock().unwrap().push(req.clone());
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("the script ran out".to_string()))
    }
}

fn say(text: &str, model: &str) -> Result<ProviderResponse, String> {
    Ok(ProviderResponse {
        content: text.to_string(),
        finish_reason: "stop".to_string(),
        prompt_tokens: 10,
        completion_tokens: 5,
        model: Some(model.to_string()),
        ..Default::default()
    })
}

fn call(
    id: &str,
    name: &str,
    arguments: serde_json::Value,
    model: &str,
) -> Result<ProviderResponse, String> {
    Ok(ProviderResponse {
        tool_calls: vec![ProviderToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
        }],
        finish_reason: "tool_calls".to_string(),
        prompt_tokens: 10,
        completion_tokens: 5,
        model: Some(model.to_string()),
        ..Default::default()
    })
}

/// Answers every approval the same way and keeps what it was asked.
struct Operator {
    answer: ApprovalDecision,
    asked: Mutex<Vec<ApprovalRequest>>,
}

#[async_trait]
impl Approver for Operator {
    fn asks_a_person(&self) -> bool {
        true
    }
    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        self.asked.lock().unwrap().push(request.clone());
        self.answer.clone()
    }
}

struct Run {
    engine: OrchestrationEngine,
    store: Arc<Mutex<EventStore>>,
    root: PathBuf,
    public_key: String,
}

fn run(tag: &str, operator: Option<Arc<Operator>>) -> Run {
    let root = workspace(tag);
    let store = Arc::new(Mutex::new(EventStore::open_in_memory().expect("store")));
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(root.join("runner.journal")).expect("runner"),
    ));
    let key = generate_signing_key();
    let public_key = hex::encode(key.verifying_key().to_bytes());
    let config = OrchestrationConfig {
        session_id: "sess-agent".to_string(),
        run_id: format!("run-{tag}"),
        workspace_root: root.clone(),
        auto_approve: false,
        model_sku: "vitna/code".to_string(),
        provider_name: "scripted".to_string(),
        sandbox_guarantee: "guarded".to_string(),
        verification_command: None,
        allow_unsandboxed: false,
    };
    let mut engine = OrchestrationEngine::new(config, store.clone(), runner, key);
    engine.routing_reason = "runtime_lane:vitna/code".to_string();
    if let Some(op) = operator {
        engine = engine.with_approver(op);
    }
    Run {
        engine,
        store,
        root,
        public_key,
    }
}

fn events(store: &Arc<Mutex<EventStore>>, run_id: &str) -> Vec<EventRecord> {
    store.lock().unwrap().get_events(run_id, 0).expect("events")
}

fn payloads<T: serde::de::DeserializeOwned>(events: &[EventRecord], url: &str) -> Vec<T> {
    events
        .iter()
        .filter(|e| e.type_url == url)
        .map(|e| serde_json::from_slice(&e.encrypted_payload).expect("declared payload"))
        .collect()
}

fn states(events: &[EventRecord]) -> Vec<String> {
    payloads::<RunStateChanged>(events, type_url::event::RUN_STATE_CHANGED)
        .into_iter()
        .map(|s| s.new_state)
        .collect()
}

#[tokio::test]
async fn an_answer_with_no_tools_ends_the_turn_and_the_receipt_names_the_model_that_served() {
    let mut r = run("answer", None);
    let model = Scripted::new(vec![say(
        "Nothing to change.",
        "deepseek/deepseek-v4.1-flash",
    )]);
    let turn = r
        .engine
        .run_agent_turn(&model, &[], "anything to do?", MAX_ROUNDS)
        .await
        .expect("turn");
    assert_eq!(turn.rounds, 1);
    assert_eq!(turn.text, "Nothing to change.");
    assert_eq!(turn.prompt_tokens, Some(10));

    let receipt = r.engine.finalize_run().await.expect("receipt");
    // The lane alias was asked for; the response named what served.
    assert_eq!(
        receipt.model_selection.model_sku,
        "deepseek/deepseek-v4.1-flash"
    );
    assert_eq!(receipt.model_selection.provider, "scripted");
    assert_eq!(
        receipt.model_selection.routing_reason,
        "runtime_lane:vitna/code"
    );
    assert_eq!(receipt.completion_state, "completed_with_evidence");
    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&r.public_key)).expect("verify");
    assert!(
        report.is_valid && report.signature_verified,
        "{:?}",
        report.errors
    );

    let ev = events(&r.store, "run-answer");
    assert_eq!(states(&ev), vec!["running_model"]);
    let texts: Vec<String> =
        payloads::<vitna_protocol::messages::MessageDelta>(&ev, type_url::event::MESSAGE_DELTA)
            .into_iter()
            .map(|m| format!("{}:{}", m.role, m.delta_text))
            .collect();
    assert_eq!(
        texts,
        vec!["user:anything to do?", "model:Nothing to change."]
    );
}

#[tokio::test]
async fn a_read_runs_without_asking_and_its_result_reaches_the_model_by_call_id() {
    let op = Arc::new(Operator {
        answer: ApprovalDecision::Approved,
        asked: Mutex::new(Vec::new()),
    });
    let mut r = run("read", Some(op.clone()));
    let model = Scripted::new(vec![
        call("c1", "read_file", json!({ "path": "README.md" }), "m"),
        say("It says hello.", "m"),
    ]);
    let turn = r
        .engine
        .run_agent_turn(&model, &[], "what does the readme say?", MAX_ROUNDS)
        .await
        .expect("turn");
    assert_eq!(turn.tools_run, vec!["read_file"]);
    assert!(
        op.asked.lock().unwrap().is_empty(),
        "a read asked for approval"
    );

    let seen = model.seen.lock().unwrap();
    let second = &seen[1].messages;
    let result = second
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("c1"))
        .expect("result by id");
    assert!(result.content.contains("hello"), "{}", result.content);
    assert_eq!(
        second[1].tool_calls[0].id, "c1",
        "the assistant's call is echoed back before its result"
    );

    let ev = events(&r.store, "run-read");
    let proposed: Vec<ToolProposed> = payloads(&ev, type_url::event::TOOL_PROPOSED);
    assert_eq!(proposed[0].effect_class, "read");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&proposed[0].raw_arguments_json).unwrap(),
        json!({ "path": "README.md" })
    );
    let finished: Vec<ToolFinished> = payloads(&ev, type_url::event::TOOL_FINISHED);
    assert_eq!(finished[0].status, "completed");
    assert_eq!(
        states(&ev),
        vec!["running_model", "running_tool", "running_model"]
    );
}

#[tokio::test]
async fn a_write_waits_for_the_operator_and_the_request_carries_the_digest_it_binds_to() {
    let op = Arc::new(Operator {
        answer: ApprovalDecision::Approved,
        asked: Mutex::new(Vec::new()),
    });
    let mut r = run("write", Some(op.clone()));
    let args = json!({ "path": "notes.txt", "content": "written\n" });
    let model = Scripted::new(vec![
        call("c1", "write_file", args.clone(), "m"),
        say("Done.", "m"),
    ]);
    r.engine
        .run_agent_turn(&model, &[], "write a note", MAX_ROUNDS)
        .await
        .expect("turn");

    assert_eq!(
        fs::read_to_string(r.root.join("notes.txt")).unwrap(),
        "written\n"
    );
    let asked = op.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].action_digest,
        vitna_tools::action_digest("write_file", &args)
    );
    assert_eq!(asked[0].tool_call_id, "c1");
    assert!(
        asked[0].timeout_ms > 0,
        "a request with no deadline holds a run forever"
    );

    let ev = events(&r.store, "run-write");
    let requested: Vec<ApprovalRequested> = payloads(&ev, type_url::event::APPROVAL_REQUESTED);
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0].approval_id, asked[0].approval_id);
    assert_eq!(requested[0].action_digest, asked[0].action_digest);
    assert_eq!(requested[0].description, "write_file: notes.txt");
    assert_eq!(
        requested[0].timeout_ms,
        vitna_orchestration::APPROVAL_TIMEOUT_MS
    );
    assert_eq!(
        states(&ev),
        vec![
            "running_model",
            "waiting_for_approval",
            "running_tool",
            "running_model"
        ]
    );
    assert!(
        ev.iter()
            .any(|e| e.type_url == type_url::event::DIFF_CHANGED),
        "a write shows its diff"
    );

    let receipt = r.engine.finalize_run().await.expect("receipt");
    assert_eq!(receipt.changeset.files_modified[0].path, "notes.txt");
}

#[tokio::test]
async fn a_refusal_leaves_the_workspace_untouched_and_the_model_is_told_why() {
    let op = Arc::new(Operator {
        answer: ApprovalDecision::Rejected("the operator said no: wrong file".to_string()),
        asked: Mutex::new(Vec::new()),
    });
    let mut r = run("refuse", Some(op));
    let model = Scripted::new(vec![
        call(
            "c1",
            "write_file",
            json!({ "path": "README.md", "content": "gone\n" }),
            "m",
        ),
        say("Understood, I left it alone.", "m"),
    ]);
    let turn = r
        .engine
        .run_agent_turn(&model, &[], "overwrite the readme", MAX_ROUNDS)
        .await
        .expect("turn");
    assert!(turn.tools_run.is_empty(), "a refused tool did not run");
    assert_eq!(
        fs::read_to_string(r.root.join("README.md")).unwrap(),
        "hello\n"
    );

    let seen = model.seen.lock().unwrap().clone();
    let result = seen[1]
        .messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("c1"))
        .unwrap();
    assert!(result.content.contains("wrong file"), "{}", result.content);

    let ev = events(&r.store, "run-refuse");
    let finished: Vec<ToolFinished> = payloads(&ev, type_url::event::TOOL_FINISHED);
    assert_eq!(finished[0].status, "cancelled");
    assert!(
        !ev.iter()
            .any(|e| e.type_url == type_url::event::TOOL_STARTED),
        "a refused tool never starts"
    );
    let receipt = r.engine.finalize_run().await.expect("receipt");
    assert!(receipt.changeset.files_modified.is_empty());
}

/// The rule-only path is unchanged: no approver and no auto-approve refuses,
/// and the loop reports it to the model instead of aborting the turn. The
/// request is still recorded, as every request is, with no deadline, because
/// nobody is waiting on it.
#[tokio::test]
async fn with_no_approver_a_write_is_refused_by_rule_and_nobody_is_shown_as_asked() {
    let mut r = run("rule", None);
    let model = Scripted::new(vec![
        call(
            "c1",
            "write_file",
            json!({ "path": "x.txt", "content": "x" }),
            "m",
        ),
        say("Could not write.", "m"),
    ]);
    r.engine
        .run_agent_turn(&model, &[], "write x", MAX_ROUNDS)
        .await
        .expect("turn");
    assert!(!r.root.join("x.txt").exists());
    let ev = events(&r.store, "run-rule");
    let requested: Vec<ApprovalRequested> = payloads(&ev, type_url::event::APPROVAL_REQUESTED);
    assert_eq!(requested.len(), 1);
    assert_eq!(
        requested[0].timeout_ms, 0,
        "a request nobody waits on declares no deadline"
    );
    assert!(!states(&ev).contains(&"waiting_for_approval".to_string()));
}

#[tokio::test]
async fn a_turn_that_reaches_the_round_limit_is_signed_failed_and_says_why() {
    let mut r = run("limit", None);
    let model = Scripted::new(
        (0..3)
            .map(|i| call(&format!("c{i}"), "list_dir", json!({ "path": "." }), "m"))
            .collect(),
    );
    let turn = r
        .engine
        .run_agent_turn(&model, &[], "keep looking", 3)
        .await
        .expect("turn");
    assert!(turn.hit_round_limit);
    assert_eq!(turn.rounds, 3);
    let receipt = r.engine.finalize_run().await.expect("receipt");
    assert_eq!(receipt.completion_state, "failed");
    assert!(receipt
        .evidence_items
        .iter()
        .any(|e| e.description.contains("round limit of 3")));
    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&r.public_key)).expect("verify");
    assert!(report.is_valid, "{:?}", report.errors);
}

#[tokio::test]
async fn a_provider_failure_ends_the_turn_with_its_reason_and_the_receipt_is_still_signed() {
    let op = Arc::new(Operator {
        answer: ApprovalDecision::Approved,
        asked: Mutex::new(Vec::new()),
    });
    let mut r = run("down", Some(op));
    let model = Scripted::new(vec![
        call(
            "c1",
            "write_file",
            json!({ "path": "half.txt", "content": "1" }),
            "m",
        ),
        Err("connection refused".to_string()),
    ]);
    let err = r
        .engine
        .run_agent_turn(&model, &[], "two steps", MAX_ROUNDS)
        .await
        .expect_err("fails");
    assert!(err.contains("connection refused"), "{err}");
    // The first round changed the workspace, so the receipt has to exist.
    let receipt = r.engine.finalize_run().await.expect("receipt");
    assert_eq!(receipt.completion_state, "failed");
    assert_eq!(receipt.changeset.files_modified[0].path, "half.txt");
    let ev = events(&r.store, "run-down");
    assert!(ev.iter().any(|e| e.type_url == type_url::event::DIAGNOSTIC));
}

#[tokio::test]
async fn an_invented_tool_is_reported_and_the_turn_goes_on() {
    let mut r = run("invent", None);
    let model = Scripted::new(vec![
        call("c1", "deploy_to_prod", json!({}), "m"),
        say("No such tool.", "m"),
    ]);
    let turn = r
        .engine
        .run_agent_turn(&model, &[], "ship it", MAX_ROUNDS)
        .await
        .expect("turn");
    assert_eq!(turn.rounds, 2);
    let seen = model.seen.lock().unwrap();
    let result = seen[1]
        .messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("c1"))
        .unwrap();
    assert!(result.content.contains("did not run"), "{}", result.content);
    let ev = events(&r.store, "run-invent");
    let diag: Vec<vitna_protocol::messages::Diagnostic> =
        payloads(&ev, type_url::event::DIAGNOSTIC);
    assert_eq!(diag[0].code, "unknown_tool");
}

#[tokio::test]
async fn a_turn_two_models_served_names_both_rather_than_either() {
    let mut r = run("mixed", None);
    let model = Scripted::new(vec![
        call(
            "c1",
            "list_dir",
            json!({ "path": "." }),
            "deepseek/deepseek-v4.1-flash",
        ),
        say("Done.", "anthropic/claude-sonnet-5"),
    ]);
    let turn = r
        .engine
        .run_agent_turn(&model, &[], "look", MAX_ROUNDS)
        .await
        .expect("turn");
    assert_eq!(
        turn.served_models,
        vec!["deepseek/deepseek-v4.1-flash", "anthropic/claude-sonnet-5"]
    );
    let receipt = r.engine.finalize_run().await.expect("receipt");
    assert_eq!(
        receipt.model_selection.model_sku,
        "deepseek/deepseek-v4.1-flash,anthropic/claude-sonnet-5"
    );
    assert!(receipt
        .model_selection
        .routing_reason
        .contains("more than one model"));
}

#[tokio::test]
async fn the_session_so_far_is_sent_before_this_turns_prompt() {
    let mut r = run("prior", None);
    let model = Scripted::new(vec![say("Yes.", "m")]);
    let prior = vec![
        ProviderMessage::user("first question"),
        ProviderMessage::assistant("first answer", vec![]),
    ];
    r.engine
        .run_agent_turn(&model, &prior, "and the second?", MAX_ROUNDS)
        .await
        .expect("turn");
    let seen = model.seen.lock().unwrap();
    let sent: Vec<&str> = seen[0]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        sent,
        vec!["first question", "first answer", "and the second?"]
    );
}
