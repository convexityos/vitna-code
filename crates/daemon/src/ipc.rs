//! The daemon's wire: it listens where ADR-0005 says, completes the declared
//! handshake, and streams a session's events to a subscriber.
//!
//! Before this module nothing in the workspace opened a listening socket, so
//! the desktop client's "no daemon" was not a placeholder. There was nothing
//! to connect to.
//!
//! # Why the connection is duplex
//!
//! The obvious shape is a loop that reads one frame and writes one reply. That
//! shape cannot serve this protocol. `SubscribeEvents` has no single reply: it
//! produces frames for as long as the session lives, and they have to keep
//! arriving while the reader is free to accept the next command. So a
//! connection is split: one task owns the write half and drains a queue,
//! and everything that wants to send, the command dispatcher and every live
//! subscription, holds a handle to that queue.
//!
//! That is also what makes the approval gate possible. A daemon that can only
//! answer the frame in front of it cannot ask a question and wait. A turn that
//! reaches a mutating action announces `ApprovalRequested` on the session's
//! stream and waits, and `ApproveAction` or `RejectAction` arriving on any
//! connection answers it (ADR-0007).
//!
//! # What a client must do first
//!
//! Send `HandshakeRequest`. Any other frame before it is refused with
//! `ERROR_CODE_UNAUTHORIZED` and the connection stays open, so a client that
//! got the order wrong learns why rather than seeing a closed socket and
//! guessing.

use std::sync::Arc;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use vitna_orchestration::ApprovalDecision;
use vitna_protocol::messages::{
    ApproveAction, CreateSession, ErrorCode, ErrorResponse, HandshakeRequest, HandshakeResponse,
    RejectAction, SessionCreated, SubmitTurn, SubscribeEvents,
};
use vitna_protocol::{
    type_url, ProtocolEnvelope, MAX_FRAME_SIZE_BYTES, PROTOCOL_VERSION_MAJOR,
    PROTOCOL_VERSION_MINOR,
};

use crate::{AnswerRefused, DaemonServer};

/// How many frames may wait for the write half before a producer blocks.
///
/// Bounded on purpose. An unbounded queue turns a client that has stopped
/// reading into daemon memory growth, and the cost of that is paid by the
/// machine rather than by the client that caused it.
const WRITE_QUEUE_DEPTH: usize = 256;

/// Control frames carry sequence 0.
///
/// No event does, because `FIRST_EVENT_SEQUENCE` is 1, so a zero here is
/// unambiguous: this frame is not part of the event stream and nothing should
/// fold it into a run's view.
const CONTROL_SEQUENCE: u64 = 0;

/// A handle every sender on a connection shares.
type Outbound = mpsc::Sender<ProtocolEnvelope>;

/// Reads one length-prefixed frame, or `None` at a clean end of stream.
///
/// `None` is a client that hung up, which is ordinary. An error is a frame
/// this daemon could not parse, which is not.
async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<ProtocolEnvelope>> {
    let mut len_bytes = [0u8; 4];
    match reader.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let frame_len = u32::from_be_bytes(len_bytes);
    if frame_len > MAX_FRAME_SIZE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame of {frame_len} bytes exceeds the {MAX_FRAME_SIZE_BYTES} limit"),
        ));
    }

    let mut buffer = vec![0u8; frame_len as usize];
    reader.read_exact(&mut buffer).await?;

    let envelope: ProtocolEnvelope = serde_json::from_slice(&buffer)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(Some(envelope))
}

async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    envelope: &ProtocolEnvelope,
) -> std::io::Result<()> {
    let bytes = envelope.encode_frame()?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

/// A refusal, addressed to the frame that caused it.
fn error_for(
    request: &ProtocolEnvelope,
    code: ErrorCode,
    message: impl Into<String>,
) -> ProtocolEnvelope {
    // Empty unless the error belongs to a specific action. The client pins an
    // error to a pending approval through this field and shows anything else
    // as a diagnostic, so a wrong guess here would attach a refusal to the
    // wrong decision.
    error_for_action(request, code, message, "")
}

/// A refusal that answers one approval, pinned to it by `action_id`.
fn error_for_action(
    request: &ProtocolEnvelope,
    code: ErrorCode,
    message: impl Into<String>,
    action_id: &str,
) -> ProtocolEnvelope {
    let body = ErrorResponse {
        code,
        message: message.into(),
        error_details: String::new(),
        action_id: action_id.to_string(),
    };
    ProtocolEnvelope::carrying(
        type_url::envelope::ERROR_RESPONSE,
        request.session_id.clone(),
        request.run_id.clone(),
        CONTROL_SEQUENCE,
        request.idempotency_key.clone(),
        &body,
    )
    .unwrap_or_else(|_| {
        // Serializing an ErrorResponse cannot fail: every field is a plain
        // String or a unit enum. This arm exists so a refusal never panics
        // the connection that was already reporting a problem.
        ProtocolEnvelope::new(
            type_url::envelope::ERROR_RESPONSE,
            String::new(),
            String::new(),
            CONTROL_SEQUENCE,
            String::new(),
            Vec::new(),
        )
    })
}

/// Decides the handshake.
///
/// The client sends the range it can speak. This daemon speaks exactly one
/// major version, so the question is whether that version falls inside the
/// client's range. A refusal says so in `rejection_reason` rather than closing
/// the socket, because "your build is too old" and "the daemon is not running"
/// are different problems and a client that cannot tell them apart reports the
/// wrong one.
fn negotiate(request: &HandshakeRequest) -> HandshakeResponse {
    let ours = PROTOCOL_VERSION_MAJOR;
    let accepted = request.min_supported_version <= ours && ours <= request.max_supported_version;

    HandshakeResponse {
        selected_version_major: if accepted { ours } else { 0 },
        selected_version_minor: if accepted { PROTOCOL_VERSION_MINOR } else { 0 },
        daemon_build_commit: option_env!("VITNA_BUILD_COMMIT")
            .unwrap_or_default()
            .to_string(),
        accepted,
        rejection_reason: if accepted {
            String::new()
        } else {
            format!(
                "this daemon speaks protocol major version {ours}; the client offered {}..={}",
                request.min_supported_version, request.max_supported_version
            )
        },
    }
}

/// Every stored event of the session's runs with a sequence after `after`, in
/// sequence order, or `None` if the session is gone.
///
/// Walks EVERY run in the session. Replaying only the latest run was how a
/// late subscriber was handed half a session.
fn session_events_after(
    daemon: &DaemonServer,
    session_id: &str,
    after: u64,
) -> Option<Vec<vitna_store::EventRecord>> {
    let runs = daemon.get_session(session_id)?.runs;
    let store = daemon.store.lock().ok()?;
    let mut events = Vec::new();
    for run in &runs {
        events.extend(store.events_after(run, after).unwrap_or_default());
    }
    // Runs number contiguously across the session, so this is already in
    // order when runs are sequential; the sort keeps it so if they ever are not.
    events.sort_by_key(|e| e.sequence);
    Some(events)
}

/// Sends one stored event as it was recorded. `false` once the connection is
/// gone, which ends the subscription.
async fn forward(out: &Outbound, session_id: &str, event: &vitna_store::EventRecord) -> bool {
    let envelope = ProtocolEnvelope::new(
        event.type_url.clone(),
        session_id.to_string(),
        event.run_id.clone(),
        event.sequence,
        String::new(),
        event.encrypted_payload.clone(),
    );
    out.send(envelope).await.is_ok()
}

/// Streams a session's events to one subscriber until the connection drops.
///
/// A session's events are numbered contiguously across all of its runs, so
/// one cursor, `last_sent`, follows the whole session. That is the contract the
/// protocol states (one `resume_after_sequence` per session) and every client
/// keeps (one cursor per session). When the engine numbered each run from 1
/// instead, a second run's opening events compared at or below the cursor and
/// were dropped, and the client would have discarded them as replays anyway.
///
/// Ordering is the whole job. The live feed is taken BEFORE the stored replay
/// is read, so an event appended in between is held in the channel rather than
/// falling between the two. That makes overlap possible instead of loss, and
/// overlap is removed by skipping anything at or below `last_sent`.
///
/// Membership is decided per event, against the session's run list as it is
/// at that moment. A run joins its session before it records anything (see
/// `DaemonServer::register_run`), so its events are recognised as they arrive.
///
/// `Lagged` is not a loss either: the broadcast reports how far a subscriber
/// fell behind, and the answer is to read that range again from the store,
/// which is the durable copy. The channel is an optimization over polling and
/// never the record.
async fn stream_events(
    daemon: Arc<DaemonServer>,
    out: Outbound,
    session_id: String,
    resume_after: u64,
) {
    let mut live = {
        let store = match daemon.store.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        store.subscribe()
    };

    if daemon.get_session(&session_id).is_none() {
        let _ = out
            .send(error_for(
                &ProtocolEnvelope::new(
                    type_url::command::SUBSCRIBE_EVENTS,
                    session_id.clone(),
                    String::new(),
                    CONTROL_SEQUENCE,
                    String::new(),
                    Vec::new(),
                ),
                ErrorCode::UnknownCommand,
                format!("no session {session_id}"),
            ))
            .await;
        return;
    }

    let mut last_sent = resume_after;

    let Some(replay) = session_events_after(&daemon, &session_id, last_sent) else {
        return;
    };
    for event in replay {
        if event.sequence <= last_sent {
            continue;
        }
        if !forward(&out, &session_id, &event).await {
            return;
        }
        last_sent = event.sequence;
    }

    loop {
        match live.recv().await {
            Ok(event) => {
                let belongs = daemon
                    .get_session(&session_id)
                    .is_some_and(|s| s.runs.contains(&event.run_id));
                if !belongs || event.sequence <= last_sent {
                    continue;
                }
                if !forward(&out, &session_id, &event).await {
                    return;
                }
                last_sent = event.sequence;
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                let Some(missed) = session_events_after(&daemon, &session_id, last_sent) else {
                    return;
                };
                for event in missed {
                    if event.sequence <= last_sent {
                        continue;
                    }
                    if !forward(&out, &session_id, &event).await {
                        return;
                    }
                    last_sent = event.sequence;
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Serves one connection until the client hangs up.
pub async fn serve_connection<S>(stream: S, daemon: Arc<DaemonServer>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (out, mut queue) = mpsc::channel::<ProtocolEnvelope>(WRITE_QUEUE_DEPTH);

    let pump = tokio::spawn(async move {
        while let Some(envelope) = queue.recv().await {
            if write_frame(&mut writer, &envelope).await.is_err() {
                return;
            }
        }
    });

    let mut shook_hands = false;

    while let Some(request) = read_frame(&mut reader).await? {
        let sent = match request.type_url.as_str() {
            type_url::envelope::HANDSHAKE_REQUEST => {
                let body: HandshakeRequest = match request.payload_as() {
                    Ok(b) => b,
                    Err(why) => {
                        let _ = out
                            .send(error_for(&request, ErrorCode::InvalidFrame, why))
                            .await;
                        continue;
                    }
                };
                let response = negotiate(&body);
                shook_hands = response.accepted;
                match ProtocolEnvelope::carrying(
                    type_url::envelope::HANDSHAKE_RESPONSE,
                    request.session_id.clone(),
                    request.run_id.clone(),
                    CONTROL_SEQUENCE,
                    request.idempotency_key.clone(),
                    &response,
                ) {
                    Ok(envelope) => out.send(envelope).await,
                    Err(e) => {
                        let _ = out
                            .send(error_for(&request, ErrorCode::Internal, e.to_string()))
                            .await;
                        continue;
                    }
                }
            }

            _ if !shook_hands => {
                out.send(error_for(
                    &request,
                    ErrorCode::Unauthorized,
                    "send HandshakeRequest before any other command",
                ))
                .await
            }

            type_url::command::SUBSCRIBE_EVENTS => {
                let body: SubscribeEvents = match request.payload_as() {
                    Ok(b) => b,
                    Err(why) => {
                        let _ = out
                            .send(error_for(&request, ErrorCode::InvalidFrame, why))
                            .await;
                        continue;
                    }
                };
                // The envelope's session_id is the route; the payload's is the
                // subject. They are the same field in practice, and the
                // payload wins because that is what the declaration names.
                let session_id = if body.session_id.is_empty() {
                    request.session_id.clone()
                } else {
                    body.session_id.clone()
                };
                tokio::spawn(stream_events(
                    Arc::clone(&daemon),
                    out.clone(),
                    session_id,
                    body.resume_after_sequence,
                ));
                Ok(())
            }

            type_url::command::CREATE_SESSION => {
                let body: CreateSession = match request.payload_as() {
                    Ok(b) => b,
                    Err(why) => {
                        let _ = out
                            .send(error_for(&request, ErrorCode::InvalidFrame, why))
                            .await;
                        continue;
                    }
                };
                if body.workspace_path.trim().is_empty() {
                    out.send(error_for(
                        &request,
                        ErrorCode::InvalidFrame,
                        "CreateSession needs a workspace_path",
                    ))
                    .await
                } else {
                    match daemon.create_session(&body.workspace_path) {
                        Err(why) => {
                            out.send(error_for(&request, ErrorCode::InvalidFrame, why))
                                .await
                        }
                        Ok(session) => {
                            let reply = SessionCreated {
                                session_id: session.session_id.clone(),
                                workspace_path: session
                                    .workspace_root
                                    .to_string_lossy()
                                    .into_owned(),
                                // Echoed, not applied: see `CreateSession`.
                                initial_mode: body.initial_mode.clone(),
                            };
                            match ProtocolEnvelope::carrying(
                                type_url::envelope::SESSION_CREATED,
                                session.session_id.clone(),
                                String::new(),
                                CONTROL_SEQUENCE,
                                request.idempotency_key.clone(),
                                &reply,
                            ) {
                                Ok(envelope) => out.send(envelope).await,
                                Err(e) => {
                                    out.send(error_for(
                                        &request,
                                        ErrorCode::Internal,
                                        e.to_string(),
                                    ))
                                    .await
                                }
                            }
                        }
                    }
                }
            }

            type_url::command::SUBMIT_TURN => {
                let body: SubmitTurn = match request.payload_as() {
                    Ok(b) => b,
                    Err(why) => {
                        let _ = out
                            .send(error_for(&request, ErrorCode::InvalidFrame, why))
                            .await;
                        continue;
                    }
                };
                let session_id = if body.session_id.is_empty() {
                    request.session_id.clone()
                } else {
                    body.session_id.clone()
                };
                if !body.attached_file_paths.is_empty() {
                    // Declared and not read yet. Refused rather than dropped,
                    // so a turn never runs without files its prompt refers to.
                    out.send(error_for(
                        &request,
                        ErrorCode::PolicyDenied,
                        "attached_file_paths is not served yet; name the files in the prompt instead",
                    ))
                    .await
                } else {
                    match daemon.submit_turn(&session_id, &body.prompt) {
                        // No declared reply: the turn's events, beginning with
                        // the run moving to running_model, are the answer, on
                        // the session's subscription.
                        Ok(_run_id) => Ok(()),
                        Err(why) => {
                            out.send(error_for(&request, ErrorCode::PolicyDenied, why))
                                .await
                        }
                    }
                }
            }

            type_url::command::APPROVE_ACTION | type_url::command::REJECT_ACTION => {
                let approving = request.type_url == type_url::command::APPROVE_ACTION;
                let parsed = if approving {
                    request.payload_as::<ApproveAction>().map(|b| {
                        let scope = b.scope.clone();
                        (b.approval_id, b.action_digest, Ok::<_, String>(scope))
                    })
                } else {
                    request
                        .payload_as::<RejectAction>()
                        .map(|b| (b.approval_id, b.action_digest, Err::<String, _>(b.reason)))
                };
                let (approval_id, action_digest, answer) = match parsed {
                    Ok(p) => p,
                    Err(why) => {
                        let _ = out
                            .send(error_for(&request, ErrorCode::InvalidFrame, why))
                            .await;
                        continue;
                    }
                };
                let decision = match answer {
                    Ok(scope) if scope.is_empty() || scope == "once" => {
                        Some(ApprovalDecision::Approved)
                    }
                    // Declared, and each would approve actions nobody has seen
                    // yet. Refused, and the request stays waiting for a "once".
                    Ok(_) => None,
                    Err(reason) => Some(ApprovalDecision::Rejected(if reason.trim().is_empty() {
                        "the operator refused this action".to_string()
                    } else {
                        format!("the operator refused this action: {reason}")
                    })),
                };
                match decision {
                    None => {
                        out.send(error_for_action(
                            &request,
                            ErrorCode::PolicyDenied,
                            "only scope \"once\" is served; \"session\" and \"project\" are declared and not implemented",
                            &approval_id,
                        ))
                        .await
                    }
                    Some(decision) => match daemon.answer_approval(&approval_id, &action_digest, decision) {
                        Ok(()) => Ok(()),
                        Err(AnswerRefused::NotWaiting) => {
                            out.send(error_for_action(
                                &request,
                                ErrorCode::PolicyDenied,
                                format!(
                                    "nothing is waiting under {approval_id}: it was answered, timed out, or never asked"
                                ),
                                &approval_id,
                            ))
                            .await
                        }
                        Err(AnswerRefused::DigestMismatch) => {
                            out.send(error_for_action(
                                &request,
                                ErrorCode::PolicyDenied,
                                "that digest is not the action waiting for approval; it is still waiting",
                                &approval_id,
                            ))
                            .await
                        }
                    },
                }
            }

            other => {
                let message = if type_url::is_declared(other) {
                    format!("{other} is declared but this daemon does not serve it yet")
                } else {
                    format!("{other} is not a declared command")
                };
                out.send(error_for(&request, ErrorCode::UnknownCommand, message))
                    .await
            }
        };

        if sent.is_err() {
            // The write half is gone, so there is nothing to answer on.
            break;
        }
    }

    drop(out);
    let _ = pump.await;
    Ok(())
}

#[cfg(unix)]
pub async fn serve<F>(daemon: Arc<DaemonServer>, endpoint: &str, on_ready: F) -> std::io::Result<()>
where
    F: FnOnce(),
{
    use std::os::unix::fs::PermissionsExt;

    let path = std::path::Path::new(endpoint);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        // 0700. The socket below is 0600, but a world-traversable directory
        // would let another account reach it by path regardless.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    // A stale socket from a killed daemon refuses binds forever.
    let _ = std::fs::remove_file(path);

    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    on_ready();

    loop {
        let (stream, _) = listener.accept().await?;
        let daemon = Arc::clone(&daemon);
        tokio::spawn(async move {
            let _ = serve_connection(stream, daemon).await;
        });
    }
}

#[cfg(windows)]
pub async fn serve<F>(daemon: Arc<DaemonServer>, endpoint: &str, on_ready: F) -> std::io::Result<()>
where
    F: FnOnce(),
{
    use tokio::net::windows::named_pipe::ServerOptions;

    // first_pipe_instance stops another process from squatting the name we
    // publish. What it does NOT do is tighten the DACL, which is the system
    // default here: restricting it to the owner needs windows-sys and is not
    // done yet. Stated rather than implied, because a reader would otherwise
    // take this line as the access control.
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(endpoint)?;
    on_ready();

    loop {
        server.connect().await?;
        let connected = server;
        server = ServerOptions::new().create(endpoint)?;

        let daemon = Arc::clone(&daemon);
        tokio::spawn(async move {
            let _ = serve_connection(connected, daemon).await;
        });
    }
}
