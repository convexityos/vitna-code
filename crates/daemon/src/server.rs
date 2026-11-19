use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use vitna_orchestration::{
    OrchestrationConfig, OrchestrationEngine, RunControl, StepType, APPROVAL_TIMEOUT_MS,
};
use vitna_protocol::messages::MessageDelta;
use vitna_protocol::type_url;
use vitna_providers::ProviderMessage;

use crate::turns::{ModelRoute, PendingApprovals};
use vitna_receipts::VitnaRunReceiptV1;
use vitna_runner::{ProcessRunner, Runner};
use vitna_store::EventStore;
use vitna_telemetry::TelemetryCollector;

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub workspace_root: PathBuf,
    pub created_at_ms: u64,
    pub status: String,
    /// The run most recently STARTED in this session. Kept equal to the last
    /// entry of `runs`; see [`DaemonServer::register_run`].
    pub latest_run_id: Option<String>,
    /// Every run in this session, in the order they started.
    #[serde(default)]
    pub runs: Vec<String>,
}

#[derive(Clone)]
pub struct DaemonServer {
    pub store: Arc<Mutex<EventStore>>,
    pub sessions: Arc<Mutex<HashMap<String, SessionInfo>>>,
    pub runner: Arc<dyn Runner>,
    pub signing_key: Arc<SigningKey>,
    /// The model a turn reaches. `None` refuses `SubmitTurn`.
    pub route: Option<ModelRoute>,
    /// Approvals waiting for an operator, by approval id.
    pub approvals: PendingApprovals,
    /// How long an approval waits before it is refused.
    pub approval_timeout_ms: u64,
    /// Each session's conversation so far: every turn's prompt and the
    /// model's last message. Rebuilt from the durable event store on startup.
    pub histories: Arc<Mutex<HashMap<String, Vec<ProviderMessage>>>>,
    /// Sessions with a turn running now.
    pub active_turns: Arc<Mutex<HashSet<String>>>,
    /// Cooperative cancel/steer/pause handles for currently running runs, by run_id.
    pub active_controls: Arc<Mutex<HashMap<String, Arc<RunControl>>>>,
    /// Nesting depth of runs (0 for top-level turns, parent_depth + 1 for SpawnChild).
    pub run_depths: Arc<Mutex<HashMap<String, u32>>>,
    /// Local-only egress and metric collector (zero network I/O).
    pub telemetry: Arc<TelemetryCollector>,
    /// Active interactive PTY sessions keyed by session_id.
    pub terminals: Arc<Mutex<HashMap<String, Arc<vitna_runner::PtySession>>>>,
    /// Staged isolated child-workspace changesets awaiting ApplyChangeSet.
    pub pending_changesets: Arc<Mutex<HashMap<String, crate::changesets::StagedChangeSet>>>,
    /// Attached external MCP tool bridges injected into every turn's registry.
    pub mcp_tools: Arc<Mutex<Vec<Arc<dyn vitna_tools::Tool>>>>,
    /// Operator-configured destination hosts permitted for brokered
    /// `net:http_fetch` tool calls (`http_fetch` and remote `browser_verify`).
    pub egress_allowlist: Vec<String>,
}

impl DaemonServer {
    pub fn new(store: EventStore, runner: Arc<dyn Runner>, signing_key: SigningKey) -> Self {
        let mut restored_sessions = HashMap::new();
        let mut restored_histories: HashMap<String, Vec<ProviderMessage>> = HashMap::new();

        if let Ok(stored) = store.load_sessions() {
            for s in stored {
                let mut history = Vec::new();
                for run_id in &s.runs {
                    if let Ok(events) = store.get_events(run_id, 0) {
                        let mut prompt: Option<String> = None;
                        let mut last_model_message_id: Option<String> = None;
                        let mut last_model_text: Option<String> = None;
                        for ev in events {
                            if ev.type_url == vitna_orchestration::audit::TURN_STARTED {
                                if let Ok(v) =
                                    serde_json::from_slice::<serde_json::Value>(&ev.encrypted_payload)
                                {
                                    if let Some(p) = v.get("prompt").and_then(|p| p.as_str()) {
                                        prompt = Some(p.to_string());
                                    }
                                }
                            } else if ev.type_url == type_url::event::MESSAGE_DELTA {
                                if let Ok(delta) =
                                    serde_json::from_slice::<MessageDelta>(&ev.encrypted_payload)
                                {
                                    if delta.role == "model" && !delta.delta_text.is_empty() {
                                        if last_model_message_id.as_deref()
                                            == Some(delta.message_id.as_str())
                                        {
                                            if let Some(existing) = last_model_text.as_mut() {
                                                existing.push_str(&delta.delta_text);
                                            }
                                        } else {
                                            last_model_message_id = Some(delta.message_id);
                                            last_model_text = Some(delta.delta_text);
                                        }
                                    }
                                }
                            }
                        }
                        if let (Some(user_text), Some(model_text)) = (prompt, last_model_text) {
                            history.push(ProviderMessage::user(user_text));
                            history.push(ProviderMessage::assistant(model_text, Vec::new()));
                        }
                    }
                }
                if !history.is_empty() {
                    restored_histories.insert(s.session_id.clone(), history);
                }
                let latest_run_id = s.runs.last().cloned();
                restored_sessions.insert(
                    s.session_id.clone(),
                    SessionInfo {
                        session_id: s.session_id,
                        workspace_root: PathBuf::from(s.workspace_root),
                        created_at_ms: s.created_at_ms,
                        status: s.status,
                        latest_run_id,
                        runs: s.runs,
                    },
                );
            }
        }

        Self {
            store: Arc::new(Mutex::new(store)),
            sessions: Arc::new(Mutex::new(restored_sessions)),
            runner,
            signing_key: Arc::new(signing_key),
            route: None,
            approvals: Arc::new(Mutex::new(HashMap::new())),
            approval_timeout_ms: APPROVAL_TIMEOUT_MS,
            histories: Arc::new(Mutex::new(restored_histories)),
            active_turns: Arc::new(Mutex::new(HashSet::new())),
            active_controls: Arc::new(Mutex::new(HashMap::new())),
            run_depths: Arc::new(Mutex::new(HashMap::new())),
            telemetry: Arc::new(TelemetryCollector::new()),
            terminals: Arc::new(Mutex::new(HashMap::new())),
            pending_changesets: Arc::new(Mutex::new(HashMap::new())),
            mcp_tools: Arc::new(Mutex::new(Vec::new())),
            egress_allowlist: Vec::new(),
        }
    }

    pub fn with_egress_allowlist(mut self, allowlist: Vec<String>) -> Self {
        self.egress_allowlist = allowlist;
        self
    }

    pub fn telemetry(&self) -> Arc<TelemetryCollector> {
        Arc::clone(&self.telemetry)
    }

    /// Initializes a daemon server with a persistent SQLite WAL event store and
    /// default process runner, signing with this account's device key.
    ///
    /// The key is the one in this account's Vitna folder (`~/.vitna`), made the
    /// first time any daemon needs it, whichever store it opens: one account is
    /// one signer. It is never kept beside a store, since `vitna run` keeps its
    /// store inside the project it runs in, where a key could be committed.
    pub fn open_default<P: AsRef<Path>>(db_path: P) -> Result<Self, String> {
        Self::open_with_key_in(db_path, &crate::launch::vitna_home()?)
    }

    /// As [`open_default`](Self::open_default), with the device key kept in
    /// `key_dir`, which tests point at a folder of their own.
    pub fn open_with_key_in<P: AsRef<Path>>(db_path: P, key_dir: &Path) -> Result<Self, String> {
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

        std::fs::create_dir_all(key_dir)
            .map_err(|e| format!("could not make the folder for the device key {}: {e}", key_dir.display()))?;
        let signing_key = crate::key::load_or_create(key_dir)?;
        Ok(Self::new(store, Arc::new(runner), signing_key))
    }

    /// The public half of the key this daemon signs receipts with, in hex: what
    /// anyone checking one of its receipts checks the signature against.
    pub fn device_public_key(&self) -> String {
        hex::encode(self.signing_key.verifying_key().to_bytes())
    }

    /// Creates a new workspace-bound session and persists it to the event store.
    pub fn create_session<P: AsRef<Path>>(&self, workspace_root: P) -> Result<SessionInfo, String> {
        let ws = workspace_root.as_ref().to_path_buf();
        if !ws.exists() {
            return Err(format!("Workspace root does not exist: {}", ws.display()));
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let seq = SESSION_COUNTER.fetch_add(1, Ordering::SeqCst);
        let session_id = format!("sess-{now_ms}-{seq}");

        let info = SessionInfo {
            session_id: session_id.clone(),
            workspace_root: ws.clone(),
            created_at_ms: now_ms,
            status: "active".to_string(),
            latest_run_id: None,
            runs: Vec::new(),
        };

        {
            let store = self.store.lock().map_err(|e| e.to_string())?;
            store
                .save_session(
                    &session_id,
                    &ws.to_string_lossy(),
                    now_ms,
                    &info.status,
                )
                .map_err(|e| format!("Failed to persist session: {e}"))?;
        }

        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        sessions.insert(session_id, info.clone());

        Ok(info)
    }

    /// Retrieves session information by ID.
    pub fn get_session(&self, session_id: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.lock().ok()?;
        sessions.get(session_id).cloned()
    }

    /// Records that `run_id` belongs to `session_id`, durably and in memory, as
    /// the session's latest.
    pub fn register_run(&self, session_id: &str, run_id: &str) -> Result<(), String> {
        let ordinal = {
            let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            let session = sessions
                .get_mut(session_id)
                .ok_or_else(|| format!("Session not found: {session_id}"))?;
            let ord = session.runs.len() as u64;
            session.runs.push(run_id.to_string());
            session.latest_run_id = Some(run_id.to_string());
            ord
        };
        let store = self.store.lock().map_err(|e| e.to_string())?;
        store
            .register_session_run(session_id, run_id, ordinal)
            .map_err(|e| format!("Failed to persist run registration: {e}"))?;
        Ok(())
    }

    /// The sequence the session's next event should carry.
    ///
    /// One past the highest sequence any of its runs actually recorded, read
    /// from the store, or `FIRST_EVENT_SEQUENCE` for a session with nothing
    /// recorded yet. Read across every run rather than only the latest, so a
    /// run that started and recorded nothing cannot reset the count.
    pub fn next_session_sequence(&self, session_id: &str) -> Result<u64, String> {
        let runs = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {session_id}"))?
            .runs;
        let store = self.store.lock().map_err(|e| e.to_string())?;
        let mut highest: Option<u64> = None;
        for run in &runs {
            if let Some(last) = store.last_sequence(run).map_err(|e| e.to_string())? {
                highest = Some(highest.map_or(last, |h| h.max(last)));
            }
        }
        Ok(highest.map_or(vitna_protocol::FIRST_EVENT_SEQUENCE, |h| h + 1))
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

    /// Finds the session that owns `run_id`, if any.
    pub fn session_for_run(&self, run_id: &str) -> Option<SessionInfo> {
        let sessions = self.sessions.lock().ok()?;
        sessions
            .values()
            .find(|s| s.runs.iter().any(|r| r == run_id))
            .cloned()
    }

    /// Executes a durable task run end-to-end within the specified session.
    pub async fn run_task(
        &self,
        session_id: &str,
        prompt: &str,
        auto_approve: bool,
        verification_command: Option<String>,
        // Explicit operator approval to run commands with no OS sandbox. Kept
        // as its own parameter so a caller has to say it out loud.
        allow_unsandboxed: bool,
    ) -> Result<VitnaRunReceiptV1, String> {
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let run_id = format!("run-{}", now_ms);

        let config = OrchestrationConfig {
            session_id: session_id.to_string(),
            run_id: run_id.clone(),
            workspace_root: session.workspace_root.clone(),
            auto_approve,
            // No model runs on this path: the one edit below is picked by
            // looking for a keyword in the prompt. So the receipt names no
            // provider and calls the stand-in what it is. This used to sign
            // "claude-3-7-sonnet" from a provider named "fake", so every
            // `vitna run` attested to a model that never served it. The v1
            // schema types both fields as plain strings, so neither value
            // needed a change to it.
            model_sku: "keyword-stub".to_string(),
            provider_name: "none".to_string(),
            sandbox_guarantee: "guarded".to_string(),
            verification_command,
            allow_unsandboxed,
        };

        // In this order, and both before anything is recorded. The number is
        // read from the session's EXISTING runs, so it has to come first; then
        // the run joins the session, so a subscriber following the session
        // recognises this run's events as they arrive rather than after the
        // run is over.
        let next_sequence = self.next_session_sequence(session_id)?;
        self.register_run(session_id, &run_id)?;

        let mut engine = OrchestrationEngine::new(
            config,
            self.store.clone(),
            self.runner.clone(),
            (*self.signing_key).clone(),
        )
        .continuing_at(next_sequence);

        // Record initial user task prompt in event store
        engine.record_event(
            vitna_orchestration::audit::TURN_STARTED,
            &serde_json::json!({
                "prompt": prompt,
                "session_id": session_id,
                "run_id": run_id,
            }),
        )?;

        // Step 1: Inspect workspace
        engine.execute_task_step(StepType::InspectWorkspace).await?;

        // Step 2: If prompt indicates file creation/modification, propose edit
        if prompt.contains("create") || prompt.contains("edit") || prompt.contains("write") || prompt.contains("add") {
            // Generate deterministic sample edit based on task prompt
            let sample_file = "vitna_output.txt";
            let sample_content = format!("Task executed by Vitna Code:\n{}\nTimestamp: {}\n", prompt, now_ms);

            engine.execute_task_step(StepType::ProposeEdit {
                path: sample_file.to_string(),
                content: sample_content,
            }).await?;
        }

        // Step 3: Finalize run and generate cryptographic receipt. The run was
        // registered with its session when it started, not here.
        let receipt = engine.finalize_run().await?;

        Ok(receipt)
    }
}
