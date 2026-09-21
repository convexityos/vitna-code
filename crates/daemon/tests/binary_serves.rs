//! Starts the real `vitna-coded` executable and talks to it.
//!
//! Every other transport test calls `ipc::serve` from inside the test process,
//! which proves the library listens and says nothing about the program that
//! ships. That gap was real: the pull request adding the listener was titled
//! "vitna-coded listens" while the binary printed one line and exited, and
//! every one of its tests passed, because none of them ran the binary.
//!
//! So this test launches the executable Cargo built for this package, waits
//! for the line it prints once bound, and completes a declared handshake over
//! the platform's own transport.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

use vitna_protocol::messages::{HandshakeRequest, HandshakeResponse};
use vitna_protocol::{type_url, ProtocolEnvelope, PROTOCOL_VERSION_MAJOR};

const PATIENCE: Duration = Duration::from_secs(30);

fn unique(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{tag}-{}-{nanos}", std::process::id())
}

#[cfg(windows)]
fn endpoint() -> String {
    format!(r"\\.\pipe\vitna-bintest-{}", unique("e"))
}

#[cfg(unix)]
fn endpoint() -> String {
    let dir = std::env::temp_dir().join(unique("vitna-bintest"));
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

#[tokio::test]
async fn the_shipped_binary_listens_and_completes_the_declared_handshake() {
    let store = std::env::temp_dir()
        .join(unique("vitna-bintest-store"))
        .join("daemon.db");
    let endpoint = endpoint();

    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_vitna-coded"))
        .arg("--endpoint")
        .arg(&endpoint)
        .arg("--store")
        .arg(&store)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // A failed assertion must not leave a daemon running behind it.
        .kill_on_drop(true)
        .spawn()
        .expect("start vitna-coded");

    // Wait for the line it prints once bound. Connecting before it is bound
    // would race the listener and fail for a reason that is not the daemon's.
    let stdout = child.stdout.take().expect("piped stdout");
    let mut lines = BufReader::new(stdout).lines();
    let ready = tokio::time::timeout(PATIENCE, async {
        while let Some(line) = lines.next_line().await.expect("read stdout") {
            if line.starts_with("vitna-coded listening on") {
                return line;
            }
        }
        panic!("vitna-coded closed stdout without announcing it was listening");
    })
    .await
    .expect("vitna-coded should announce that it is listening");
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
        "bintest",
        &HandshakeRequest {
            min_supported_version: 1,
            max_supported_version: 1,
            client_identifier: "binary-serves-test/0.1.0".into(),
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
        "the binary refused the handshake: {}",
        response.rejection_reason
    );
    assert_eq!(response.selected_version_major, PROTOCOL_VERSION_MAJOR);

    child.kill().await.expect("stop vitna-coded");
}

/// A daemon that cannot start must say why and exit non-zero, rather than
/// dying silently, because from a client a silent death looks exactly like a
/// daemon that was never launched.
#[tokio::test]
async fn a_daemon_that_cannot_start_says_why_and_exits_nonzero() {
    // A store path whose parent is a FILE cannot be created.
    let blocker = std::env::temp_dir().join(unique("vitna-bintest-blocker"));
    std::fs::write(&blocker, b"not a directory").expect("create blocker file");
    let impossible_store = blocker.join("sub").join("daemon.db");

    let output = tokio::time::timeout(
        PATIENCE,
        tokio::process::Command::new(env!("CARGO_BIN_EXE_vitna-coded"))
            .arg("--endpoint")
            .arg(endpoint())
            .arg("--store")
            .arg(&impossible_store)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("a daemon that cannot start should exit promptly")
    .expect("run vitna-coded");

    assert!(
        !output.status.success(),
        "it exited successfully despite having no usable store"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("vitna-coded:"),
        "it must name what failed on stderr; got {stderr:?}"
    );
}
