//! Turns in a session, driven through the real `DaemonServer::run_task` and
//! watched over the real transport.
//!
//! The subscription tests in `ipc_subscription.rs` append events by hand, and
//! their fixture used to name the session's run BEFORE appending, which
//! production never did: `run_task` set `latest_run_id` only after a run had
//! finished. That hid two defects:
//!
//! - A run driven through `run_task` streamed nothing live, not even a
//!   session's first, because the stream forwarded only events whose run was
//!   the session's `latest_run_id`, still unset or still the previous run while
//!   the current one ran.
//! - A session's second run lost its opening events. The protocol numbers
//!   events per SESSION (one `resume_after_sequence`, one client cursor), but
//!   the engine numbered every RUN from 1, so the second run's 1..N looked like
//!   replays of the first run's and were dropped, and its N+1 then landed
//!   exactly contiguous, so nothing on either side could notice.
//!
//! **Every test here runs on a multi-threaded runtime, and must.** On the
//! default single-threaded `#[tokio::test]` runtime the first defect is
//! invisible: `run_task`'s async steps never actually yield, so the
//! subscription task cannot run until the whole turn has finished, by which
//! point even a late registration has happened and every event looks like the
//! session's own. The daemon itself runs on `#[tokio::main]`'s multi-threaded
//! runtime, where the subscriber runs alongside the turn and the events were
//! dropped. Found by mutation testing: moving registration back to the end of
//! the run left all four tests green on one thread, and fails two of them on
//! every repetition on two.
//!
//! So nothing here presets a run. Every event arrives because a turn produced
//! it.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use vitna_daemon::DaemonServer;
use vitna_protocol::messages::{HandshakeRequest, HandshakeResponse, SubscribeEvents};
use vitna_protocol::{type_url, ProtocolEnvelope};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

const PATIENCE: Duration = Duration::from_secs(10);

/// A turn whose prompt names no edit, so `run_task` inspects the workspace and
/// finishes: TurnStarted, ContextAssembled, ToolStarted, ToolFinished and
/// ReceiptGenerated.
const EVENTS_PER_QUIET_TURN: u64 = 5;

fn unique(tag: &str) -> String {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{tag}-{}-{n}", std::process::id())
}

#[cfg(windows)]
fn endpoint_for(tag: &str) -> String {
    format!(r"\\.\pipe\vitna-turns-{}", unique(tag))
}

#[cfg(unix)]
fn endpoint_for(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("vitna-turns-{}", unique(tag)));
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

async fn send<W: AsyncWrite + Unpin, T: serde::Serialize>(w: &mut W, type_url: &str, body: &T) {
    let envelope =
        ProtocolEnvelope::carrying(type_url, "", "", 1, "turns-test", body).expect("encode");
    w.write_all(&envelope.encode_frame().expect("frame"))
        .await
        .expect("write");
    w.flush().await.expect("flush");
}

/// The next frame, or `None` if nothing arrives within `wait`.
async fn next_frame<R: AsyncRead + Unpin>(r: &mut R, wait: Duration) -> Option<ProtocolEnvelope> {
    let mut len = [0u8; 4];
    match tokio::time::timeout(wait, r.read_exact(&mut len)).await {
        Ok(Ok(_)) => {}
        _ => return None,
    }
    let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
    tokio::time::timeout(PATIENCE, r.read_exact(&mut body))
        .await
        .expect("a frame body should follow its length")
        .expect("read body");
    Some(serde_json::from_slice(&body).expect("decode envelope"))
}

struct Fixture {
    endpoint: String,
    daemon: Arc<DaemonServer>,
    session_id: String,
}

async fn start(tag: &str) -> Fixture {
    let workspace = std::env::temp_dir().join(unique("vitna-turns-ws"));
    std::fs::create_dir_all(&workspace).expect("create workspace");

    let daemon = Arc::new(DaemonServer::new(
        EventStore::open_in_memory().expect("open store"),
        Arc::new(std::sync::Mutex::new(
            FakeRunner::new(workspace.join("runner.journal")).expect("open runner"),
        )),
        vitna_receipts::generate_signing_key(),
    ));
    let session_id = daemon
        .create_session(&workspace)
        .expect("create session")
        .session_id;

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
        .expect("the listener should bind")
        .expect("ready signal");

    Fixture {
        endpoint,
        daemon,
        session_id,
    }
}

/// Connects, completes the handshake and subscribes after `resume_after`.
async fn subscribe(fx: &Fixture, resume_after: u64) -> impl AsyncRead + AsyncWrite + Unpin {
    let mut stream = connect(&fx.endpoint).await;
    send(
        &mut stream,
        type_url::envelope::HANDSHAKE_REQUEST,
        &HandshakeRequest {
            min_supported_version: 1,
            max_supported_version: 1,
            client_identifier: "turns-test/0.1.0".into(),
        },
    )
    .await;
    let reply = next_frame(&mut stream, PATIENCE)
        .await
        .expect("handshake reply");
    let hs: HandshakeResponse = reply.payload_as().expect("decode handshake");
    assert!(hs.accepted, "handshake refused: {}", hs.rejection_reason);

    send(
        &mut stream,
        type_url::command::SUBSCRIBE_EVENTS,
        &SubscribeEvents {
            session_id: fx.session_id.clone(),
            resume_after_sequence: resume_after,
        },
    )
    .await;
    stream
}

async fn turn(fx: &Fixture) {
    fx.daemon
        .run_task(&fx.session_id, "look around", true, None, false)
        .await
        .expect("a quiet turn completes");
}

/// Collects sequences until `count` have arrived, or until the stream goes
/// quiet, and returns what came.
async fn collect<R: AsyncRead + Unpin>(stream: &mut R, count: u64) -> Vec<u64> {
    let mut got = Vec::new();
    while (got.len() as u64) < count {
        match next_frame(stream, Duration::from_secs(3)).await {
            Some(frame) => got.push(frame.sequence),
            None => break,
        }
    }
    got
}

/// A subscriber sees a turn it subscribed before, in order, from the first
/// event.
///
/// On two threads the subscriber normally reaches its live loop before the
/// turn records anything, so this usually exercises the live path, and it does
/// fail when registration moves to the end of the run. It is not synchronized,
/// though: a subscriber slow enough to start its replay after the turn has
/// finished receives everything by replay instead. The synchronized live-path
/// test is [`a_second_turn_continues_the_sessions_sequence`].
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscriber_sees_a_turn_it_subscribed_before() {
    let fx = start("first").await;
    let mut stream = subscribe(&fx, 0).await;

    turn(&fx).await;

    let got = collect(&mut stream, EVENTS_PER_QUIET_TURN).await;
    assert_eq!(
        got,
        (1..=EVENTS_PER_QUIET_TURN).collect::<Vec<_>>(),
        "a subscriber watching a session must see its turn, in order, from the \
         first event"
    );
}

/// The second turn in a session, which is also this file's LIVE-path test.
///
/// Receiving all of the first turn's events proves the subscriber has finished
/// its replay and is waiting on the live feed, so every event of the second
/// turn can only arrive live. That makes this deterministic where the
/// first-turn test above is not, and it is why this one failed against both
/// defects: the second turn restarted its count at 1, and the stream matched
/// live events against the run the session had FINISHED, not the one running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_turn_continues_the_sessions_sequence() {
    let fx = start("second").await;
    let mut stream = subscribe(&fx, 0).await;

    turn(&fx).await;
    let mut got = collect(&mut stream, EVENTS_PER_QUIET_TURN).await;
    assert_eq!(
        got,
        (1..=EVENTS_PER_QUIET_TURN).collect::<Vec<_>>(),
        "the first turn, delivered before the second starts"
    );

    // Only now: the subscriber is provably on the live feed.
    turn(&fx).await;
    got.extend(collect(&mut stream, EVENTS_PER_QUIET_TURN).await);
    assert_eq!(
        got,
        (1..=2 * EVENTS_PER_QUIET_TURN).collect::<Vec<_>>(),
        "the second turn must continue at {} rather than restart at 1; a \
         restarted count reads as replays of the first turn and is dropped",
        EVENTS_PER_QUIET_TURN + 1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_subscriber_arriving_after_two_turns_replays_both() {
    let fx = start("replay").await;
    turn(&fx).await;
    turn(&fx).await;

    let mut stream = subscribe(&fx, 0).await;
    let got = collect(&mut stream, 2 * EVENTS_PER_QUIET_TURN).await;

    assert_eq!(
        got,
        (1..=2 * EVENTS_PER_QUIET_TURN).collect::<Vec<_>>(),
        "a late subscriber must get the whole session, not only its latest turn"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resuming_mid_session_sends_exactly_what_follows() {
    let fx = start("resume").await;
    turn(&fx).await;
    turn(&fx).await;

    // Resume from inside the FIRST turn, so the replay has to cross into the
    // second turn to finish.
    let held = 3;
    let mut stream = subscribe(&fx, held).await;
    let got = collect(&mut stream, 2 * EVENTS_PER_QUIET_TURN - held).await;

    assert_eq!(
        got,
        (held + 1..=2 * EVENTS_PER_QUIET_TURN).collect::<Vec<_>>(),
        "resuming after {held} must send {} onward across both turns, with no \
         repeat and no hole",
        held + 1
    );
}
