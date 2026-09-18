//! The daemon's request and response payloads.
//!
//! These live here rather than in `vitna-daemon` because the window has to
//! speak them too. `vitna-protocol` is the one crate both sides can carry:
//! the daemon pulls in the event store, the runner and the providers, and a
//! window that depended on it would link the whole backend into the UI.
//!
//! Every payload travels as the `payload` bytes of a [`crate::ProtocolEnvelope`],
//! whose `type_url` says which of these it is. A call that fails comes back
//! under [`type_url::ERROR`] carrying an [`ErrorResponse`], so a caller never
//! has to infer failure from a missing field.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The `type_url` each payload travels under.
pub mod type_url {
    pub const HEALTH: &str = "type.vitna.ai/vitna.protocol.v1.Health";
    pub const HEALTH_RESULT: &str = "type.vitna.ai/vitna.protocol.v1.HealthResult";
    pub const CREATE_SESSION: &str = "type.vitna.ai/vitna.protocol.v1.CreateSession";
    pub const SESSION: &str = "type.vitna.ai/vitna.protocol.v1.Session";
    pub const LIST_SESSIONS: &str = "type.vitna.ai/vitna.protocol.v1.ListSessions";
    pub const SESSION_LIST: &str = "type.vitna.ai/vitna.protocol.v1.SessionList";
    pub const SUBMIT_TURN: &str = "type.vitna.ai/vitna.protocol.v1.SubmitTurn";
    pub const TURN_RESULT: &str = "type.vitna.ai/vitna.protocol.v1.TurnResult";
    pub const LIST_TURNS: &str = "type.vitna.ai/vitna.protocol.v1.ListTurns";
    pub const TURN_LIST: &str = "type.vitna.ai/vitna.protocol.v1.TurnList";
    pub const ERROR: &str = "type.vitna.ai/vitna.protocol.v1.Error";
}

/// A workspace-bound session, as the daemon holds it.
///
/// Declared here so the daemon and the window share one definition rather than
/// two that drift.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub workspace_root: PathBuf,
    pub created_at_ms: u64,
    pub status: String,
    pub latest_run_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthRequest {}

/// What the daemon can say about itself without being asked to do work.
///
/// `providers_ready` names the providers whose credentials actually resolved,
/// so the window states a checked fact rather than counting configured names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub version: String,
    pub pid: u32,
    pub endpoint: String,
    pub providers_ready: Vec<String>,
    pub session_count: usize,
    /// The public half of the key this daemon signs receipts with, as hex. A
    /// client checks a receipt's `device_signature` against it. `None` from a
    /// daemon too old to say, which is not the same as "unsigned".
    #[serde(default)]
    pub device_public_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListSessionsRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionListResponse {
    pub sessions: Vec<SessionInfo>,
}

/// A turn to run. `provider` and `model_sku` are the window's PREFERENCE; the
/// daemon resolves them against the credentials it can actually reach and the
/// receipt reports what ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitTurnRequest {
    pub session_id: String,
    pub prompt: String,
    pub provider: Option<String>,
    pub model_sku: Option<String>,
    pub auto_approve: bool,
    pub verification_command: Option<String>,
}

/// The outcome of a turn, summarised for a caller that does not carry
/// `vitna-receipts`. `receipt_path` is how that caller reaches the real thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnResultResponse {
    pub run_id: String,
    pub receipt_path: PathBuf,
    /// The provider and sku that actually served the turn, read back off the
    /// receipt rather than echoed from the request.
    pub provider: String,
    pub model_sku: String,
    pub completion_state: String,
    pub files_modified: Vec<String>,
    /// None when the provider did not report usage. Zero would be a claim, and
    /// a wrong one: it is the value a free turn would carry.
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    /// The assistant's closing message, for the transcript.
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListTurnsRequest {}

/// One turn, as the daemon's event log recorded it when the turn began.
///
/// A receipt carries no prompt, on purpose: it records what was done, not
/// what was asked. So the prompt a person typed exists in exactly one place,
/// the `TurnStarted` event the daemon writes before the model is called, and
/// this is how a window reads it back rather than remembering what it sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnInfo {
    pub run_id: String,
    pub session_id: String,
    pub prompt: String,
    /// When the event was written, off the event itself.
    pub started_at_ms: u64,
}

/// A `TurnStarted` event the daemon could not read back: its hash did not
/// match its contents, or its payload did not parse. Reported rather than
/// dropped, so a missing title is never mistaken for a turn that never ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnreadableTurn {
    pub run_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnListResponse {
    /// Oldest first.
    pub turns: Vec<TurnInfo>,
    pub unreadable: Vec<UnreadableTurn>,
}

/// A call that failed, with the reason the caller should show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub message: String,
}
