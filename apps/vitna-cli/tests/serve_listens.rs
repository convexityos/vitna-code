//! `vitna serve` starts the daemon, rather than saying it has.
//!
//! It printed "Vitna daemon listening on local session channel" while binding
//! nothing, so a client probing the declared endpoint found no daemon however
//! long it waited, and the release scripts ship `vitna` and not `vitna-coded`,
//! which made it the only way an installed copy had to start one. These run
//! the `vitna` executable Cargo built, the way
//! `crates/daemon/tests/binary_serves.rs` runs `vitna-coded`, and complete the
//! declared handshake with it over the platform's own transport.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Child;

use vitna_protocol::messages::{HandshakeRequest, HandshakeResponse};
use vitna_protocol::{type_url, ProtocolEnvelope, PROTOCOL_VERSION_MAJOR};

const PATIENCE: Duration = Duration::from_secs(30);
const READY: &str = "Vitna daemon listening on";

fn unique(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{tag}-{}-{nanos}", std::process::id())
}

#[cfg(windows)]
fn endpoint() -> String {
    format!(r"\\.\pipe\vitna-servetest-{}", unique("e"))
}

#[cfg(unix)]
fn endpoint() -> String {
    let dir = std::env::temp_dir().join(unique("vitna-servetest"));
    std::fs::create_dir_all(&dir).expect("create socket dir");
    dir.join("vitna.sock").to_string_lossy().to_string()
}

#[cfg(windows)]
async fn connect(endpoint: &str) -> impl AsyncRead + AsyncWrite + Unpin {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .expect("connect to the daemon's pipe")
}

#[cfg(unix)]
async fn connect(endpoint: &str) -> impl AsyncRead + AsyncWrite + Unpin {
    tokio::net::UnixStream::connect(endpoint)
        .await
        .expect("connect to the daemon's socket")
}

fn scratch_store() -> PathBuf {
    std::env::temp_dir()
        .join(unique("vitna-servetest-store"))
        .join("daemon.db")
}

/// Starts `vitna serve` with `store_flag` naming the store, and returns once
/// it prints the line it prints after binding. Connecting before that would
/// race the listener and fail for a reason that is not the command's.
async fn start(endpoint: &str, store_flag: &str, store: &Path) -> (Child, String) {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_vitna"))
        .arg("serve")
        .arg("--endpoint")
        .arg(endpoint)
        .arg(store_flag)
        .arg(store)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A failed assertion must not leave a daemon running behind it.
        .kill_on_drop(true)
        .spawn()
        .expect("start vitna serve");

    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    let ready = tokio::time::timeout(PATIENCE, async {
        while let Some(line) = lines.next_line().await.expect("read stdout") {
            if line.starts_with(READY) {
                return line;
            }
        }
        panic!("vitna serve closed stdout without saying it was listening");
    })
    .await
    .expect("vitna serve should say it is listening once it is");
    (child, ready)
}

#[tokio::test]
async fn vitna_serve_listens_and_completes_the_declared_handshake() {
    let endpoint = endpoint();
    let store = scratch_store();
    let (mut child, ready) = start(&endpoint, "--store", &store).await;
    assert!(
        ready.contains(&endpoint),
        "it announced {ready:?}, not the endpoint it was told to use"
    );

    let mut stream = connect(&endpoint).await;
    let request = ProtocolEnvelope::carrying(
        type_url::envelope::HANDSHAKE_REQUEST,
        "",
        "",
        1,
        "servetest",
        &HandshakeRequest {
            min_supported_version: 1,
            max_supported_version: 1,
            client_identifier: "serve-listens-test/0.1.0".into(),
        },
    )
    .expect("encode handshake");
    stream
        .write_all(&request.encode_frame().expect("frame"))
        .await
        .expect("send handshake");
    stream.flush().await.expect("flush");

    let mut len = [0u8; 4];
    tokio::time::timeout(PATIENCE, stream.read_exact(&mut len))
        .await
        .expect("the daemon should answer the handshake")
        .expect("read length");
    let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut body).await.expect("read body");
    let reply: ProtocolEnvelope = serde_json::from_slice(&body).expect("decode envelope");

    assert_eq!(reply.type_url, type_url::envelope::HANDSHAKE_RESPONSE);
    let response: HandshakeResponse = reply.payload_as().expect("decode handshake response");
    assert!(
        response.accepted,
        "vitna serve refused the handshake: {}",
        response.rejection_reason
    );
    assert_eq!(response.selected_version_major, PROTOCOL_VERSION_MAJOR);

    // The journal is where it was told to be. The old command opened one
    // beside whatever folder it was run from.
    assert!(store.exists(), "no journal at {}", store.display());

    child.kill().await.expect("stop vitna serve");
}

/// `--db` was this flag's name before it matched vitna-coded's `--store`, and
/// a script written against it still starts the daemon on the store it names.
#[tokio::test]
async fn the_old_db_flag_still_names_the_journal() {
    let endpoint = endpoint();
    let store = scratch_store();
    let (mut child, _) = start(&endpoint, "--db", &store).await;
    assert!(
        store.exists(),
        "--db did not name the journal: nothing at {}",
        store.display()
    );
    child.kill().await.expect("stop vitna serve");
}

/// A daemon that cannot start says why and exits non-zero, rather than dying
/// quietly, because from a client a quiet death looks exactly like a daemon
/// that was never started.
#[tokio::test]
async fn vitna_serve_that_cannot_start_says_why_and_exits_nonzero() {
    // A store whose parent is a FILE cannot be created.
    let blocker = std::env::temp_dir().join(unique("vitna-servetest-blocker"));
    std::fs::write(&blocker, b"not a directory").expect("create blocker file");
    let impossible_store = blocker.join("sub").join("daemon.db");

    let output = tokio::time::timeout(
        PATIENCE,
        tokio::process::Command::new(env!("CARGO_BIN_EXE_vitna"))
            .arg("serve")
            .arg("--endpoint")
            .arg(endpoint())
            .arg("--store")
            .arg(&impossible_store)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("a daemon that cannot start should exit promptly")
    .expect("run vitna serve");

    assert!(
        !output.status.success(),
        "it exited successfully with no usable store"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("vitna serve:"),
        "it must name what failed on stderr; got {stderr:?}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains(READY),
        "it said it was listening and then failed: {stdout:?}"
    );
}
