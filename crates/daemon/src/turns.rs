//! A turn over the wire: `SubmitTurn` starts one, `ApproveAction` and
//! `RejectAction` answer what it asks, and the session's event stream carries
//! everything in between (ADR-0007).
//!
//! The model is reached through one route the daemon was started with, a
//! vitna-runtime lane (`vitna-coded --runtime-url ... --lane ...`). A daemon
//! started without one refuses `SubmitTurn` by saying so, rather than picking
//! a model nobody chose.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::oneshot;
use vitna_orchestration::{
    ApprovalDecision, ApprovalRequest, Approver, OrchestrationConfig, OrchestrationEngine,
    RunControl, MAX_ROUNDS,
};
use vitna_protocol::messages::{run_state, Diagnostic};
use vitna_protocol::type_url;
use vitna_providers::{Provider, ProviderMessage};
use vitna_tools::workspace_files::WorkspaceFiles;

use crate::DaemonServer;

/// Maximum number of files attached to a single turn.
const MAX_ATTACHED_FILES: usize = 16;

/// Maximum size of a single attached file (256 KiB).
const MAX_ATTACHMENT_BYTES: usize = 256 * 1024;

/// The model a daemon reaches, fixed when it starts.
#[derive(Clone)]
pub struct ModelRoute {
    pub provider: Arc<dyn Provider>,
    /// What goes in each request's `model` field: the lane alias.
    pub model: String,
    /// What the receipt records as the reason this model served.
    pub routing_reason: String,
}

/// One approval waiting for an operator.
pub struct PendingApproval {
    pub action_digest: String,
    pub run_id: String,
    answer: oneshot::Sender<ApprovalDecision>,
}

pub type PendingApprovals = Arc<Mutex<HashMap<String, PendingApproval>>>;

/// Why an answer to an approval was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerRefused {
    /// Nothing is waiting under that id: never asked, already answered, or
    /// timed out.
    NotWaiting,
    /// Something is waiting, for a different action. Left waiting, so the
    /// operator can answer the action they were actually shown.
    DigestMismatch,
}

/// Waits for `ApproveAction` or `RejectAction` naming this request, or for
/// `CancelRun` to abort the turn cleanly.
struct WireApprover {
    pending: PendingApprovals,
    run_id: String,
    control: Arc<RunControl>,
}

#[async_trait]
impl Approver for WireApprover {
    fn asks_a_person(&self) -> bool {
        true
    }

    async fn decide(&self, request: &ApprovalRequest) -> ApprovalDecision {
        if let Some(reason) = self.control.cancel_reason() {
            return ApprovalDecision::Rejected(format!("cancelled: {reason}"));
        }

        let (answer, waiting) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(
                request.approval_id.clone(),
                PendingApproval {
                    action_digest: request.action_digest.clone(),
                    run_id: self.run_id.clone(),
                    answer,
                },
            );
        } else {
            return ApprovalDecision::Rejected(
                "the daemon could not record the request".to_string(),
            );
        }

        tokio::select! {
            res = tokio::time::timeout(Duration::from_millis(request.timeout_ms), waiting) => {
                match res {
                    Ok(Ok(decision)) => decision,
                    Ok(Err(_)) => {
                        ApprovalDecision::Rejected("the daemon stopped waiting for an answer".to_string())
                    }
                    Err(_) => {
                        if let Ok(mut pending) = self.pending.lock() {
                            pending.remove(&request.approval_id);
                        }
                        ApprovalDecision::Rejected(format!(
                            "nobody answered within {} seconds, so the action was not taken",
                            request.timeout_ms / 1000
                        ))
                    }
                }
            }
            _ = self.control.cancelled() => {
                if let Ok(mut pending) = self.pending.lock() {
                    pending.remove(&request.approval_id);
                }
                let reason = self
                    .control
                    .cancel_reason()
                    .unwrap_or_else(|| "cancelled by operator".to_string());
                ApprovalDecision::Rejected(format!("cancelled: {reason}"))
            }
        }
    }
}

static TURN_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl DaemonServer {
    /// Serves turns through `route`.
    pub fn with_route(mut self, route: ModelRoute) -> Self {
        self.route = Some(route);
        self
    }

    /// Starts one turn in `session_id` with no attached files and returns its
    /// run id at once.
    pub fn submit_turn(&self, session_id: &str, prompt: &str) -> Result<String, String> {
        self.submit_turn_with_attachments(session_id, prompt, &[])
    }

    /// Starts one turn in `session_id` with optional workspace-contained file
    /// attachments and returns its run id at once.
    ///
    /// Every attached path is resolved and read through [`WorkspaceFiles`]
    /// before the turn begins, so path traversals, symlinks escaping the root,
    /// missing files, binary files, or oversized files are refused immediately
    /// rather than silently dropped.
    ///
    /// One turn per session at a time: a second `SubmitTurn` while one runs is
    /// refused, since two turns editing one folder would each be working from
    /// a tree the other is changing.
    pub fn submit_turn_with_attachments(
        &self,
        session_id: &str,
        prompt: &str,
        attached_file_paths: &[String],
    ) -> Result<String, String> {
        let route = self.route.clone().ok_or_else(|| {
            "this daemon has no model route. Start it with --runtime-url and --lane naming a \
             vitna-runtime lane on this machine"
                .to_string()
        })?;
        let session = self
            .get_session(session_id)
            .ok_or_else(|| format!("Session not found: {session_id}"))?;
        if prompt.trim().is_empty() {
            return Err("a turn needs a prompt".to_string());
        }
        if attached_file_paths.len() > MAX_ATTACHED_FILES {
            return Err(format!(
                "attached_file_paths exceeds the maximum of {MAX_ATTACHED_FILES} files"
            ));
        }

        let mut attachments = Vec::with_capacity(attached_file_paths.len());
        if !attached_file_paths.is_empty() {
            let ws = WorkspaceFiles::open(&session.workspace_root)?;
            for raw_path in attached_file_paths {
                if raw_path.trim().is_empty() {
                    return Err("attached_file_paths contains an empty path".to_string());
                }
                let canonical_rel = ws
                    .locate(raw_path)
                    .map_err(|e| format!("attached file '{raw_path}' refused: {e}"))?;
                let bytes = ws
                    .read(raw_path)
                    .map_err(|e| format!("attached file '{raw_path}' refused: {e}"))?;
                if bytes.len() > MAX_ATTACHMENT_BYTES {
                    return Err(format!(
                        "attached file '{raw_path}' is {} bytes, over the {MAX_ATTACHMENT_BYTES} byte attachment limit",
                        bytes.len()
                    ));
                }
                let text = String::from_utf8(bytes).map_err(|_| {
                    format!("attached file '{raw_path}' is not valid UTF-8 text")
                })?;
                attachments.push((canonical_rel, text));
            }
        }

        {
            let mut active = self.active_turns.lock().map_err(|e| e.to_string())?;
            if !active.insert(session_id.to_string()) {
                return Err(format!("a turn is already running in {session_id}"));
            }
        }

        let started = (|| -> Result<(String, OrchestrationEngine, Arc<RunControl>), String> {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let run_id = format!(
                "run-{}-{}",
                now_ms,
                TURN_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            );
            let config = OrchestrationConfig {
                session_id: session_id.to_string(),
                run_id: run_id.clone(),
                workspace_root: session.workspace_root.clone(),
                // Every mutating action is put to the operator; nothing is
                // approved by rule on this path.
                auto_approve: false,
                model_sku: route.model.clone(),
                // Off the adapter that will serve the turn, not off anything
                // a client said, so a receipt cannot claim a provider that was
                // merely asked for.
                provider_name: route.provider.name().to_string(),
                sandbox_guarantee: "guarded".to_string(),
                verification_command: None,
                allow_unsandboxed: false,
            };

            let control = Arc::new(RunControl::new());

            // Number first, then join the session, both before anything is
            // recorded: see `run_task`.
            let next_sequence = self.next_session_sequence(session_id)?;
            self.register_run(session_id, &run_id)?;

            let mut engine = OrchestrationEngine::new(
                config,
                self.store.clone(),
                self.runner.clone(),
                (*self.signing_key).clone(),
            )
            .continuing_at(next_sequence)
            .with_control(Arc::clone(&control))
            .with_approver(Arc::new(WireApprover {
                pending: self.approvals.clone(),
                run_id: run_id.clone(),
                control: Arc::clone(&control),
            }));
            engine.routing_reason = route.routing_reason.clone();
            engine.approval_timeout_ms = self.approval_timeout_ms;
            engine.record_event(
                vitna_orchestration::audit::TURN_STARTED,
                &serde_json::json!({
                    "prompt": prompt,
                    "session_id": session_id,
                    "run_id": run_id,
                    "attached_file_count": attachments.len(),
                }),
            )?;
            Ok((run_id, engine, control))
        })();

        let (run_id, engine, control) = match started {
            Ok(started) => started,
            Err(e) => {
                if let Ok(mut active) = self.active_turns.lock() {
                    active.remove(session_id);
                }
                return Err(e);
            }
        };

        if let Ok(mut controls) = self.active_controls.lock() {
            controls.insert(run_id.clone(), control);
        }

        let prior = self
            .histories
            .lock()
            .map(|h| h.get(session_id).cloned().unwrap_or_default())
            .unwrap_or_default();
        let daemon = self.clone();
        let (session_id, prompt, run_id_task) =
            (session_id.to_string(), prompt.to_string(), run_id.clone());
        tokio::spawn(async move {
            daemon
                .drive_turn(engine, route, prior, session_id, run_id_task, prompt, attachments)
                .await;
        });
        Ok(run_id)
    }

    /// Cancels an active run cooperatively.
    pub fn cancel_run(&self, run_id: &str, reason: &str) -> Result<(), String> {
        let control = self
            .active_controls
            .lock()
            .map_err(|e| e.to_string())?
            .get(run_id)
            .cloned()
            .ok_or_else(|| format!("no active run {run_id}"))?;
        let msg = if reason.trim().is_empty() {
            "cancelled by operator"
        } else {
            reason.trim()
        };
        control.cancel(msg);
        Ok(())
    }

    /// Queues operator steering feedback for an active run.
    pub fn steer_run(&self, run_id: &str, feedback: &str) -> Result<(), String> {
        if feedback.trim().is_empty() {
            return Err("SteerRun needs non-empty feedback".to_string());
        }
        let control = self
            .active_controls
            .lock()
            .map_err(|e| e.to_string())?
            .get(run_id)
            .cloned()
            .ok_or_else(|| format!("no active run {run_id}"))?;
        control.steer(feedback);
        Ok(())
    }

    /// Pauses an active run before its next model or tool step.
    pub fn pause_run(&self, run_id: &str, reason: &str) -> Result<(), String> {
        let control = self
            .active_controls
            .lock()
            .map_err(|e| e.to_string())?
            .get(run_id)
            .cloned()
            .ok_or_else(|| format!("no active run {run_id}"))?;
        let msg = if reason.trim().is_empty() {
            "paused by operator"
        } else {
            reason.trim()
        };
        control.pause(msg);
        Ok(())
    }

    /// Resumes a paused active run.
    pub fn resume_run(&self, run_id: &str) -> Result<(), String> {
        let control = self
            .active_controls
            .lock()
            .map_err(|e| e.to_string())?
            .get(run_id)
            .cloned()
            .ok_or_else(|| format!("no active run {run_id}"))?;
        control.resume();
        Ok(())
    }

    /// The turn, start to signed receipt to final state.
    #[allow(clippy::too_many_arguments)]
    async fn drive_turn(
        &self,
        mut engine: OrchestrationEngine,
        route: ModelRoute,
        prior: Vec<ProviderMessage>,
        session_id: String,
        run_id: String,
        prompt: String,
        attachments: Vec<(String, String)>,
    ) {
        let outcome = engine
            .run_agent_turn_with_attachments(
                route.provider.as_ref(),
                &prior,
                &prompt,
                &attachments,
                MAX_ROUNDS,
            )
            .await;

        // Signed whatever happened, since the rounds before a failure or
        // cancellation may already have changed the folder.
        let receipt = engine.finalize_run().await;

        let (state, reason) = match (&outcome, &receipt) {
            (_, Err(e)) => {
                let _ = engine.record_event(
                    type_url::event::DIAGNOSTIC,
                    &Diagnostic {
                        level: "error".to_string(),
                        code: "receipt_failed".to_string(),
                        message: format!("the run's receipt could not be signed: {e}"),
                        details: String::new(),
                    },
                );
                (run_state::FAILED, format!("receipt failed: {e}"))
            }
            (Err(e), Ok(_)) => (run_state::FAILED, e.clone()),
            (Ok(turn), Ok(_)) if turn.cancelled => (
                run_state::CANCELLED,
                turn.cancel_reason
                    .clone()
                    .unwrap_or_else(|| "cancelled by operator".to_string()),
            ),
            (Ok(turn), Ok(_)) if turn.hit_round_limit => (
                run_state::FAILED,
                format!("stopped at the round limit of {MAX_ROUNDS}"),
            ),
            (Ok(_), Ok(_)) => (run_state::COMPLETED, "the model finished".to_string()),
        };
        let _ = engine.set_run_state(state, &reason);

        if let Ok(turn) = &outcome {
            if !turn.cancelled {
                if let Ok(mut histories) = self.histories.lock() {
                    let history = histories.entry(session_id.clone()).or_default();
                    history.push(ProviderMessage::user(prompt));
                    history.push(ProviderMessage::assistant(turn.text.clone(), Vec::new()));
                }
            }
        }
        if let Ok(mut controls) = self.active_controls.lock() {
            controls.remove(&run_id);
        }
        if let Ok(mut active) = self.active_turns.lock() {
            active.remove(&session_id);
        }
    }

    /// Delivers an operator's answer to the approval waiting under
    /// `approval_id`, if `action_digest` is the one it was asked about.
    pub fn answer_approval(
        &self,
        approval_id: &str,
        action_digest: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AnswerRefused> {
        let mut pending = self
            .approvals
            .lock()
            .map_err(|_| AnswerRefused::NotWaiting)?;
        match pending.get(approval_id) {
            None => return Err(AnswerRefused::NotWaiting),
            Some(p) if p.action_digest != action_digest => {
                return Err(AnswerRefused::DigestMismatch)
            }
            Some(_) => {}
        }
        let waiting = pending
            .remove(approval_id)
            .ok_or(AnswerRefused::NotWaiting)?;
        // A send that fails means the turn already gave up on this request,
        // which is the same as nothing waiting.
        waiting
            .answer
            .send(decision)
            .map_err(|_| AnswerRefused::NotWaiting)
    }

    /// The run an approval belongs to, while it waits.
    pub fn approval_run(&self, approval_id: &str) -> Option<String> {
        self.approvals
            .lock()
            .ok()?
            .get(approval_id)
            .map(|p| p.run_id.clone())
    }
}
