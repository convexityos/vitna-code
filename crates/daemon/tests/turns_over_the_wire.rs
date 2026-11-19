//! A turn driven entirely through the declared wire (ADR-0007):
//! `CreateSession`, `SubscribeEvents`, `SubmitTurn`, then `ApproveAction` or
//! `RejectAction` answering the `ApprovalRequested` the turn sends.
//!
//! The connection is an in-memory duplex stream handed to the same
//! `serve_connection` a socket gets, so these tests need no endpoint and run
//! the same on every platform. The model is scripted; a test that needed a
//! real key would be skipped in CI and prove nothing.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf,
};
use vitna_daemon::{ipc, DaemonServer, ModelRoute};
use vitna_protocol::messages::{
    ApprovalRequested, ErrorCode, ErrorResponse, HandshakeRequest, RunStateChanged, SessionCreated,
    ToolFinished,
};
use vitna_protocol::{type_url, ProtocolEnvelope, PROTOCOL_VERSION_MAJOR};
use vitna_providers::{
    Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall,
};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

const PATIENCE: Duration = Duration::from_secs(10);
static NEXT: AtomicU64 = AtomicU64::new(0);

fn folder(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitna_wire_turn_{tag}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("folder");
    std::fs::write(dir.join("README.md"), "hello\n").expect("seed");
    dir
}

struct Scripted {
    replies: Mutex<VecDeque<ProviderResponse>>,
    seen: Mutex<Vec<ProviderRequest>>,
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
            .ok_or_else(|| "the script ran out".to_string())
    }
}

fn write_then_done(path: &str) -> Vec<ProviderResponse> {
    vec![
        ProviderResponse {
            tool_calls: vec![ProviderToolCall {
                id: "c1".into(),
                name: "write_file".into(),
                arguments: json!({ "path": path, "content": "written\n" }),
            }],
            finish_reason: "tool_calls".into(),
            model: Some("deepseek/deepseek-v4.1-flash".into()),
            ..Default::default()
        },
        ProviderResponse {
            content: "Done.".into(),
            finish_reason: "stop".into(),
            model: Some("deepseek/deepseek-v4.1-flash".into()),
            ..Default::default()
        },
    ]
}

fn say(text: &str) -> ProviderResponse {
    ProviderResponse {
        content: text.into(),
        finish_reason: "stop".into(),
        model: Some("m".into()),
        ..Default::default()
    }
}

struct Wire {
    daemon: Arc<DaemonServer>,
    model: Arc<Scripted>,
    read: ReadHalf<DuplexStream>,
    write: WriteHalf<DuplexStream>,
    root: PathBuf,
}

async fn connect(
    tag: &str,
    script: Option<Vec<ProviderResponse>>,
    timeout_ms: Option<u64>,
) -> Wire {
    let root = folder(tag);
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(root.join("runner.journal")).expect("runner"),
    ));
    let mut daemon = DaemonServer::new(
        EventStore::open_in_memory().expect("store"),
        runner,
        vitna_receipts::generate_signing_key(),
    );
    let model = Arc::new(Scripted {
        replies: Mutex::new(script.clone().unwrap_or_default().into()),
        seen: Mutex::new(Vec::new()),
    });
    if script.is_some() {
        daemon = daemon.with_route(ModelRoute {
            provider: model.clone(),
            model: "vitna/code".into(),
            routing_reason: "runtime_lane:vitna/code".into(),
        });
    }
    if let Some(ms) = timeout_ms {
        daemon.approval_timeout_ms = ms;
    }
    let daemon = Arc::new(daemon);
    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(ipc::serve_connection(server, daemon.clone()));
    let (read, write) = tokio::io::split(client);
    let mut wire = Wire {
        daemon,
        model,
        read,
        write,
        root,
    };

    wire.send(
        type_url::envelope::HANDSHAKE_REQUEST,
        "",
        &HandshakeRequest {
            min_supported_version: PROTOCOL_VERSION_MAJOR,
            max_supported_version: PROTOCOL_VERSION_MAJOR,
            client_identifier: "turns-over-the-wire".into(),
        },
    )
    .await;
    assert_eq!(
        wire.recv().await.type_url,
        type_url::envelope::HANDSHAKE_RESPONSE
    );
    wire
}

impl Wire {
    async fn send<T: serde::Serialize>(&mut self, url: &str, session_id: &str, body: &T) {
        let envelope =
            ProtocolEnvelope::carrying(url, session_id, "", 1, "k", body).expect("encode");
        send_frame(&mut self.write, &envelope).await;
    }

    async fn recv(&mut self) -> ProtocolEnvelope {
        recv_frame(&mut self.read).await
    }

    /// Reads until `pick` accepts a frame, returning it and everything before it.
    async fn until(
        &mut self,
        mut pick: impl FnMut(&ProtocolEnvelope) -> bool,
    ) -> (ProtocolEnvelope, Vec<ProtocolEnvelope>) {
        let mut before = Vec::new();
        loop {
            let frame = self.recv().await;
            if pick(&frame) {
                return (frame, before);
            }
            before.push(frame);
        }
    }

    async fn open_session(&mut self) -> String {
        let path = self.root.to_string_lossy().into_owned();
        self.send(
            type_url::command::CREATE_SESSION,
            "",
            &json!({ "workspace_path": path, "initial_mode": "build" }),
        )
        .await;
        let reply = self.recv().await;
        assert_eq!(reply.type_url, type_url::envelope::SESSION_CREATED);
        assert_eq!(
            reply.sequence, 0,
            "a reply travels at the control sequence, outside the event stream"
        );
        let created: SessionCreated = reply.payload_as().expect("SessionCreated");
        assert!(!created.session_id.is_empty());
        assert_eq!(reply.session_id, created.session_id);
        assert_eq!(created.initial_mode, "build");
        self.send(
            type_url::command::SUBSCRIBE_EVENTS,
            &created.session_id,
            &json!({ "session_id": created.session_id, "resume_after_sequence": 0 }),
        )
        .await;
        created.session_id
    }

    async fn submit(&mut self, session: &str, prompt: &str) {
        self.send(
            type_url::command::SUBMIT_TURN,
            session,
            &json!({ "session_id": session, "prompt": prompt }),
        )
        .await;
    }

    async fn approval(&mut self) -> ApprovalRequested {
        let (frame, _) = self
            .until(|f| f.type_url == type_url::event::APPROVAL_REQUESTED)
            .await;
        frame.payload_as().expect("ApprovalRequested")
    }

    async fn final_state(&mut self) -> (RunStateChanged, Vec<ProtocolEnvelope>) {
        let (frame, before) = self
            .until(|f| {
                f.type_url == type_url::event::RUN_STATE_CHANGED
                    && f.payload_as::<RunStateChanged>()
                        .map(|s| s.new_state == "completed" || s.new_state == "failed")
                        .unwrap_or(false)
            })
            .await;
        (frame.payload_as().expect("state"), before)
    }
}

async fn send_frame<W: AsyncWrite + Unpin>(w: &mut W, envelope: &ProtocolEnvelope) {
    let bytes = envelope.encode_frame().expect("frame");
    w.write_all(&bytes).await.expect("write");
    w.flush().await.expect("flush");
}

async fn recv_frame<R: AsyncRead + Unpin>(r: &mut R) -> ProtocolEnvelope {
    let mut len = [0u8; 4];
    tokio::time::timeout(PATIENCE, r.read_exact(&mut len))
        .await
        .expect("a frame should have arrived before the deadline")
        .expect("read length");
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    r.read_exact(&mut buf).await.expect("read body");
    serde_json::from_slice(&buf).expect("decode envelope")
}

fn error_of(frame: &ProtocolEnvelope) -> Option<ErrorResponse> {
    (frame.type_url == type_url::envelope::ERROR_RESPONSE)
        .then(|| frame.payload_as().expect("ErrorResponse"))
}

#[tokio::test]
async fn a_turn_runs_over_the_wire_and_waits_for_the_operators_answer() {
    let mut wire = connect("approve", Some(write_then_done("notes.txt")), None).await;
    let session = wire.open_session().await;
    wire.submit(&session, "write a note").await;

    let asked = wire.approval().await;
    assert_eq!(asked.description, "write_file: notes.txt");
    assert!(
        !wire.root.join("notes.txt").exists(),
        "nothing is written before the answer"
    );

    // A yes for a different action is not a yes for this one, and the
    // request keeps waiting.
    wire.send(type_url::command::APPROVE_ACTION, &session, &json!({ "approval_id": asked.approval_id, "action_digest": "0".repeat(64), "scope": "once" }))
        .await;
    let (refusal, _) = wire
        .until(|f| f.type_url == type_url::envelope::ERROR_RESPONSE)
        .await;
    let refusal = error_of(&refusal).unwrap();
    assert_eq!(refusal.code, ErrorCode::PolicyDenied);
    assert_eq!(
        refusal.action_id, asked.approval_id,
        "the refusal is pinned to the approval it answers"
    );

    wire.send(type_url::command::APPROVE_ACTION, &session, &json!({ "approval_id": asked.approval_id, "action_digest": asked.action_digest, "scope": "once" }))
        .await;
    let (done, events) = wire.final_state().await;
    assert_eq!(done.new_state, "completed", "{}", done.reason);
    assert_eq!(
        std::fs::read_to_string(wire.root.join("notes.txt")).unwrap(),
        "written\n"
    );
    assert!(events
        .iter()
        .any(|f| f.type_url == type_url::event::MESSAGE_DELTA));
    assert!(events
        .iter()
        .any(|f| f.type_url == type_url::event::TOOL_FINISHED));

    let receipt_path = wire
        .root
        .join(".vitna")
        .join("receipts")
        .join(format!("{}.json", done.run_id));
    let report =
        ReceiptVerifier::verify_file(&receipt_path, Some(&wire.daemon.device_public_key()))
            .expect("receipt");
    assert!(
        report.is_valid && report.signature_verified,
        "{:?}",
        report.errors
    );
    let receipt: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&receipt_path).unwrap()).unwrap();
    assert_eq!(
        receipt["model_selection"]["model_sku"],
        "deepseek/deepseek-v4.1-flash"
    );
    assert_eq!(receipt["model_selection"]["provider"], "scripted");
    assert_eq!(
        receipt["model_selection"]["routing_reason"],
        "runtime_lane:vitna/code"
    );
}

#[tokio::test]
async fn a_refused_action_is_not_taken_and_the_turn_still_ends() {
    let mut wire = connect("reject", Some(write_then_done("README.md")), None).await;
    let session = wire.open_session().await;
    wire.submit(&session, "overwrite the readme").await;
    let asked = wire.approval().await;
    wire.send(type_url::command::REJECT_ACTION, &session, &json!({ "approval_id": asked.approval_id, "action_digest": asked.action_digest, "reason": "keep it" }))
        .await;
    let (done, events) = wire.final_state().await;
    assert_eq!(done.new_state, "completed");
    assert_eq!(
        std::fs::read_to_string(wire.root.join("README.md")).unwrap(),
        "hello\n"
    );
    let finished: Vec<ToolFinished> = events
        .iter()
        .filter(|f| f.type_url == type_url::event::TOOL_FINISHED)
        .map(|f| f.payload_as().unwrap())
        .collect();
    assert_eq!(finished[0].status, "cancelled");
    let told = &wire.model.seen.lock().unwrap()[1];
    assert!(
        told.messages.iter().any(|m| m.content.contains("keep it")),
        "the model is told why"
    );
}

#[tokio::test]
async fn without_a_route_a_turn_is_refused_by_saying_how_to_give_one() {
    let mut wire = connect("noroute", None, None).await;
    let session = wire.open_session().await;
    wire.submit(&session, "anything").await;
    let (frame, _) = wire
        .until(|f| f.type_url == type_url::envelope::ERROR_RESPONSE)
        .await;
    let refusal = error_of(&frame).unwrap();
    assert!(
        refusal.message.contains("--runtime-url"),
        "{}",
        refusal.message
    );
}

#[tokio::test]
async fn an_answer_with_nothing_waiting_is_refused() {
    let mut wire = connect("nothing", Some(vec![]), None).await;
    let session = wire.open_session().await;
    wire.send(
        type_url::command::APPROVE_ACTION,
        &session,
        &json!({ "approval_id": "apr-none", "action_digest": "x", "scope": "once" }),
    )
    .await;
    let (frame, _) = wire
        .until(|f| f.type_url == type_url::envelope::ERROR_RESPONSE)
        .await;
    let refusal = error_of(&frame).unwrap();
    assert_eq!(refusal.code, ErrorCode::PolicyDenied);
    assert!(
        refusal.message.contains("nothing is waiting"),
        "{}",
        refusal.message
    );
}

#[tokio::test]
async fn a_wider_scope_is_refused_and_the_request_keeps_waiting_for_a_once() {
    let mut wire = connect("scope", Some(write_then_done("a.txt")), None).await;
    let session = wire.open_session().await;
    wire.submit(&session, "write a").await;
    let asked = wire.approval().await;
    wire.send(type_url::command::APPROVE_ACTION, &session, &json!({ "approval_id": asked.approval_id, "action_digest": asked.action_digest, "scope": "session" }))
        .await;
    let (frame, _) = wire
        .until(|f| f.type_url == type_url::envelope::ERROR_RESPONSE)
        .await;
    assert!(error_of(&frame).unwrap().message.contains("only scope"));
    assert!(!wire.root.join("a.txt").exists());

    wire.send(
        type_url::command::APPROVE_ACTION,
        &session,
        &json!({ "approval_id": asked.approval_id, "action_digest": asked.action_digest }),
    )
    .await;
    let (done, _) = wire.final_state().await;
    assert_eq!(done.new_state, "completed");
    assert!(wire.root.join("a.txt").exists(), "an empty scope is once");
}

#[tokio::test]
async fn an_approval_nobody_answers_is_refused_when_its_time_runs_out() {
    let mut wire = connect("timeout", Some(write_then_done("late.txt")), Some(300)).await;
    let session = wire.open_session().await;
    wire.submit(&session, "write late").await;
    let asked = wire.approval().await;
    assert_eq!(
        asked.timeout_ms, 300,
        "the request declares the deadline it is held to"
    );
    let (done, _) = wire.final_state().await;
    assert_eq!(done.new_state, "completed");
    assert!(!wire.root.join("late.txt").exists());
    let told = &wire.model.seen.lock().unwrap()[1];
    assert!(
        told.messages
            .iter()
            .any(|m| m.content.contains("nobody answered")),
        "the model is told it timed out"
    );
    assert!(
        wire.daemon.approvals.lock().unwrap().is_empty(),
        "a timed-out request is not left waiting"
    );
}

#[tokio::test]
async fn a_second_turn_while_one_runs_is_refused() {
    let mut wire = connect("busy", Some(write_then_done("b.txt")), None).await;
    let session = wire.open_session().await;
    wire.submit(&session, "write b").await;
    let asked = wire.approval().await;
    wire.submit(&session, "and something else").await;
    let (frame, _) = wire
        .until(|f| f.type_url == type_url::envelope::ERROR_RESPONSE)
        .await;
    assert!(error_of(&frame)
        .unwrap()
        .message
        .contains("already running"));
    wire.send(
        type_url::command::REJECT_ACTION,
        &session,
        &json!({ "approval_id": asked.approval_id, "action_digest": asked.action_digest }),
    )
    .await;
    assert_eq!(wire.final_state().await.0.new_state, "completed");
}

#[tokio::test]
async fn a_sessions_second_turn_is_sent_its_first() {
    let mut wire = connect(
        "history",
        Some(vec![say("First answer."), say("Second answer.")]),
        None,
    )
    .await;
    let session = wire.open_session().await;
    wire.submit(&session, "first question").await;
    assert_eq!(wire.final_state().await.0.new_state, "completed");
    wire.submit(&session, "second question").await;
    assert_eq!(wire.final_state().await.0.new_state, "completed");
    let seen = wire.model.seen.lock().unwrap();
    let sent: Vec<&str> = seen[1]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect();
    assert_eq!(
        sent,
        vec!["first question", "First answer.", "second question"]
    );
}

#[tokio::test]
async fn a_session_for_a_folder_that_does_not_exist_is_refused() {
    let mut wire = connect("nofolder", None, None).await;
    let missing = wire.root.join("not-here");
    wire.send(
        type_url::command::CREATE_SESSION,
        "",
        &json!({ "workspace_path": missing.to_string_lossy() }),
    )
    .await;
    let refusal = error_of(&wire.recv().await).expect("an ErrorResponse");
    assert_eq!(refusal.code, ErrorCode::InvalidFrame);
}

#[tokio::test]
async fn attached_files_are_refused_rather_than_dropped() {
    let mut wire = connect("attach", Some(vec![say("x")]), None).await;
    let session = wire.open_session().await;
    wire.send(type_url::command::SUBMIT_TURN, &session, &json!({ "session_id": session, "prompt": "read it", "attached_file_paths": ["README.md"] }))
        .await;
    let (frame, _) = wire
        .until(|f| f.type_url == type_url::envelope::ERROR_RESPONSE)
        .await;
    assert!(error_of(&frame)
        .unwrap()
        .message
        .contains("attached_file_paths"));
    assert!(
        wire.model.seen.lock().unwrap().is_empty(),
        "no model was asked"
    );
}
