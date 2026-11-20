//! Whether a daemon is there, found out by asking it.
//!
//! The terminal tries each endpoint `vitna_protocol::endpoint` declares and
//! completes the declared handshake with the first that answers. Only an
//! accepted handshake counts as a daemon: a pipe that opens and then says
//! nothing, or says something this terminal cannot read, is reported as that,
//! never as connected. ADR-0005 keeps the daemon off TCP, so there is no port
//! to fall back to, and nothing here tries one.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedReceiver;

use vitna_protocol::messages::{
    ApplyChangeSet, ApproveAction, CancelRun, CreateSession, ErrorResponse, HandshakeRequest,
    HandshakeResponse, PauseRun, RejectAction, ResumeRun, SessionCreated, SpawnChild, SteerRun,
    SubmitTurn, SubscribeEvents,
};
use vitna_protocol::{type_url, ProtocolEnvelope, MAX_FRAME_SIZE_BYTES};

use crate::app::Update;

/// Long enough for a daemon under load, short enough that a pipe which opens
/// and never answers does not leave the terminal looking forever.
const PATIENCE: Duration = Duration::from_secs(3);

/// A handshake response is a few hundred bytes. Any process can create a pipe
/// whose name starts `vitna-`, and the endpoint list enumerates those, so the
/// reply's own length prefix is not trusted to size an allocation.
const MAX_REPLY_BYTES: u32 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Probing,
    /// Nothing answered at any declared endpoint.
    Absent {
        why: String,
    },
    /// Something answered and the handshake did not complete: a refusal in the
    /// daemon's own words, silence, or a reply this terminal could not read.
    Trouble {
        what: String,
    },
    /// A daemon accepted the handshake.
    Open {
        version: String,
    },
}

impl Link {
    pub fn is_open(&self) -> bool {
        matches!(self, Link::Open { .. })
    }
}

/// Commands the TUI sends to a connected daemon session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundCmd {
    SubmitTurn {
        prompt: String,
        attached_file_paths: Vec<String>,
    },
    ApproveAction {
        approval_id: String,
        action_digest: String,
        scope: String,
    },
    RejectAction {
        approval_id: String,
        action_digest: String,
        reason: String,
    },
    SteerRun {
        run_id: String,
        feedback: String,
    },
    PauseRun {
        run_id: String,
        reason: String,
    },
    ResumeRun {
        run_id: String,
    },
    CancelRun {
        run_id: String,
        reason: String,
    },
    SpawnChild {
        parent_run_id: String,
        goal: String,
    },
    ApplyChangeSet {
        change_set_id: String,
        target_checkout_path: String,
        expected_base_commit: String,
    },
}

/// Asks every declared endpoint in turn and reports the first real answer.
pub async fn probe() -> Link {
    let endpoints = vitna_protocol::endpoint::candidates();
    if endpoints.is_empty() {
        let why = vitna_protocol::endpoint::preferred()
            .err()
            .unwrap_or_else(|| "no endpoint is declared for this platform".to_string());
        return Link::Absent {
            why: crate::text::clean(&why),
        };
    }
    for endpoint in &endpoints {
        let Some(mut stream) = connect(endpoint).await else {
            continue;
        };
        return match tokio::time::timeout(PATIENCE, handshake_io(&mut stream)).await {
            Ok(Ok(reply)) if reply.accepted => Link::Open {
                version: format!(
                    "{}.{}",
                    reply.selected_version_major, reply.selected_version_minor
                ),
            },
            Ok(Ok(reply)) => Link::Trouble {
                what: crate::text::clean(&if reply.rejection_reason.is_empty() {
                    "the daemon refused the handshake and gave no reason".to_string()
                } else {
                    format!(
                        "the daemon refused the handshake: {}",
                        reply.rejection_reason
                    )
                }),
            },
            Ok(Err(e)) => Link::Trouble {
                what: crate::text::clean(&format!("the handshake failed: {e}")),
            },
            Err(_) => Link::Trouble {
                what: format!(
                    "the daemon did not answer the handshake within {} seconds",
                    PATIENCE.as_secs()
                ),
            },
        };
    }
    Link::Absent {
        why: "nothing is listening at the declared endpoint".to_string(),
    }
}

/// Connects to the first responding daemon endpoint, opens a workspace-bound
/// session in default `"build"` mode, subscribes to its event stream, and
/// bridges commands and events until either side closes.
pub async fn connect_session(
    workspace: PathBuf,
    updates: Sender<Update>,
    cmds: UnboundedReceiver<OutboundCmd>,
) {
    connect_session_with_mode(workspace, "build".to_string(), updates, cmds).await;
}

/// Connects to the first responding daemon endpoint and opens a workspace-bound
/// session in `initial_mode` (`"inspect"` or `"build"`).
pub async fn connect_session_with_mode(
    workspace: PathBuf,
    initial_mode: String,
    updates: Sender<Update>,
    cmds: UnboundedReceiver<OutboundCmd>,
) {
    let endpoints = vitna_protocol::endpoint::candidates();
    if endpoints.is_empty() {
        let why = vitna_protocol::endpoint::preferred()
            .err()
            .unwrap_or_else(|| "no endpoint is declared for this platform".to_string());
        let _ = updates.send(Update::Link(Link::Absent {
            why: crate::text::clean(&why),
        }));
        return;
    }
    for endpoint in &endpoints {
        let Some(stream) = connect(endpoint).await else {
            continue;
        };
        drive_stream_with_mode(stream, workspace, initial_mode, updates, cmds).await;
        return;
    }
    let _ = updates.send(Update::Link(Link::Absent {
        why: "nothing is listening at the declared endpoint".to_string(),
    }));
}

/// Drives the full declared protocol lifecycle over any duplex byte stream in
/// default `"build"` mode.
pub async fn drive_stream<S>(
    stream: S,
    workspace: PathBuf,
    updates: Sender<Update>,
    cmds: UnboundedReceiver<OutboundCmd>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    drive_stream_with_mode(stream, workspace, "build".to_string(), updates, cmds).await;
}

/// Drives the full declared protocol lifecycle over any duplex byte stream:
/// HandshakeRequest -> CreateSession (with `initial_mode`) -> SubscribeEvents -> duplex pump.
pub async fn drive_stream_with_mode<S>(
    mut stream: S,
    workspace: PathBuf,
    initial_mode: String,
    updates: Sender<Update>,
    mut cmds: UnboundedReceiver<OutboundCmd>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let reply = match tokio::time::timeout(PATIENCE, handshake_io(&mut stream)).await {
        Ok(Ok(reply)) if reply.accepted => reply,
        Ok(Ok(reply)) => {
            let _ = updates.send(Update::Link(Link::Trouble {
                what: crate::text::clean(&if reply.rejection_reason.is_empty() {
                    "the daemon refused the handshake and gave no reason".to_string()
                } else {
                    format!(
                        "the daemon refused the handshake: {}",
                        reply.rejection_reason
                    )
                }),
            }));
            return;
        }
        Ok(Err(e)) => {
            let _ = updates.send(Update::Link(Link::Trouble {
                what: crate::text::clean(&format!("the handshake failed: {e}")),
            }));
            return;
        }
        Err(_) => {
            let _ = updates.send(Update::Link(Link::Trouble {
                what: format!(
                    "the daemon did not answer the handshake within {} seconds",
                    PATIENCE.as_secs()
                ),
            }));
            return;
        }
    };

    let version = format!(
        "{}.{}",
        reply.selected_version_major, reply.selected_version_minor
    );
    let _ = updates.send(Update::Link(Link::Open { version }));

    // Open a workspace-bound session with the requested initial_mode.
    let create = ProtocolEnvelope::carrying(
        type_url::command::CREATE_SESSION,
        "",
        "",
        0,
        "vitna-tui-create",
        &CreateSession {
            workspace_path: workspace.to_string_lossy().into_owned(),
            title: String::new(),
            initial_mode,
            idempotency_key: "vitna-tui-create".to_string(),
        },
    );
    let create = match create {
        Ok(env) => env,
        Err(e) => {
            let _ = updates.send(Update::Link(Link::Trouble {
                what: crate::text::clean(&format!("could not encode CreateSession: {e}")),
            }));
            return;
        }
    };
    if let Err(e) = write_envelope(&mut stream, &create).await {
        let _ = updates.send(Update::Link(Link::Trouble {
            what: crate::text::clean(&format!("could not send CreateSession: {e}")),
        }));
        return;
    }

    let created_env = match tokio::time::timeout(PATIENCE, read_envelope(&mut stream)).await {
        Ok(Ok(Some(env))) => env,
        Ok(Ok(None)) => {
            let _ = updates.send(Update::Link(Link::Absent {
                why: "the daemon closed the connection before creating a session".to_string(),
            }));
            return;
        }
        Ok(Err(e)) => {
            let _ = updates.send(Update::Link(Link::Trouble {
                what: crate::text::clean(&format!("failed reading SessionCreated: {e}")),
            }));
            return;
        }
        Err(_) => {
            let _ = updates.send(Update::Link(Link::Trouble {
                what: "the daemon did not answer CreateSession in time".to_string(),
            }));
            return;
        }
    };

    let (session_id, confirmed_mode) = if created_env.type_url == type_url::envelope::SESSION_CREATED {
        match created_env.payload_as::<SessionCreated>() {
            Ok(sc) if !sc.session_id.is_empty() => (
                sc.session_id,
                if sc.initial_mode.is_empty() {
                    "build".to_string()
                } else {
                    sc.initial_mode
                },
            ),
            Ok(_) => {
                let _ = updates.send(Update::Link(Link::Trouble {
                    what: "the daemon returned an empty session id".to_string(),
                }));
                return;
            }
            Err(e) => {
                let _ = updates.send(Update::Link(Link::Trouble {
                    what: crate::text::clean(&format!("bad SessionCreated payload: {e}")),
                }));
                return;
            }
        }
    } else if created_env.type_url == type_url::envelope::ERROR_RESPONSE {
        let msg = created_env
            .payload_as::<ErrorResponse>()
            .map(|e| e.message)
            .unwrap_or_else(|_| "the daemon refused CreateSession".to_string());
        let _ = updates.send(Update::Link(Link::Trouble {
            what: crate::text::clean(&msg),
        }));
        return;
    } else {
        let _ = updates.send(Update::Link(Link::Trouble {
            what: crate::text::clean(&format!(
                "expected SessionCreated, got {}",
                created_env.type_url
            )),
        }));
        return;
    };

    // Subscribe to the session's live event stream before announcing SessionReady.
    if let Ok(sub) = ProtocolEnvelope::carrying(
        type_url::command::SUBSCRIBE_EVENTS,
        &session_id,
        "",
        0,
        "vitna-tui-sub",
        &SubscribeEvents {
            session_id: session_id.clone(),
            resume_after_sequence: 0,
        },
    ) {
        if let Err(e) = write_envelope(&mut stream, &sub).await {
            let _ = updates.send(Update::Link(Link::Trouble {
                what: crate::text::clean(&format!("could not subscribe to events: {e}")),
            }));
            return;
        }
    }

    let _ = updates.send(Update::SessionMode(confirmed_mode));
    let _ = updates.send(Update::SessionReady(session_id.clone()));

    let (mut reader, mut writer) = tokio::io::split(stream);
    let session_for_writer = session_id.clone();
    let writer_task = tokio::spawn(async move {
        let mut seq: u64 = 1;
        while let Some(cmd) = cmds.recv().await {
            let key = format!("tui-{seq}");
            seq = seq.saturating_add(1);
            let env = match cmd {
                OutboundCmd::SubmitTurn {
                    prompt,
                    attached_file_paths,
                } => ProtocolEnvelope::carrying(
                    type_url::command::SUBMIT_TURN,
                    &session_for_writer,
                    "",
                    0,
                    &key,
                    &SubmitTurn {
                        session_id: session_for_writer.clone(),
                        prompt,
                        attached_file_paths,
                        idempotency_key: key.clone(),
                    },
                ),
                OutboundCmd::ApproveAction {
                    approval_id,
                    action_digest,
                    scope,
                } => ProtocolEnvelope::carrying(
                    type_url::command::APPROVE_ACTION,
                    &session_for_writer,
                    "",
                    0,
                    &key,
                    &ApproveAction {
                        approval_id,
                        action_digest,
                        scope,
                    },
                ),
                OutboundCmd::RejectAction {
                    approval_id,
                    action_digest,
                    reason,
                } => ProtocolEnvelope::carrying(
                    type_url::command::REJECT_ACTION,
                    &session_for_writer,
                    "",
                    0,
                    &key,
                    &RejectAction {
                        approval_id,
                        action_digest,
                        reason,
                    },
                ),
                OutboundCmd::SteerRun { run_id, feedback } => ProtocolEnvelope::carrying(
                    type_url::command::STEER_RUN,
                    &session_for_writer,
                    &run_id,
                    0,
                    &key,
                    &SteerRun {
                        run_id: run_id.clone(),
                        feedback,
                        idempotency_key: key.clone(),
                    },
                ),
                OutboundCmd::PauseRun { run_id, reason } => ProtocolEnvelope::carrying(
                    type_url::command::PAUSE_RUN,
                    &session_for_writer,
                    &run_id,
                    0,
                    &key,
                    &PauseRun {
                        run_id: run_id.clone(),
                        reason,
                    },
                ),
                OutboundCmd::ResumeRun { run_id } => ProtocolEnvelope::carrying(
                    type_url::command::RESUME_RUN,
                    &session_for_writer,
                    &run_id,
                    0,
                    &key,
                    &ResumeRun {
                        run_id: run_id.clone(),
                    },
                ),
                OutboundCmd::CancelRun { run_id, reason } => ProtocolEnvelope::carrying(
                    type_url::command::CANCEL_RUN,
                    &session_for_writer,
                    &run_id,
                    0,
                    &key,
                    &CancelRun {
                        run_id: run_id.clone(),
                        reason,
                    },
                ),
                OutboundCmd::SpawnChild {
                    parent_run_id,
                    goal,
                } => ProtocolEnvelope::carrying(
                    type_url::command::SPAWN_CHILD,
                    &session_for_writer,
                    &parent_run_id,
                    0,
                    &key,
                    &SpawnChild {
                        parent_run_id: parent_run_id.clone(),
                        goal,
                        requested_mode: "build".to_string(),
                        token_budget: 0,
                        max_nesting_depth: 0,
                        idempotency_key: key.clone(),
                    },
                ),
                OutboundCmd::ApplyChangeSet {
                    change_set_id,
                    target_checkout_path,
                    expected_base_commit,
                } => ProtocolEnvelope::carrying(
                    type_url::command::APPLY_CHANGE_SET,
                    &session_for_writer,
                    "",
                    0,
                    &key,
                    &ApplyChangeSet {
                        change_set_id,
                        target_checkout_path,
                        expected_base_commit,
                        idempotency_key: key.clone(),
                    },
                ),
            };
            if let Ok(env) = env {
                if write_envelope(&mut writer, &env).await.is_err() {
                    break;
                }
            }
        }
    });

    loop {
        match read_envelope(&mut reader).await {
            Ok(Some(env)) => {
                if updates.send(Update::Wire(env)).is_err() {
                    break;
                }
            }
            Ok(None) => {
                let _ = updates.send(Update::Link(Link::Absent {
                    why: "the daemon disconnected".to_string(),
                }));
                break;
            }
            Err(e) => {
                let _ = updates.send(Update::Link(Link::Trouble {
                    what: crate::text::clean(&format!("daemon stream error: {e}")),
                }));
                break;
            }
        }
    }

    writer_task.abort();
}

#[cfg(windows)]
async fn connect(endpoint: &str) -> Option<tokio::net::windows::named_pipe::NamedPipeClient> {
    tokio::net::windows::named_pipe::ClientOptions::new()
        .open(endpoint)
        .ok()
}

#[cfg(unix)]
async fn connect(endpoint: &str) -> Option<tokio::net::UnixStream> {
    tokio::net::UnixStream::connect(endpoint).await.ok()
}

async fn write_envelope<W: AsyncWrite + Unpin>(
    writer: &mut W,
    envelope: &ProtocolEnvelope,
) -> Result<(), String> {
    let frame = envelope.encode_frame().map_err(|e| e.to_string())?;
    writer.write_all(&frame).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
}

async fn read_envelope<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<ProtocolEnvelope>, String> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.to_string()),
    }
    let len = u32::from_be_bytes(len);
    if len > MAX_FRAME_SIZE_BYTES {
        return Err(format!(
            "frame claims {len} bytes, over the {MAX_FRAME_SIZE_BYTES} limit"
        ));
    }
    let mut body = vec![0u8; len as usize];
    reader.read_exact(&mut body).await.map_err(|e| e.to_string())?;
    let env: ProtocolEnvelope = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    Ok(Some(env))
}

#[cfg(test)]
async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
) -> Result<HandshakeResponse, String> {
    handshake_io(&mut stream).await
}

async fn handshake_io<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
) -> Result<HandshakeResponse, String> {
    let request = ProtocolEnvelope::carrying(
        type_url::envelope::HANDSHAKE_REQUEST,
        "",
        "",
        0,
        "vitna-tui-probe",
        &HandshakeRequest {
            min_supported_version: vitna_protocol::PROTOCOL_VERSION_MAJOR,
            max_supported_version: vitna_protocol::PROTOCOL_VERSION_MAJOR,
            client_identifier: format!("vitna-tui/{}", env!("CARGO_PKG_VERSION")),
        },
    )
    .map_err(|e| e.to_string())?;
    write_envelope(stream, &request).await?;

    let mut len = [0u8; 4];
    stream
        .read_exact(&mut len)
        .await
        .map_err(|e| e.to_string())?;
    let len = u32::from_be_bytes(len);
    if len > MAX_REPLY_BYTES.min(MAX_FRAME_SIZE_BYTES) {
        return Err(format!(
            "the reply claims {len} bytes, far more than a handshake needs"
        ));
    }
    let mut body = vec![0u8; len as usize];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    let reply: ProtocolEnvelope = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if reply.type_url != type_url::envelope::HANDSHAKE_RESPONSE {
        return Err(format!(
            "the reply was a {}, not a handshake response",
            reply.type_url
        ));
    }
    reply
        .payload_as::<HandshakeResponse>()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same stream a daemon would answer on, with this crate's own frames
    /// read back by a stand-in that accepts, so the client half is checked
    /// without starting a daemon.
    #[tokio::test]
    async fn an_accepted_handshake_reads_as_open_and_a_refusal_as_trouble() {
        for (accepted, reason) in [(true, ""), (false, "protocol 9 only")] {
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            let answer = tokio::spawn(async move {
                let mut len = [0u8; 4];
                server.read_exact(&mut len).await.unwrap();
                let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
                server.read_exact(&mut body).await.unwrap();
                let asked: ProtocolEnvelope = serde_json::from_slice(&body).unwrap();
                assert_eq!(asked.type_url, type_url::envelope::HANDSHAKE_REQUEST);
                let reply = ProtocolEnvelope::carrying(
                    type_url::envelope::HANDSHAKE_RESPONSE,
                    "",
                    "",
                    0,
                    "stand-in",
                    &HandshakeResponse {
                        selected_version_major: 1,
                        selected_version_minor: 0,
                        daemon_build_commit: String::new(),
                        accepted,
                        rejection_reason: reason.to_string(),
                    },
                )
                .unwrap();
                server
                    .write_all(&reply.encode_frame().unwrap())
                    .await
                    .unwrap();
            });
            let reply = handshake(client).await.expect("handshake");
            answer.await.unwrap();
            assert_eq!(reply.accepted, accepted);
            assert_eq!(reply.rejection_reason, reason);
        }
    }

    #[tokio::test]
    async fn a_peer_that_hangs_up_is_an_error_not_a_daemon() {
        let (client, server) = tokio::io::duplex(1024);
        drop(server);
        assert!(handshake(client).await.is_err());
    }
}
