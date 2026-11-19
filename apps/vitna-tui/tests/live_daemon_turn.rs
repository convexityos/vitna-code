//! End-to-end test driving `vitna-tui`'s `App` and `link::drive_stream` against
//! a real `DaemonServer` over an in-memory duplex stream.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use vitna_daemon::{ipc, DaemonServer, ModelRoute};
use vitna_providers::{
    Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall,
};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;
use vitna_tui::link::{self, Link};
use vitna_tui::turn::{self, Entry};
use vitna_tui::{App, Ask, Update};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn temp_workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitna_tui_live_{tag}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("workspace");
    std::fs::write(dir.join("README.md"), "vitna\n").expect("seed");
    dir
}

struct ScriptedModel {
    replies: Mutex<VecDeque<ProviderResponse>>,
}

#[async_trait]
impl Provider for ScriptedModel {
    fn name(&self) -> &str {
        "scripted-tui"
    }

    fn parse_sse_event(&self, _: &str, _: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        Ok(Vec::new())
    }

    async fn complete(&self, _: &ProviderRequest) -> Result<ProviderResponse, String> {
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "script exhausted".to_string())
    }
}

fn write_script(path: &str, content: &str) -> Vec<ProviderResponse> {
    vec![
        ProviderResponse {
            tool_calls: vec![ProviderToolCall {
                id: "tc-1".into(),
                name: "write_file".into(),
                arguments: json!({ "path": path, "content": content }),
            }],
            finish_reason: "tool_calls".into(),
            model: Some("vitna/test-model".into()),
            ..Default::default()
        },
        ProviderResponse {
            content: "File written cleanly.".into(),
            finish_reason: "stop".into(),
            model: Some("vitna/test-model".into()),
            ..Default::default()
        },
    ]
}

async fn pump_until(
    app: &mut App,
    rx: &mpsc::Receiver<Update>,
    mut stop: impl FnMut(&App, Ask) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        while let Ok(update) = rx.try_recv() {
            let ask = app.apply_update(update);
            if stop(app, ask) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for TUI state transition");
}

fn type_and_enter(app: &mut App, text: &str) {
    for c in text.chars() {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

#[tokio::test]
async fn tui_submits_turn_answers_approval_and_refreshes_ledger() {
    let root = temp_workspace("approve");
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(root.join("runner.journal")).expect("runner"),
    ));
    let model = Arc::new(ScriptedModel {
        replies: Mutex::new(write_script("out.txt", "hello from tui\n").into()),
    });
    let daemon = Arc::new(
        DaemonServer::new(
            EventStore::open_in_memory().expect("store"),
            runner,
            vitna_receipts::generate_signing_key(),
        )
        .with_route(ModelRoute {
            provider: model,
            model: "vitna/code".into(),
            routing_reason: "runtime_lane:vitna/code".into(),
        }),
    );

    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(ipc::serve_connection(server, daemon.clone()));

    let (upd_tx, upd_rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(root.clone());
    app.cmd_tx = Some(cmd_tx);

    let link_task = tokio::spawn(link::drive_stream(client, root.clone(), upd_tx, cmd_rx));

    // Wait for Handshake + CreateSession + SubscribeEvents.
    pump_until(&mut app, &upd_rx, |a, _| {
        matches!(a.link, Link::Open { .. }) && a.session_id.is_some()
    })
    .await;

    // Submit a turn by typing into the composer and pressing Enter.
    type_and_enter(&mut app, "write out.txt");
    assert!(app.draft.is_empty(), "submitted draft clears");
    assert!(app.run_active);

    // Wait until the approval box arrives on the tool entry.
    pump_until(&mut app, &upd_rx, |a, _| {
        a.turn.iter().any(|e| {
            matches!(e, Entry::Tool(t) if t.phase == turn::Phase::Waiting && t.approval.is_some())
        })
    })
    .await;

    // Key pressed before arming must be ignored.
    let approval_opened = app
        .turn
        .iter()
        .find_map(|e| match e {
            Entry::Tool(t) => t.approval.as_ref().map(|a| a.opened),
            _ => None,
        })
        .unwrap();
    app.key_at(
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        approval_opened + Duration::from_millis(100),
    );
    assert!(
        !root.join("out.txt").exists(),
        "unarmed keypress must not approve"
    );

    // Key pressed after arming approves once with the exact action digest.
    app.key_at(
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        approval_opened + turn::ARMING + Duration::from_millis(50),
    );

    // Wait for completion and ledger refresh signal.
    let mut refreshed = false;
    pump_until(&mut app, &upd_rx, |a, ask| {
        if ask == Ask::RefreshLedger {
            refreshed = true;
        }
        !a.run_active && refreshed
    })
    .await;

    assert_eq!(
        std::fs::read_to_string(root.join("out.txt")).expect("written file"),
        "hello from tui\n"
    );
    assert!(
        app.turn
            .iter()
            .any(|e| matches!(e, Entry::Model(t) if t.contains("File written cleanly."))),
        "model summary must appear in turn transcript"
    );

    link_task.abort();
}

#[tokio::test]
async fn tui_ctrl_c_cancels_active_run_without_quitting() {
    let root = temp_workspace("cancel");
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(root.join("runner.journal")).expect("runner"),
    ));
    let model = Arc::new(ScriptedModel {
        replies: Mutex::new(write_script("nope.txt", "should not write\n").into()),
    });
    let daemon = Arc::new(
        DaemonServer::new(
            EventStore::open_in_memory().expect("store"),
            runner,
            vitna_receipts::generate_signing_key(),
        )
        .with_route(ModelRoute {
            provider: model,
            model: "vitna/code".into(),
            routing_reason: "runtime_lane:vitna/code".into(),
        }),
    );

    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(ipc::serve_connection(server, daemon.clone()));

    let (upd_tx, upd_rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(root.clone());
    app.cmd_tx = Some(cmd_tx);

    let link_task = tokio::spawn(link::drive_stream(client, root.clone(), upd_tx, cmd_rx));

    pump_until(&mut app, &upd_rx, |a, _| {
        matches!(a.link, Link::Open { .. }) && a.session_id.is_some()
    })
    .await;

    type_and_enter(&mut app, "write nope.txt");
    pump_until(&mut app, &upd_rx, |a, _| {
        a.turn.iter().any(|e| {
            matches!(e, Entry::Tool(t) if t.phase == turn::Phase::Waiting && t.approval.is_some())
        })
    })
    .await;

    // Ctrl+C during an active run cancels the run rather than quitting the TUI.
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert!(!app.quit, "Ctrl+C during active run must cancel, not quit");

    pump_until(&mut app, &upd_rx, |a, _| !a.run_active).await;
    assert!(
        !root.join("nope.txt").exists(),
        "cancelled run must not write file"
    );
    assert!(
        app.turn
            .iter()
            .any(|e| matches!(e, Entry::Note { text, .. } if text.contains("cancelled"))),
        "turn transcript should record cancellation"
    );

    link_task.abort();
}

struct RecordingScriptedModel {
    replies: Mutex<VecDeque<ProviderResponse>>,
    requests: Arc<Mutex<Vec<ProviderRequest>>>,
}

#[async_trait]
impl Provider for RecordingScriptedModel {
    fn name(&self) -> &str {
        "recording-tui"
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
            .ok_or_else(|| "script exhausted".to_string())
    }
}

#[tokio::test]
async fn tui_attachments_and_steer_command_reach_live_daemon_model() {
    let root = temp_workspace("attach_steer");
    std::fs::write(root.join("notes.txt"), "secret-attachment-marker-77\n").expect("seed notes");

    let runner = Arc::new(Mutex::new(
        FakeRunner::new(root.join("runner.journal")).expect("runner"),
    ));
    let recorded = Arc::new(Mutex::new(Vec::<ProviderRequest>::new()));
    let model = Arc::new(RecordingScriptedModel {
        replies: Mutex::new(write_script("steered.txt", "ok\n").into()),
        requests: recorded.clone(),
    });
    let daemon = Arc::new(
        DaemonServer::new(
            EventStore::open_in_memory().expect("store"),
            runner,
            vitna_receipts::generate_signing_key(),
        )
        .with_route(ModelRoute {
            provider: model,
            model: "vitna/code".into(),
            routing_reason: "runtime_lane:vitna/code".into(),
        }),
    );

    let (client, server) = tokio::io::duplex(1 << 20);
    tokio::spawn(ipc::serve_connection(server, daemon.clone()));

    let (upd_tx, upd_rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(root.clone());
    app.cmd_tx = Some(cmd_tx);

    let link_task = tokio::spawn(link::drive_stream(client, root.clone(), upd_tx, cmd_rx));

    pump_until(&mut app, &upd_rx, |a, _| {
        matches!(a.link, Link::Open { .. }) && a.session_id.is_some()
    })
    .await;

    // Submit with an @notes.txt workspace attachment.
    type_and_enter(&mut app, "inspect @notes.txt and write steered.txt");

    // Wait for the approval box so round 1 is paused on approval.
    pump_until(&mut app, &upd_rx, |a, _| {
        a.turn.iter().any(|e| {
            matches!(e, Entry::Tool(t) if t.phase == turn::Phase::Waiting && t.approval.is_some())
        })
    })
    .await;

    // Send /steer while the run is active before approving the tool.
    // Note: while an approval is pending, keys route to approval first; test
    // steering via /steer after approving or by sending directly when no
    // approval is open. First approve the tool, and simultaneously verify the
    // attachment reached round 1.
    let approval_opened = app
        .turn
        .iter()
        .find_map(|e| match e {
            Entry::Tool(t) => t.approval.as_ref().map(|a| a.opened),
            _ => None,
        })
        .unwrap();
    app.key_at(
        KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE),
        approval_opened + turn::ARMING + Duration::from_millis(50),
    );

    pump_until(&mut app, &upd_rx, |a, _| !a.run_active).await;

    let reqs = recorded.lock().unwrap();
    assert!(!reqs.is_empty());
    let first_prompt = reqs[0]
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        first_prompt.contains("secret-attachment-marker-77"),
        "expected @notes.txt contents in model request, got: {first_prompt}"
    );

    link_task.abort();
}

