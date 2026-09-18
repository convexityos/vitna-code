use ed25519_dalek::SigningKey;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use vitna_orchestration::{OrchestrationConfig, OrchestrationEngine, RECEIPT_GENERATED};
use vitna_protocol::api::{
    SubmitTurnRequest, TurnInfo, TurnListResponse, TurnResultResponse, UnreadableTurn,
};
use vitna_providers::{AnthropicProvider, CredentialResolver, OpenAIProvider, Provider};
use vitna_runner::{ProcessRunner, Runner};
use vitna_store::{EventRecord, EventStore, GENESIS_HASH};

/// Declared in `vitna-protocol` so the window shares the definition rather than
/// carrying a second one that can drift.
pub use vitna_protocol::api::SessionInfo;

/// How many times one turn may ask the model before the loop stops.
///
/// A cap has to exist, because a model that keeps calling tools without
/// concluding would otherwise run until the credentials do. Twenty-four is
/// enough for ordinary multi-file work and is reported when it is reached, so
/// a cut-off run is never presented as a finished one.
const MAX_ROUNDS: usize = 24;

/// The event a turn opens with, and the only record of its prompt. One
/// constant for the writer and the reader, since a reader looking for a type
/// the writer no longer uses would find no turns and report none, which reads
/// exactly like a daemon that has never run one.
pub const TURN_STARTED: &str = "vitna.v1.TurnStarted";

/// The event a session opens with, so that it outlives the daemon that made
/// it, turns or no turns. Written once per session, as a one-event chain keyed
/// by the session id, since a session is not a run.
pub const SESSION_CREATED: &str = "vitna.v1.SessionCreated";

/// What rebuilding the session list from the event log found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Restored {
    pub sessions: usize,
    /// Events that failed their hash or would not parse, and so placed
    /// nothing. Counted rather than skipped in silence.
    pub unreadable: usize,
    /// Sessions whose turns are logged with no workspace anywhere in the log,
    /// so there is nothing to bind them to. Named, not guessed at.
    pub unplaced: Vec<String>,
}

/// Builds the provider the caller asked for, or the first one whose credential
/// actually resolves.
///
/// The error names the variables that were absent, because "no provider
/// available" sends someone looking in the wrong place.
fn resolve_provider(preferred: Option<&str>) -> Result<Box<dyn Provider>, String> {
    match preferred.map(|p| p.trim().to_ascii_lowercase()) {
        Some(p) if p == "anthropic" => Ok(Box::new(AnthropicProvider::new(
            CredentialResolver::resolve_anthropic()?,
        ))),
        Some(p) if p == "openai" => Ok(Box::new(OpenAIProvider::new(
            CredentialResolver::resolve_openai()?,
        ))),
        Some(other) => Err(format!(
            "no adapter for provider '{other}'; this build carries anthropic and openai"
        )),
        None => {
            if let Ok(c) = CredentialResolver::resolve_anthropic() {
                return Ok(Box::new(AnthropicProvider::new(c)));
            }
            if let Ok(c) = CredentialResolver::resolve_openai() {
                return Ok(Box::new(OpenAIProvider::new(c)));
            }
            Err("no provider credential is set: neither ANTHROPIC_API_KEY nor OPENAI_API_KEY is present in the daemon's environment".to_string())
        }
    }
}

#[derive(Clone)]
pub struct DaemonServer {
    pub store: Arc<Mutex<EventStore>>,
    pub sessions: Arc<Mutex<HashMap<String, SessionInfo>>>,
    pub runner: Arc<dyn Runner>,
    pub signing_key: Arc<SigningKey>,
}

impl DaemonServer {
    pub fn new(store: EventStore, runner: Arc<dyn Runner>, signing_key: SigningKey) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            runner,
            signing_key: Arc::new(signing_key),
        }
    }

    /// Initializes a daemon server with a persistent SQLite WAL event store,
    /// the default process runner, and the signing key kept beside the store
    /// (see [`crate::key`]), made on the first start and read on every other.
    pub fn open_default<P: AsRef<Path>>(db_path: P) -> Result<Self, String> {
        let parent = db_path.as_ref().parent();
        if let Some(p) = parent {
            let _ = std::fs::create_dir_all(p);
        }

        let store = EventStore::open(db_path.as_ref())
            .map_err(|e| format!("Failed to open event store: {}", e))?;

        let journal_path = parent
            .unwrap_or_else(|| Path::new("."))
            .join("vitna_runner.journal");

        let runner = ProcessRunner::open_or_create(journal_path)
            .map_err(|e| format!("Failed to initialize process runner journal: {}", e))?;

        let signing_key = crate::key::load_or_create(parent.unwrap_or_else(|| Path::new(".")))?;
        let server = Self::new(store, Arc::new(runner), signing_key);

        let restored = server.restore_sessions()?;
        tracing::info!(sessions = restored.sessions, "sessions restored from the event log");
        if restored.unreadable > 0 {
            tracing::warn!(events = restored.unreadable, "session events that failed their hash or would not parse");
        }
        if !restored.unplaced.is_empty() {
            tracing::warn!(
                sessions = ?restored.unplaced,
                "sessions whose turns are logged with no workspace anywhere in the log, left out"
            );
        }
        Ok(server)
    }

    /// The public half of the key receipts are signed with, as hex: what
    /// `Health` publishes so that a receipt can be checked by someone other
    /// than the daemon that wrote it.
    pub fn device_public_key(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    /// Creates a new workspace-bound session.
    pub fn create_session<P: AsRef<Path>>(&self, workspace_root: P) -> Result<SessionInfo, String> {
        let ws = workspace_root.as_ref().to_path_buf();
        if !ws.exists() {
            return Err(format!("Workspace root does not exist: {}", ws.display()));
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let session_id = format!("sess-{}", now_ms);

        // Logged before it is listed: a session the log does not hold is gone
        // at the next restart, and nothing would say it had ever existed.
        let payload = serde_json::to_vec(&serde_json::json!({
            "session_id": session_id,
            "workspace_root": ws,
            "created_at_ms": now_ms,
        }))
        .map_err(|e| format!("the session could not be recorded: {e}"))?;
        let event = EventRecord::new(
            format!("ev-{session_id}-0"),
            &session_id,
            0,
            SESSION_CREATED,
            now_ms,
            payload,
            GENESIS_HASH,
        );
        self.store
            .lock()
            .map_err(|e| e.to_string())?
            .append_event(&event)
            .map_err(|e| format!("the session could not be recorded in the event log: {e}"))?;

        let info = SessionInfo {
            session_id: session_id.clone(),
            workspace_root: ws,
            created_at_ms: now_ms,
            status: "active".to_string(),
            latest_run_id: None,
        };

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        sessions.insert(session_id, info.clone());

        Ok(info)
    }

    /// Retrieves session information by ID.
    pub fn get_session(&self, session_id: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.lock().ok()?;
        sessions.get(session_id).cloned()
    }

    /// Lists all known active sessions.
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        let sessions = match self.sessions.lock() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut list: Vec<SessionInfo> = sessions.values().cloned().collect();
        // Newest session first.
        list.sort_by_key(|s| std::cmp::Reverse(s.created_at_ms));
        list
    }

    /// Rebuilds the session list from the event log, which is what lets a
    /// session outlive the daemon that made it. The map in memory forgot every
    /// session on restart, so a window showed "No sessions yet" above the runs
    /// those sessions had made.
    ///
    /// A session is read from its `SessionCreated` event. One made before that
    /// event existed is rebuilt from its turns instead, placed at the workspace
    /// its turn names or, for a turn logged before turns named one, at the one
    /// its receipt was written under (`<workspace>/.vitna/receipts/<run>.json`,
    /// as `ReceiptGenerated` records it). A session nothing in the log places
    /// is left out and named in the report, not given a guessed workspace.
    /// Sessions already in memory are kept as they are.
    pub fn restore_sessions(&self) -> Result<Restored, String> {
        let (created, turns, receipts) = {
            let store = self.store.lock().map_err(|e| e.to_string())?;
            let read = |type_url: &str| {
                store
                    .events_of_type(type_url)
                    .map_err(|e| format!("the event log could not be read: {e}"))
            };
            (read(SESSION_CREATED)?, read(TURN_STARTED)?, read(RECEIPT_GENERATED)?)
        };
        let payload = |event: &EventRecord| -> Option<serde_json::Value> {
            event
                .verify_integrity()
                .then(|| serde_json::from_slice(&event.encrypted_payload).ok())
                .flatten()
        };
        let text = |v: &serde_json::Value, key: &str| v.get(key).and_then(|x| x.as_str()).map(str::to_string);

        let mut report = Restored::default();
        let mut found: HashMap<String, SessionInfo> = HashMap::new();
        for event in &created {
            let info = payload(event).and_then(|v| {
                Some(SessionInfo {
                    session_id: text(&v, "session_id")?,
                    workspace_root: PathBuf::from(text(&v, "workspace_root")?),
                    created_at_ms: v.get("created_at_ms")?.as_u64()?,
                    status: "active".to_string(),
                    latest_run_id: None,
                })
            });
            match info {
                Some(info) => {
                    found.insert(info.session_id.clone(), info);
                }
                None => report.unreadable += 1,
            }
        }

        // Where each run's receipt went: the workspace, three levels up.
        let receipt_workspace: HashMap<String, PathBuf> = receipts
            .iter()
            .filter_map(|event| {
                let path = PathBuf::from(text(&payload(event)?, "receipt_path")?);
                Some((event.run_id.clone(), path.parent()?.parent()?.parent()?.to_path_buf()))
            })
            .collect();

        // Oldest first, so the last turn seen for a session is its latest.
        let mut from_turns: HashMap<String, SessionInfo> = HashMap::new();
        for event in &turns {
            let Some(v) = payload(event) else {
                report.unreadable += 1;
                continue;
            };
            let Some(session_id) = text(&v, "session_id") else {
                report.unreadable += 1;
                continue;
            };
            let workspace = text(&v, "workspace_root")
                .map(PathBuf::from)
                .or_else(|| receipt_workspace.get(&event.run_id).cloned());
            let session = from_turns.entry(session_id.clone()).or_insert_with(|| SessionInfo {
                session_id,
                workspace_root: PathBuf::new(),
                created_at_ms: event.timestamp_ms,
                status: "active".to_string(),
                latest_run_id: None,
            });
            session.latest_run_id = Some(event.run_id.clone());
            if session.workspace_root.as_os_str().is_empty() {
                if let Some(ws) = workspace {
                    session.workspace_root = ws;
                }
            }
        }
        for (id, turned) in from_turns {
            match found.get_mut(&id) {
                Some(session) => session.latest_run_id = turned.latest_run_id,
                None if turned.workspace_root.as_os_str().is_empty() => report.unplaced.push(id),
                None => {
                    found.insert(id, turned);
                }
            }
        }
        report.unplaced.sort();

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        for (id, session) in found {
            if let std::collections::hash_map::Entry::Vacant(slot) = sessions.entry(id) {
                slot.insert(session);
                report.sessions += 1;
            }
        }
        Ok(report)
    }

    /// Every turn the event log holds, read off its `TurnStarted` events.
    ///
    /// The log is the source rather than the session map, because the map is
    /// in memory and forgets on restart while the log does not. An event whose
    /// hash no longer matches its contents, or whose payload will not parse, is
    /// reported under `unreadable` rather than skipped.
    pub fn list_turns(&self) -> Result<TurnListResponse, String> {
        let events = {
            let store = self.store.lock().map_err(|e| e.to_string())?;
            store
                .events_of_type(TURN_STARTED)
                .map_err(|e| format!("the event log could not be read: {e}"))?
        };

        let mut list = TurnListResponse::default();
        for event in events {
            if !event.verify_integrity() {
                list.unreadable.push(UnreadableTurn {
                    run_id: event.run_id,
                    reason: "the event's hash does not match its contents".to_string(),
                });
                continue;
            }
            let read = serde_json::from_slice::<serde_json::Value>(&event.encrypted_payload)
                .map_err(|e| format!("the payload is not JSON: {e}"))
                .and_then(|v| {
                    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
                    match (field("prompt"), field("session_id")) {
                        (Some(prompt), Some(session_id)) => Ok((prompt, session_id)),
                        _ => Err("the payload names no prompt or no session".to_string()),
                    }
                });
            match read {
                Ok((prompt, session_id)) => list.turns.push(TurnInfo {
                    run_id: event.run_id,
                    session_id,
                    prompt,
                    started_at_ms: event.timestamp_ms,
                }),
                Err(reason) => list.unreadable.push(UnreadableTurn {
                    run_id: event.run_id,
                    reason,
                }),
            }
        }
        Ok(list)
    }

    /// Runs one turn: a real model call, the tools it asks for, and a signed
    /// receipt naming what actually served it.
    ///
    /// What this replaced is worth stating, because it looked like a working
    /// run. The previous version matched keywords in the prompt ("create",
    /// "edit", "write", "add") and, on a hit, wrote a fixed `vitna_output.txt`
    /// carrying the prompt back. No model was ever asked anything, and the
    /// receipt recorded a provider named "fake" because that string was a
    /// hardcoded default rather than an observation.
    pub async fn run_turn(&self, req: &SubmitTurnRequest) -> Result<TurnResultResponse, String> {
        let provider = resolve_provider(req.provider.as_deref())?;
        self.run_turn_with(provider.as_ref(), req).await
    }

    /// The turn itself, against a provider the caller supplies.
    ///
    /// Split from `run_turn` so a test can drive the loop with a scripted
    /// provider. The alternative was an environment variable that swapped in a
    /// fake, which is a synthetic-data path sitting in the production binary
    /// waiting to be set by accident.
    pub async fn run_turn_with(
        &self,
        provider: &dyn Provider,
        req: &SubmitTurnRequest,
    ) -> Result<TurnResultResponse, String> {
        let session = self
            .get_session(&req.session_id)
            .ok_or_else(|| format!("Session not found: {}", req.session_id))?;

        // No default sku. The catalog lives with the caller, and a model the
        // daemon picked on its own would be a choice nobody made appearing in
        // a receipt as though somebody had.
        let model_sku = req
            .model_sku
            .clone()
            .filter(|m| !m.trim().is_empty())
            .ok_or_else(|| "no model was chosen: the caller must name a model_sku".to_string())?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let run_id = format!("run-{}", now_ms);

        let config = OrchestrationConfig {
            session_id: req.session_id.clone(),
            run_id: run_id.clone(),
            workspace_root: session.workspace_root.clone(),
            auto_approve: req.auto_approve,
            model_sku,
            // The name comes off the adapter that will serve the turn, not off
            // the request, so the receipt cannot claim a provider that was
            // merely asked for.
            provider_name: provider.name().to_string(),
            sandbox_guarantee: "guarded".to_string(),
            verification_command: req.verification_command.clone(),
        };

        let mut engine = OrchestrationEngine::new(
            config,
            self.store.clone(),
            self.runner.clone(),
            (*self.signing_key).clone(),
        );

        engine.record_event(
            TURN_STARTED,
            &serde_json::json!({
                "prompt": req.prompt,
                "session_id": req.session_id,
                "workspace_root": session.workspace_root,
                "run_id": run_id,
                "auto_approve": req.auto_approve,
            }),
        )?;

        let turn = engine
            .run_agent_turn(provider, &req.prompt, MAX_ROUNDS)
            .await?;

        let receipt = engine.finalize_run().await?;

        if let Ok(mut sessions) = self.sessions.lock() {
            if let Some(s) = sessions.get_mut(&req.session_id) {
                s.latest_run_id = Some(run_id.clone());
            }
        }

        Ok(TurnResultResponse {
            run_id: receipt.run_id.clone(),
            receipt_path: session
                .workspace_root
                .join(".vitna")
                .join("receipts")
                .join(format!("{}.json", receipt.run_id)),
            // Read back off the signed receipt rather than echoed from the
            // request, so the two cannot disagree.
            provider: receipt.model_selection.provider.clone(),
            model_sku: receipt.model_selection.model_sku.clone(),
            completion_state: receipt.completion_state.clone(),
            files_modified: receipt
                .changeset
                .files_modified
                .iter()
                .map(|f| f.path.clone())
                .collect(),
            prompt_tokens: turn.prompt_tokens,
            completion_tokens: turn.completion_tokens,
            text: turn.text,
        })
    }
}
