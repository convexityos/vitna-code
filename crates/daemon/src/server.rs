use ed25519_dalek::SigningKey;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use vitna_orchestration::{OrchestrationConfig, OrchestrationEngine};
use vitna_protocol::api::{SubmitTurnRequest, TurnResultResponse};
use vitna_providers::{AnthropicProvider, CredentialResolver, OpenAIProvider, Provider};
use vitna_receipts::generate_signing_key;
use vitna_runner::{ProcessRunner, Runner};
use vitna_store::EventStore;

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
            "vitna.v1.TurnStarted",
            &serde_json::json!({
                "prompt": req.prompt,
                "session_id": req.session_id,
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
