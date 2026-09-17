//! A blocking client for the daemon.
//!
//! Blocking on purpose. The desktop window is an immediate-mode UI with no
//! async runtime, and the daemon transport is a pipe or a socket that std can
//! open directly, so a caller runs these on a worker thread rather than
//! dragging tokio into the UI process. The daemon's own tests drive this same
//! client, so the path the window takes is the path that is covered.

use std::io::{Read, Write};

use crate::api::{
    self, CreateSessionRequest, ErrorResponse, HealthRequest, HealthResponse, ListSessionsRequest,
    SessionInfo, SessionListResponse, SubmitTurnRequest, TurnResultResponse,
};
use crate::{endpoint, ProtocolEnvelope};

trait Transport: Read + Write + Send {}
impl<T: Read + Write + Send> Transport for T {}

/// One connection to `vitna-coded`.
pub struct Client {
    stream: Box<dyn Transport>,
    endpoint: String,
    sequence: u64,
}

impl Client {
    /// Connects to the first declared endpoint that answers.
    ///
    /// The error lists every endpoint tried and why each refused, because
    /// "cannot connect" with no detail is the reading that sends someone
    /// restarting a daemon that was never installed.
    pub fn connect() -> Result<Self, String> {
        let tried = endpoint::candidates();
        if tried.is_empty() {
            return Err("no endpoint is defined on this platform".to_string());
        }

        let mut detail = String::new();
        for candidate in &tried {
            match Self::connect_to(candidate) {
                Ok(c) => return Ok(c),
                Err(e) => {
                    if !detail.is_empty() {
                        detail.push_str("; ");
                    }
                    detail.push_str(&format!("{candidate}: {e}"));
                }
            }
        }
        Err(detail)
    }

    pub fn connect_to(endpoint: &str) -> Result<Self, String> {
        let stream = open(endpoint)?;
        Ok(Self {
            stream,
            endpoint: endpoint.to_string(),
            sequence: 0,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Sends one request and reads its reply.
    ///
    /// An ERROR frame becomes an `Err` carrying the daemon's own message, so a
    /// caller never has to tell a refusal apart from a malformed reply.
    fn call<Req, Resp>(&mut self, type_url: &str, request: &Req) -> Result<Resp, String>
    where
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
    {
        self.sequence += 1;
        let envelope = ProtocolEnvelope::with_payload(
            type_url,
            "",
            "",
            self.sequence,
            format!("{}-{}", type_url, self.sequence),
            request,
        )
        .map_err(|e| format!("request could not be encoded: {e}"))?;

        envelope
            .write_frame(&mut self.stream)
            .map_err(|e| format!("sending to {} failed: {e}", self.endpoint))?;

        let reply = ProtocolEnvelope::decode_frame(&mut self.stream)
            .map_err(|e| format!("reading from {} failed: {e}", self.endpoint))?;

        if reply.type_url == api::type_url::ERROR {
            let err: ErrorResponse = reply
                .payload_as()
                .map_err(|e| format!("the daemon refused the call, and the reason did not parse: {e}"))?;
            return Err(err.message);
        }

        reply
            .payload_as()
            .map_err(|e| format!("the daemon's reply did not parse: {e}"))
    }

    pub fn health(&mut self) -> Result<HealthResponse, String> {
        self.call(api::type_url::HEALTH, &HealthRequest {})
    }

    pub fn create_session(
        &mut self,
        workspace_root: impl Into<std::path::PathBuf>,
    ) -> Result<SessionInfo, String> {
        self.call(
            api::type_url::CREATE_SESSION,
            &CreateSessionRequest {
                workspace_root: workspace_root.into(),
            },
        )
    }

    pub fn list_sessions(&mut self) -> Result<Vec<SessionInfo>, String> {
        let list: SessionListResponse =
            self.call(api::type_url::LIST_SESSIONS, &ListSessionsRequest {})?;
        Ok(list.sessions)
    }

    /// Runs a turn. This blocks for as long as the model and its tools take,
    /// which is why a UI calls it off the thread that paints.
    pub fn submit_turn(&mut self, request: &SubmitTurnRequest) -> Result<TurnResultResponse, String> {
        self.call(api::type_url::SUBMIT_TURN, request)
    }
}

#[cfg(windows)]
fn open(endpoint: &str) -> Result<Box<dyn Transport>, String> {
    use std::fs::OpenOptions;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(endpoint)
        .map_err(|e| e.to_string())?;
    Ok(Box::new(file))
}

#[cfg(unix)]
fn open(endpoint: &str) -> Result<Box<dyn Transport>, String> {
    use std::os::unix::net::UnixStream;
    let stream = UnixStream::connect(endpoint).map_err(|e| e.to_string())?;
    Ok(Box::new(stream))
}

#[cfg(not(any(windows, unix)))]
fn open(_endpoint: &str) -> Result<Box<dyn Transport>, String> {
    Err("this platform has no declared local IPC transport".to_string())
}
