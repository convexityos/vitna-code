//! Drives the real transport: a real listener, a real client socket, real
//! length-prefixed frames.
//!
//! These are not unit tests of a dispatch function. The thing most likely to
//! be wrong about a wire is the wire, and a test that calls `dispatch()`
//! directly proves nothing about whether anything was ever bound, framed or
//! flushed.
//!
//! What they do not cover: the Windows pipe's DACL, which is the system
//! default, and the Unix socket's 0600 mode, which is asserted on Unix only
//! because there is nothing equivalent to assert on Windows.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use vitna_daemon::DaemonServer;
use vitna_protocol::messages::{
    ErrorCode, ErrorResponse, HandshakeRequest, HandshakeResponse, SubscribeEvents,
};
use vitna_protocol::{type_url, ProtocolEnvelope, PROTOCOL_VERSION_MAJOR};
use vitna_runner::FakeRunner;
use vitna_store::{EventRecord, EventStore, GENESIS_HASH};

/// Every wait in this file is bounded, so a wire that never answers fails the
/// test instead of hanging the suite.
const PATIENCE: Duration = Duration::from_secs(10);

fn unique(tag: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{tag}-{}-{n}", std::process::id())
}

#[cfg(windows)]
fn endpoint_for(tag: &str) -> String {
    format!(r"\\.\pipe\vitna-test-{}", unique(tag))
}

#[cfg(unix)]
fn endpoint_for(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("vitna-ipc-{}", unique(tag)));
    std::fs::create_dir_all(&dir).expect("create socket dir");
    dir.join("vitna.sock").to_string_lossy().to_string()
}

#[cfg(windows)]
async fn connect(endpoint: &str) -> impl AsyncRead + AsyncWrite + Unpin {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .expect("connect to the pipe")
}

#[cfg(unix)]
async fn connect(endpoint: &str) -> impl AsyncRead + AsyncWrite + Unpin {
    tokio::net::UnixStream::connect(endpoint)
        .await
        .expect("connect to the socket")
}

async fn send<W: AsyncWrite + Unpin, T: serde::Serialize>(
    w: &mut W,
    type_url: &str,
    session_id: &str,
    body: &T,
) {
    let envelope =
        ProtocolEnvelope::carrying(type_url, session_id, "", 1, "test-key", body).expect("encode");
    let bytes = envelope.encode_frame().expect("frame");
    w.write_all(&bytes).await.expect("write");
    w.flush().await.expect("flush");
}

async fn recv<R: AsyncRead + Unpin>(r: &mut R) -> ProtocolEnvelope {
    let mut len = [0u8; 4];
    tokio::time::timeout(PATIENCE, r.read_exact(&mut len))
        .await
        .expect("a frame should have arrived before the deadline")
        .expect("read length");
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    tokio::time::timeout(PATIENCE, r.read_exact(&mut buf))
        .await
        .expect("frame body should have arrived")
        .expect("read body");
    serde_json::from_slice(&buf).expect("decode envelope")
}

/// A daemon listening on its own endpoint, with one session.
struct Fixture {
    endpoint: String,
    daemon: Arc<DaemonServer>,
    session_id: String,
    run_id: String,
}

async fn start(tag: &str) -> Fixture {
    let workspace = std::env::temp_dir().join(unique("vitna-ws"));
    std::fs::create_dir_all(&workspace).expect("create workspace");

    let store = EventStore::open_in_memory().expect("open store");
    let runner = FakeRunner::new(workspace.join("runner.journal")).expect("open runner");
    let daemon = Arc::new(DaemonServer::new(
        store,
        Arc::new(std::sync::Mutex::new(runner)),
        vitna_receipts::generate_signing_key(),
    ));

    let session = daemon.create_session(&workspace).expect("create session");
    let run_id = format!("run-{}", unique("t"));

    // `latest_run_id` is set by run_task, which needs a provider. A
    // subscription only needs the session to name a run, so the test names one
    // directly rather than running a turn to get one.
    {
        let mut sessions = daemon.sessions.lock().expect("lock sessions");
        if let Some(s) = sessions.get_mut(&session.session_id) {
            s.latest_run_id = Some(run_id.clone());
        }
    }

    let endpoint = endpoint_for(tag);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    {
        let daemon = Arc::clone(&daemon);
        let endpoint = endpoint.clone();
        tokio::spawn(async move {
            let _ = vitna_daemon::ipc::serve(daemon, &endpoint, move || {
                let _ = ready_tx.send(());
            })
            .await;
        });
    }
    tokio::time::timeout(PATIENCE, ready_rx)
        .await
        .expect("the listener should have bound")
        .expect("ready signal");

    Fixture {
        endpoint,
        daemon,
        session_id: session.session_id,
        run_id,
    }
}

/// Appends `count` events to the fixture's run, numbered from
/// `FIRST_EVENT_SEQUENCE`, chained as the engine chains them.
fn append_events(fx: &Fixture, count: u64) {
    let store = fx.daemon.store.lock().expect("lock store");
    let mut prev = GENESIS_HASH.to_string();
    for i in 0..count {
        let sequence = vitna_protocol::FIRST_EVENT_SEQUENCE + i;
        let event = EventRecord::new(
            format!("ev-{}-{sequence}", fx.run_id),
            &fx.run_id,
            sequence,
            type_url::event::DIAGNOSTIC,
            1_000 + sequence,
            serde_json::to_vec(&serde_json::json!({ "message": format!("event {sequence}") }))
                .expect("payload"),
            &prev,
        );
        prev = event.event_hash.clone();
        store.append_event(&event).expect("append");
    }
}

async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) -> HandshakeResponse {
    send(
        stream,
        type_url::envelope::HANDSHAKE_REQUEST,
        "",
        &HandshakeRequest {
            min_supported_version: 1,
            max_supported_version: 1,
            client_identifier: "ipc-test/0.1.0".into(),
        },
    )
    .await;
    let reply = recv(stream).await;
    assert_eq!(reply.type_url, type_url::envelope::HANDSHAKE_RESPONSE);
    reply.payload_as().expect("handshake response decodes")
}

#[tokio::test]
async fn a_client_reaches_the_daemon_and_completes_the_declared_handshake() {
    let fx = start("handshake").await;
    let mut stream = connect(&fx.endpoint).await;

    let response = handshake(&mut stream).await;
    assert!(
        response.accepted,
        "handshake refused: {}",
        response.rejection_reason
    );
    assert_eq!(response.selected_version_major, PROTOCOL_VERSION_MAJOR);
}

#[tokio::test]
async fn a_version_this_daemon_cannot_speak_is_refused_with_a_reason() {
    let fx = start("version").await;
    let mut stream = connect(&fx.endpoint).await;

    send(
        &mut stream,
        type_url::envelope::HANDSHAKE_REQUEST,
        "",
        &HandshakeRequest {
            min_supported_version: 7,
            max_supported_version: 9,
            client_identifier: "from-the-future/1.0".into(),
        },
    )
    .await;

    let response: HandshakeResponse = recv(&mut stream).await.payload_as().expect("decode");
    assert!(!response.accepted);
    assert!(
        !response.rejection_reason.is_empty(),
        "a refusal must say why, or a client cannot tell it from an absent daemon"
    );
}

#[tokio::test]
async fn a_command_before_the_handshake_is_refused_and_the_connection_survives() {
    let fx = start("unauth").await;
    let mut stream = connect(&fx.endpoint).await;

    send(
        &mut stream,
        type_url::command::SUBSCRIBE_EVENTS,
        &fx.session_id,
        &SubscribeEvents {
            session_id: fx.session_id.clone(),
            resume_after_sequence: 0,
        },
    )
    .await;

    let refusal: ErrorResponse = recv(&mut stream).await.payload_as().expect("decode");
    assert_eq!(refusal.code, ErrorCode::Unauthorized);

    // The connection stays open, so a client that got the order wrong can
    // recover rather than reconnecting blind.
    let response = handshake(&mut stream).await;
    assert!(response.accepted);
}

#[tokio::test]
async fn resume_after_zero_delivers_every_stored_event_including_the_first() {
    let fx = start("replay").await;
    append_events(&fx, 3);

    let mut stream = connect(&fx.endpoint).await;
    assert!(handshake(&mut stream).await.accepted);

    send(
        &mut stream,
        type_url::command::SUBSCRIBE_EVENTS,
        &fx.session_id,
        &SubscribeEvents {
            session_id: fx.session_id.clone(),
            resume_after_sequence: 0,
        },
    )
    .await;

    let mut got = Vec::new();
    for _ in 0..3 {
        got.push(recv(&mut stream).await.sequence);
    }

    assert_eq!(
        got,
        vec![1, 2, 3],
        "a first subscription must receive the run's opening event; \
         this is the whole reason no event is numbered 0"
    );
}

#[tokio::test]
async fn resume_after_a_sequence_is_exclusive() {
    let fx = start("resume").await;
    append_events(&fx, 4);

    let mut stream = connect(&fx.endpoint).await;
    assert!(handshake(&mut stream).await.accepted);

    send(
        &mut stream,
        type_url::command::SUBSCRIBE_EVENTS,
        &fx.session_id,
        &SubscribeEvents {
            session_id: fx.session_id.clone(),
            resume_after_sequence: 2,
        },
    )
    .await;

    let first = recv(&mut stream).await;
    assert_eq!(
        first.sequence, 3,
        "resume_after_sequence names the last event the client HOLDS, so the \
         next one it receives is that plus one, never a repeat of it"
    );
    assert_eq!(recv(&mut stream).await.sequence, 4);
}

#[tokio::test]
async fn an_event_appended_after_subscribing_arrives_live() {
    let fx = start("live").await;
    append_events(&fx, 1);

    let mut stream = connect(&fx.endpoint).await;
    assert!(handshake(&mut stream).await.accepted);

    send(
        &mut stream,
        type_url::command::SUBSCRIBE_EVENTS,
        &fx.session_id,
        &SubscribeEvents {
            session_id: fx.session_id.clone(),
            resume_after_sequence: 0,
        },
    )
    .await;

    assert_eq!(recv(&mut stream).await.sequence, 1, "the replayed event");

    // Appended only now, after the subscription is established.
    {
        let store = fx.daemon.store.lock().expect("lock store");
        let event = EventRecord::new(
            format!("ev-{}-2", fx.run_id),
            &fx.run_id,
            2,
            type_url::event::DIAGNOSTIC,
            2_000,
            serde_json::to_vec(&serde_json::json!({ "message": "live" })).expect("payload"),
            GENESIS_HASH,
        );
        store.append_event(&event).expect("append");
    }

    let live = recv(&mut stream).await;
    assert_eq!(
        live.sequence, 2,
        "a live event must reach an established subscriber"
    );
    assert_eq!(live.type_url, type_url::event::DIAGNOSTIC);
}

/// An event this protocol does not declare is still streamed, in sequence.
///
/// The obvious improvement here is a filter that sends only declared events,
/// and it would hang every client. A run numbers ALL its events from one
/// counter, declared and not, so dropping one leaves a hole. The desktop
/// client admits undecodable events through the same sequencing as decoded
/// ones (`admit()` in `apps/vitna-desktop/src/run/reducer.ts`), so a hole reads
/// as a gap: it resubscribes after the last sequence it holds, the daemon
/// drops the same event again, and the stream never advances. Forever, and
/// with nothing on screen to say why.
///
/// So an undeclared name is the client's to handle, and it does: it lands as a
/// visible "unknown" item with the sequence advanced. Keeping names declared
/// is the engine's job, not the transport's.
#[tokio::test]
async fn an_undeclared_event_is_streamed_rather_than_leaving_a_hole() {
    let fx = start("undeclared").await;

    {
        let store = fx.daemon.store.lock().expect("lock store");
        let mut prev = GENESIS_HASH.to_string();
        let names = [
            type_url::event::DIAGNOSTIC,
            // One of the names the engine records today that no .proto declares.
            "vitna.v1.TurnStarted",
            type_url::event::DIAGNOSTIC,
        ];
        for (i, name) in names.iter().enumerate() {
            let sequence = vitna_protocol::FIRST_EVENT_SEQUENCE + i as u64;
            let event = EventRecord::new(
                format!("ev-{}-{sequence}", fx.run_id),
                &fx.run_id,
                sequence,
                *name,
                1_000 + sequence,
                b"{}".to_vec(),
                &prev,
            );
            prev = event.event_hash.clone();
            store.append_event(&event).expect("append");
        }
    }

    let mut stream = connect(&fx.endpoint).await;
    assert!(handshake(&mut stream).await.accepted);
    send(
        &mut stream,
        type_url::command::SUBSCRIBE_EVENTS,
        &fx.session_id,
        &SubscribeEvents {
            session_id: fx.session_id.clone(),
            resume_after_sequence: 0,
        },
    )
    .await;

    let got: Vec<(u64, String)> = {
        let mut v = Vec::new();
        for _ in 0..3 {
            let e = recv(&mut stream).await;
            v.push((e.sequence, e.type_url));
        }
        v
    };

    assert_eq!(
        got.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "the stream must stay contiguous; a filtered event is a hole the client \
         reads as a gap and resubscribes into forever"
    );
    assert_eq!(got[1].1, "vitna.v1.TurnStarted");
}

#[tokio::test]
async fn an_undeclared_command_is_refused_by_name() {
    let fx = start("unknown").await;
    let mut stream = connect(&fx.endpoint).await;
    assert!(handshake(&mut stream).await.accepted);

    send(
        &mut stream,
        "type.vitna.ai/vitna.protocol.v1.NoSuchCommand",
        &fx.session_id,
        &serde_json::json!({}),
    )
    .await;

    let refusal: ErrorResponse = recv(&mut stream).await.payload_as().expect("decode");
    assert_eq!(refusal.code, ErrorCode::UnknownCommand);
    assert!(
        refusal.message.contains("not a declared command"),
        "a refusal should distinguish an undeclared name from a declared one \
         nobody has implemented yet; got {:?}",
        refusal.message
    );
}

/// A declared command with no implementation must not be reported the same way
/// as a typo. The client can act on "later"; it cannot act on "never".
#[tokio::test]
async fn a_declared_but_unimplemented_command_says_so() {
    let fx = start("notyet").await;
    let mut stream = connect(&fx.endpoint).await;
    assert!(handshake(&mut stream).await.accepted);

    send(
        &mut stream,
        type_url::command::APPROVE_ACTION,
        &fx.session_id,
        &serde_json::json!({}),
    )
    .await;

    let refusal: ErrorResponse = recv(&mut stream).await.payload_as().expect("decode");
    assert_eq!(refusal.code, ErrorCode::UnknownCommand);
    assert!(
        refusal.message.contains("does not serve it yet"),
        "got {:?}",
        refusal.message
    );
}

#[cfg(unix)]
#[tokio::test]
async fn the_socket_is_owner_only_inside_an_owner_only_directory() {
    use std::os::unix::fs::PermissionsExt;

    let fx = start("perms").await;
    let path = std::path::Path::new(&fx.endpoint);

    let socket = std::fs::metadata(path).expect("stat socket");
    assert_eq!(
        socket.permissions().mode() & 0o777,
        0o600,
        "the socket must not be reachable by another account"
    );

    let dir = std::fs::metadata(path.parent().expect("socket has a parent")).expect("stat dir");
    assert_eq!(
        dir.permissions().mode() & 0o777,
        0o700,
        "a world-traversable directory defeats the socket's own mode"
    );
}
