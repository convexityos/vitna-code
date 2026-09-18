use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use vitna_orchestration::{OrchestrationConfig, OrchestrationEngine, StepType};
use vitna_receipts::{generate_signing_key, VitnaRunReceiptV1};
use vitna_runner::{ProcessRunner, Runner};
use vitna_store::EventStore;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub workspace_root: PathBuf,
    pub created_at_ms: u64,
    pub status: String,
    pub latest_run_id: Option<String>,
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

    /// Initializes a daemon server with a persistent SQLite WAL event store and default process runner.
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

        let signing_key = generate_signing_key();
        Ok(Self::new(store, Arc::new(runner), signing_key))
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
            model_sku: "claude-3-7-sonnet".to_string(),
            provider_name: "fake".to_string(),
            sandbox_guarantee: "guarded".to_string(),
            verification_command,
            allow_unsandboxed,
        };

        let mut engine = OrchestrationEngine::new(
            config,
            self.store.clone(),
            self.runner.clone(),
            (*self.signing_key).clone(),
        );

        // Record initial user task prompt in event store
        engine.record_event(
            "vitna.v1.TurnStarted",
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

        // Step 3: Finalize run and generate cryptographic receipt
        let receipt = engine.finalize_run().await?;

        // Update session with latest run ID
        if let Ok(mut sessions) = self.sessions.lock() {
            if let Some(s) = sessions.get_mut(session_id) {
                s.latest_run_id = Some(run_id);
            }
        }

        Ok(receipt)
    }
}
