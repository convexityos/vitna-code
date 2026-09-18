//! The daemon's door.
//!
//! `vitna-coded` was a three-line `println!` until this module existed, and
//! nothing in the workspace had ever opened a listening socket of any kind, so
//! the window's "Daemon not running" was not a placeholder: there was genuinely
//! nothing to connect to.
//!
//! The transport is the one ADR-0005 declares, a Windows named pipe or a Unix
//! domain socket, resolved through [`vitna_protocol::endpoint`] so that the
//! side that binds and the side that probes read the same list. Frames are the
//! protocol crate's own length-prefixed envelopes.
//!
//! What this module does NOT claim. On Unix the socket directory is created
//! 0700 and the socket itself chmod 0600, which is owner-only in the way the
//! ADR means. On Windows the pipe is created with `first_pipe_instance`, so no
//! later process can squat the name and impersonate the daemon, but its DACL
//! is the system default rather than an explicit owner-only one; tightening
//! that needs a Win32 security descriptor and a `windows-sys` dependency. The
//! ADR's load-bearing claim, that no web page can reach the transport, holds
//! either way, since no browser API opens a named pipe.

use std::io::{self, Cursor};
use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use vitna_protocol::api::{
    self, CreateSessionRequest, ErrorResponse, HealthResponse, SessionListResponse,
    SubmitTurnRequest,
};
use vitna_protocol::{ProtocolEnvelope, MAX_FRAME_SIZE_BYTES};

use crate::server::DaemonServer;

/// Reads one frame, or `None` at a clean end of stream.
async fn read_frame<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Option<ProtocolEnvelope>> {
    let mut len_bytes = [0u8; 4];
    match stream.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        // A client that closes between calls is ordinary, not an error.
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u32::from_be_bytes(len_bytes);
    if len > MAX_FRAME_SIZE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("incoming frame size {len} exceeds limit {MAX_FRAME_SIZE_BYTES}"),
        ));
    }

    let mut body = vec![0u8; len as usize];
    stream.read_exact(&mut body).await?;

    // Rebuild the whole frame so the protocol crate's own decoder does the
    // parsing, rather than a second implementation of it living here.
    let mut whole = Vec::with_capacity(4 + body.len());
    whole.extend_from_slice(&len_bytes);
    whole.extend_from_slice(&body);
    let mut cursor = Cursor::new(whole);
    ProtocolEnvelope::decode_frame(&mut cursor).map(Some)
}

async fn write_frame<S: AsyncWrite + Unpin>(
    stream: &mut S,
    envelope: &ProtocolEnvelope,
) -> io::Result<()> {
    let frame = envelope.encode_frame()?;
    stream.write_all(&frame).await?;
    stream.flush().await
}

/// Builds a reply envelope, falling back to an error frame if the payload
/// itself will not serialize. A caller is always answered.
fn reply<T: serde::Serialize>(request: &ProtocolEnvelope, type_url: &str, payload: &T) -> ProtocolEnvelope {
    ProtocolEnvelope::with_payload(
        type_url,
        request.session_id.clone(),
        request.run_id.clone(),
        request.sequence,
        request.idempotency_key.clone(),
        payload,
    )
    .unwrap_or_else(|e| error_reply(request, &format!("reply could not be encoded: {e}")))
}

fn error_reply(request: &ProtocolEnvelope, message: &str) -> ProtocolEnvelope {
    let payload = serde_json::to_vec(&ErrorResponse {
        message: message.to_string(),
    })
    .unwrap_or_default();
    ProtocolEnvelope::new(
        api::type_url::ERROR,
        request.session_id.clone(),
        request.run_id.clone(),
        request.sequence,
        request.idempotency_key.clone(),
        payload,
    )
}

/// Answers one call. Every failure comes back as an ERROR frame carrying its
/// reason, so a client never has to infer what went wrong from a dropped
/// connection.
pub async fn dispatch(daemon: &DaemonServer, request: &ProtocolEnvelope) -> ProtocolEnvelope {
    match request.type_url.as_str() {
        api::type_url::HEALTH => {
            let endpoint = vitna_protocol::endpoint::preferred().unwrap_or_default();
            reply(
                request,
                api::type_url::HEALTH_RESULT,
                &HealthResponse {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    pid: std::process::id(),
                    endpoint,
                    providers_ready: crate::providers_ready(),
                    session_count: daemon.list_sessions().len(),
                },
            )
        }

        api::type_url::CREATE_SESSION => match request.payload_as::<CreateSessionRequest>() {
            Ok(req) => match daemon.create_session(&req.workspace_root) {
                Ok(info) => reply(request, api::type_url::SESSION, &info),
                Err(e) => error_reply(request, &e),
            },
            Err(e) => error_reply(request, &format!("malformed CreateSession: {e}")),
        },

        api::type_url::LIST_SESSIONS => reply(
            request,
            api::type_url::SESSION_LIST,
            &SessionListResponse {
                sessions: daemon.list_sessions(),
            },
        ),

        api::type_url::SUBMIT_TURN => match request.payload_as::<SubmitTurnRequest>() {
            Ok(req) => match daemon.run_turn(&req).await {
                Ok(result) => reply(request, api::type_url::TURN_RESULT, &result),
                Err(e) => error_reply(request, &e),
            },
            Err(e) => error_reply(request, &format!("malformed SubmitTurn: {e}")),
        },

        other => error_reply(request, &format!("unknown type_url: {other}")),
    }
}

/// Serves one connection until the client goes away.
pub async fn serve_connection<S>(mut stream: S, daemon: Arc<DaemonServer>) -> io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let request = match read_frame(&mut stream).await? {
            Some(r) => r,
            None => return Ok(()),
        };
        let response = dispatch(&daemon, &request).await;
        write_frame(&mut stream, &response).await?;
    }
}

/// Binds the declared endpoint and serves it until the process is stopped.
///
/// Returns the endpoint it bound, through `on_ready`, before the first accept,
/// so a caller can print it or a test can connect without polling for a file
/// that may not exist yet.
pub async fn serve<F>(daemon: Arc<DaemonServer>, endpoint: &str, on_ready: F) -> io::Result<()>
where
    F: FnOnce(&str),
{
    platform::serve(daemon, endpoint, on_ready).await
}

#[cfg(windows)]
mod platform {
    use super::*;
    use tokio::net::windows::named_pipe::ServerOptions;

    pub async fn serve<F>(daemon: Arc<DaemonServer>, endpoint: &str, on_ready: F) -> io::Result<()>
    where
        F: FnOnce(&str),
    {
        // first_pipe_instance fails rather than joining if the name is already
        // published. A daemon that silently became the second instance of
        // someone else's pipe is the failure this prevents.
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(endpoint)?;

        on_ready(endpoint);

        loop {
            server.connect().await?;
            let connected = server;
            // Publish the next instance before serving this one, or a second
            // client meets a closed door for the length of the first call.
            server = ServerOptions::new().create(endpoint)?;

            let d = daemon.clone();
            tokio::spawn(async move {
                if let Err(e) = serve_connection(connected, d).await {
                    tracing::warn!("connection ended: {e}");
                }
            });
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tokio::net::UnixListener;

    pub async fn serve<F>(daemon: Arc<DaemonServer>, endpoint: &str, on_ready: F) -> io::Result<()>
    where
        F: FnOnce(&str),
    {
        let path = Path::new(endpoint);

        if let Some(dir) = path.parent() {
            // Owner-only, per ADR-0005: the directory matters as much as the
            // socket, since a traversable parent lets another account reach it.
            //
            // Only on a directory this call creates, though. Chmod'ing a
            // directory somebody else made is not ours to do, and the case that
            // proves it is an endpoint under /tmp: tightening that to 0700
            // would lock every other account out of it, and as root it would
            // succeed. CI caught this because the process did not own /tmp and
            // the chmod failed with EPERM. An existing directory keeps its
            // permissions and the socket's own 0600 is what guards the endpoint.
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }

        // A socket file outlives the process that made it, so a daemon that
        // was killed leaves one behind and bind() would fail on it. Removing
        // it is safe only because a LIVE daemon holds the name through
        // first_pipe_instance's Unix equivalent: bind fails while one listens.
        if path.exists() {
            match std::os::unix::net::UnixStream::connect(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!("a daemon is already listening on {endpoint}"),
                    ))
                }
                Err(_) => std::fs::remove_file(path)?,
            }
        }

        let listener = UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;

        on_ready(endpoint);

        loop {
            let (stream, _addr) = listener.accept().await?;
            let d = daemon.clone();
            tokio::spawn(async move {
                if let Err(e) = serve_connection(stream, d).await {
                    tracing::warn!("connection ended: {e}");
                }
            });
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use super::*;

    pub async fn serve<F>(_d: Arc<DaemonServer>, _e: &str, _r: F) -> io::Result<()>
    where
        F: FnOnce(&str),
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this platform has no declared local IPC transport",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use vitna_protocol::client::Client;
    use vitna_runner::FakeRunner;
    use vitna_store::EventStore;

    /// An endpoint no other test or daemon will collide with.
    fn test_endpoint(tag: &str) -> String {
        let unique = format!("{}-{}", std::process::id(), tag);
        if cfg!(windows) {
            format!(r"\\.\pipe\vitna-test-{unique}")
        } else {
            std::env::temp_dir()
                .join(format!("vitna-test-{unique}.sock"))
                .to_string_lossy()
                .to_string()
        }
    }

    fn daemon_in(dir: &std::path::Path) -> Arc<DaemonServer> {
        let store = EventStore::open_in_memory().expect("open store");
        let runner = FakeRunner::new(dir.join("ipc.journal")).expect("fake runner");
        Arc::new(DaemonServer::new(
            store,
            Arc::new(std::sync::Mutex::new(runner)),
            vitna_receipts::generate_signing_key(),
        ))
    }

    /// The whole of the transport, end to end: a listener on the endpoint
    /// ADR-0005 declares, and the same blocking client the window uses.
    ///
    /// Nothing in this workspace had ever opened a listening socket, so this is
    /// the first test that could fail for a reason the framing tests cannot
    /// see: a daemon that binds somewhere nobody connects to.
    #[tokio::test]
    async fn a_client_reaches_the_daemon_over_the_declared_transport() {
        let dir = std::env::temp_dir().join(format!("vitna_ipc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let endpoint = test_endpoint("roundtrip");
        let daemon = daemon_in(&dir);

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<String>();
        let serving = tokio::spawn({
            let endpoint = endpoint.clone();
            async move {
                let mut tx = Some(ready_tx);
                if let Err(e) = serve(daemon, &endpoint, move |e| {
                    if let Some(tx) = tx.take() {
                        let _ = tx.send(e.to_string());
                    }
                })
                .await
                {
                    eprintln!("serve({endpoint}) failed: {e}");
                }
            }
        });

        // Bound before the first connect, so this waits on a fact rather than
        // on a sleep long enough to usually work.
        let bound = tokio::time::timeout(Duration::from_secs(5), ready_rx)
            .await
            .expect("the listener binds")
            .expect("the ready signal arrives");
        assert_eq!(bound, endpoint);

        let dir_for_client = dir.clone();
        let endpoint_for_client = endpoint.clone();
        let calls = tokio::task::spawn_blocking(move || {
            let mut client = Client::connect_to(&endpoint_for_client)?;

            let health = client.health()?;
            let session = client.create_session(dir_for_client)?;
            let sessions = client.list_sessions()?;

            // A call the daemon does not know must come back as a refusal with
            // a reason, not as a dropped connection.
            let unknown = client.create_session("");

            Ok::<_, String>((health, session, sessions, unknown))
        })
        .await
        .expect("the client thread finishes");

        let (health, session, sessions, unknown) = calls.expect("the calls succeed");

        assert_eq!(health.pid, std::process::id());
        assert_eq!(health.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, session.session_id);
        assert_eq!(session.status, "active");

        let refusal = unknown.expect_err("an empty workspace root is refused");
        assert!(
            refusal.contains("does not exist"),
            "the refusal carries its reason: {refusal}"
        );

        serving.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
