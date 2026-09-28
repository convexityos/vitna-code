use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use vitna_context::{ContextAssembler, ContextMessage};
use vitna_receipts::{
    ChangeSetRecord, EvidenceItemRecord, FileModificationRecord, ModelSelectionRecord,
    RunnerExecutionStatementRecord, VitnaRunReceiptV1,
};
use vitna_runner::Runner;
use vitna_store::{EventRecord, EventStore, GENESIS_HASH};
use vitna_tools::{ToolContext, ToolRegistry, ToolResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationConfig {
    pub session_id: String,
    pub run_id: String,
    pub workspace_root: PathBuf,
    pub auto_approve: bool,
    pub model_sku: String,
    pub provider_name: String,
    pub sandbox_guarantee: String,
    pub verification_command: Option<String>,
    /// Explicit operator approval to run commands with no OS sandbox.
    ///
    /// Deliberately not folded into `auto_approve`. Approving the actions a run
    /// wants to take and approving that they run outside a sandbox are
    /// different decisions, and `serde` defaults this to false so a config
    /// written before this field existed does not silently grant it.
    #[serde(default)]
    pub allow_unsandboxed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepType {
    InspectWorkspace,
    ProposeEdit { path: String, content: String },
    RunVerification { command: String },
}

pub struct OrchestrationEngine {
    pub config: OrchestrationConfig,
    pub store: Arc<Mutex<EventStore>>,
    pub runner: Arc<dyn Runner>,
    pub tool_registry: ToolRegistry,
    pub context_assembler: ContextAssembler,
    pub signing_key: SigningKey,
    pub sequence_counter: u64,
    /// How many tool calls this run has made, for minting `tool_call_id`.
    ///
    /// The declared events link an approval to the tool it gates and to that
    /// tool's start and finish through this id, so a client can tell which
    /// tool an approval answered. Nothing linked them before it existed.
    pub tool_calls: u64,
    /// How many approvals this run has asked for, for minting `approval_id`.
    pub approvals: u64,
    pub last_event_hash: String,
    pub recorded_event_hashes: Vec<String>,
    pub changeset_modifications: Vec<FileModificationRecord>,
    pub accumulated_diffs: Vec<String>,
    pub runner_statements: Vec<RunnerExecutionStatementRecord>,
    pub evidence_items: Vec<EvidenceItemRecord>,
    pub history: Vec<ContextMessage>,
}

/// Milliseconds since the epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// The time limit an action carries, or 0 when it states none.
///
/// Part of what an operator is approving: the same command with a different
/// limit is a different action, which is why the action digest covers every
/// argument.
fn timeout_ms_of(args: &serde_json::Value) -> u64 {
    args.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(0)
}

/// A short, human-readable account of what a tool was asked to do.
///
/// The declared `ApprovalRequested` carries no arguments field, so this string
/// is the ONLY place an operator can see what they are approving. It is
/// written for a person, and it is truncated rather than allowed to run to the
/// length of a file's contents.
fn describe_arguments(args: &serde_json::Value) -> String {
    const LIMIT: usize = 120;
    // The argument that says what the action touches, in the order a reader
    // cares about. Falls back to the whole object when a tool uses none of them.
    let summary = ["command", "path", "query", "url_or_path"]
        .iter()
        .find_map(|key| args.get(key).and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| args.to_string());

    if summary.chars().count() > LIMIT {
        let kept: String = summary.chars().take(LIMIT).collect();
        format!("{kept}...")
    } else {
        summary
    }
}

impl OrchestrationEngine {
    pub fn new(
        config: OrchestrationConfig,
        store: Arc<Mutex<EventStore>>,
        runner: Arc<dyn Runner>,
        signing_key: SigningKey,
    ) -> Self {
        Self {
            config,
            store,
            runner,
            tool_registry: ToolRegistry::standard(),
            context_assembler: ContextAssembler::new(),
            signing_key,
            // Not 0. A subscriber resuming "after 0" would never be sent an
            // event numbered 0, which is a session's first. See
            // vitna_protocol::FIRST_EVENT_SEQUENCE. A run that is not a
            // session's first continues from where the last one stopped,
            // through `continuing_at`.
            sequence_counter: vitna_protocol::FIRST_EVENT_SEQUENCE,
            tool_calls: 0,
            approvals: 0,
            last_event_hash: GENESIS_HASH.to_string(),
            recorded_event_hashes: Vec::new(),
            changeset_modifications: Vec::new(),
            accumulated_diffs: Vec::new(),
            runner_statements: Vec::new(),
            evidence_items: Vec::new(),
            history: Vec::new(),
        }
    }

    /// Numbers this run's events from `next_sequence` instead of from the
    /// start, so a session's second run continues its first.
    ///
    /// The protocol numbers events per SESSION: `SubscribeEvents` carries one
    /// `resume_after_sequence` for the whole session, and a client keeps one
    /// cursor. Numbering every run from 1 made a second run's opening events
    /// indistinguishable from replays of the first run's, so they were dropped,
    /// and its next event then landed exactly contiguous, which hid the loss
    /// from both ends.
    ///
    /// The hash chain is untouched: it still starts from `GENESIS_HASH` for
    /// every run, since a receipt attests to one run. Only the numbering spans
    /// the session. A value below `FIRST_EVENT_SEQUENCE` is raised to it, for
    /// the reason that constant gives.
    pub fn continuing_at(mut self, next_sequence: u64) -> Self {
        self.sequence_counter = next_sequence.max(vitna_protocol::FIRST_EVENT_SEQUENCE);
        self
    }

    /// Records an event to the append-only event store with monotonic sequence and hash chaining.
    /// Records that an action needs a decision, as the declaration describes
    /// it, and returns the `approval_id` an answer must name.
    ///
    /// The id and the digest are both recorded because an answer has to match
    /// both: the id says WHICH request, and the digest says WHAT was asked, so
    /// a stale answer cannot land on a different action that happens to reuse
    /// an id.
    ///
    /// Fields left empty are ones this engine does not know. The resolved
    /// executable, the environment names and the bound mounts are the runner's,
    /// and it does not report them here yet; empty is proto3's "unset" and the
    /// honest reading is "not recorded" rather than "none".
    pub fn request_approval(
        &mut self,
        tool_call_id: &str,
        action_digest: &str,
        description: &str,
        timeout_ms: u64,
    ) -> Result<String, String> {
        self.approvals += 1;
        let approval_id = format!("ap-{}-{}", self.config.run_id, self.approvals);

        let canonical_cwd = fs::canonicalize(&self.config.workspace_root)
            .unwrap_or_else(|_| self.config.workspace_root.clone())
            .to_string_lossy()
            .to_string();

        self.record_event(
            vitna_protocol::type_url::event::APPROVAL_REQUESTED,
            &vitna_protocol::messages::ApprovalRequested {
                approval_id: approval_id.clone(),
                tool_call_id: tool_call_id.to_string(),
                action_digest: action_digest.to_string(),
                description: description.to_string(),
                executable_identity: String::new(),
                canonical_cwd,
                environment_names: Vec::new(),
                bound_mounts: vec![self.config.workspace_root.to_string_lossy().to_string()],
                timeout_ms,
            },
        )?;

        Ok(approval_id)
    }

    pub fn record_event<T: Serialize>(
        &mut self,
        type_url: &str,
        payload: &T,
    ) -> Result<String, String> {
        // Deny by default, at the one place every event passes through. An
        // event is either a message `events.proto` declares, or one of this
        // engine's own audit names; there is no third kind. A name that is
        // neither reaches a client as an undecodable frame, and the whole
        // reason the audit names carry their own prefix is that such a name
        // looks like a typo somebody should "fix" onto the declared one.
        //
        // Checked here rather than at each call site, because the call site
        // that skipped the convention is the one that would skip the check.
        if !vitna_protocol::type_url::EVENTS.contains(&type_url)
            && !crate::audit::is_audit(type_url)
        {
            return Err(format!(
                "{type_url} is neither an event events.proto declares nor a \
                 vitna.audit.v1 name; see crates/orchestration/src/audit.rs"
            ));
        }

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let event_id = format!("ev-{}-{}", self.config.run_id, self.sequence_counter);
        let payload_bytes = serde_json::to_vec(payload).map_err(|e| e.to_string())?;

        let event = EventRecord::new(
            &event_id,
            &self.config.run_id,
            self.sequence_counter,
            type_url,
            now_ms,
            payload_bytes,
            &self.last_event_hash,
        );

        let event_hash = event.event_hash.clone();
        {
            let store = self.store.lock().map_err(|e| e.to_string())?;
            store
                .append_event(&event)
                .map_err(|e| format!("Failed to append event: {}", e))?;
        }

        self.last_event_hash = event_hash.clone();
        self.recorded_event_hashes.push(event_hash.clone());
        self.sequence_counter += 1;

        Ok(event_hash)
    }

    /// Executes an end-to-end task step through the vertical slice:
    /// context assembly -> policy check -> tool execution -> evidence capture -> receipt emission.
    pub async fn execute_task_step(
        &mut self,
        step: StepType,
    ) -> Result<ToolResult, String> {
        // Step 1: Assemble context
        let active_diffs = self.accumulated_diffs.join("\n");
        let assembled = self.context_assembler.assemble(
            &self.config.workspace_root,
            &self.history,
            &self.tool_registry.definitions(),
            Some(&active_diffs),
        );

        self.record_event(
            crate::audit::CONTEXT_ASSEMBLED,
            &serde_json::json!({
                "context_digest": assembled.context_digest,
                "estimated_tokens": assembled.estimated_tokens,
            }),
        )?;

        // Step 2: Dispatch step action
        let (tool_name, args, is_mutating) = match &step {
            StepType::InspectWorkspace => (
                "list_dir".to_string(),
                serde_json::json!({ "path": "." }),
                false,
            ),
            StepType::ProposeEdit { path, content } => (
                "write_file".to_string(),
                serde_json::json!({ "path": path, "content": content }),
                true,
            ),
            StepType::RunVerification { command } => (
                "run_command".to_string(),
                serde_json::json!({ "command": command }),
                true,
            ),
        };

        let tool = self
            .tool_registry
            .get(&tool_name)
            .ok_or_else(|| format!("Tool not found: {}", tool_name))?;

        let action_digest = tool.compute_action_digest(&args);

        // One id for this tool call, minted before the approval that gates it,
        // because the declared events link the approval, the start and the
        // finish through it. A client uses that link to decide an approval took
        // effect, so the id has to exist before anything is asked.
        self.tool_calls += 1;
        let tool_call_id = format!("tc-{}-{}", self.config.run_id, self.tool_calls);

        // Step 3: Exact-action capability & approval check
        if is_mutating {
            self.request_approval(
                &tool_call_id,
                &action_digest,
                &format!("{tool_name}: {}", describe_arguments(&args)),
                timeout_ms_of(&args),
            )?;

            if !self.config.auto_approve {
                return Err(format!(
                    "Action requires human approval: {} (digest: {})",
                    tool_name, action_digest
                ));
            }

            self.record_event(
                crate::audit::APPROVAL_GRANTED,
                &serde_json::json!({
                    "tool_name": tool_name,
                    "action_digest": action_digest,
                }),
            )?;
        }

        // Step 3b: Isolation is part of the policy decision, not a detail of
        // execution. A command that cannot be sandboxed needs its own approval,
        // and `auto_approve` does not supply it: the operator approved the work,
        // not the removal of the boundary around it.
        if tool_name == "run_command" && self.runner.spawns_processes() {
            if let Err(reason) = vitna_runner::sandbox_status(&self.config.workspace_root, false) {
                // A SECOND approval, kept apart from the one above on
                // purpose: approving the work and approving that it runs with
                // no boundary around it are different decisions.
                self.request_approval(
                    &tool_call_id,
                    &action_digest,
                    &format!(
                        "{tool_name} with NO OS sandbox: {reason}. Approving this                          removes the boundary, not just the work."
                    ),
                    timeout_ms_of(&args),
                )?;

                if !self.config.allow_unsandboxed {
                    return Err(format!(
                        "No OS sandbox is available for this workspace ({}). Running \
                         '{}' unsandboxed requires explicit approval.",
                        reason, tool_name
                    ));
                }

                self.record_event(
                    crate::audit::APPROVAL_GRANTED,
                    &serde_json::json!({
                        "tool_name": tool_name,
                        "action_digest": action_digest,
                        "reason": "unsandboxed_execution_approved",
                    }),
                )?;
            }
        }

        // Step 4: Execute tool via guarded runner / sandbox
        let mut ctx = ToolContext::new(
            self.config.workspace_root.clone(),
            self.runner.clone(),
        );
        ctx.allow_unsandboxed = self.config.allow_unsandboxed;

        let started_at_ms = now_ms();
        self.record_event(
            vitna_protocol::type_url::event::TOOL_STARTED,
            &vitna_protocol::messages::ToolStarted {
                tool_call_id: tool_call_id.clone(),
                started_at_ms,
            },
        )?;

        let result = tool.execute(args.clone(), &ctx).await?;

        self.record_event(
            vitna_protocol::type_url::event::TOOL_FINISHED,
            &vitna_protocol::messages::ToolFinished {
                tool_call_id: tool_call_id.clone(),
                // A tool that runs no process has no exit code, so it reports
                // success rather than inventing a signal it never saw.
                exit_code: result
                    .exit_code
                    .unwrap_or(if result.success { 0 } else { 1 }),
                // The runner holds the real stream digests and does not report
                // them here yet. Empty means not recorded, never "no output".
                stdout_digest: String::new(),
                stderr_digest: String::new(),
                duration_ms: now_ms().saturating_sub(started_at_ms),
                status: if result.success {
                    vitna_protocol::messages::tool_status::COMPLETED
                } else {
                    vitna_protocol::messages::tool_status::FAILED
                }
                .to_string(),
            },
        )?;

        // Step 5: Update state and track modifications
        if let (Some(pre), Some(post)) = (&result.preimage_hash, &result.postimage_hash) {
            if let Some(path) = args.get("path").and_then(|p| p.as_str()) {
                self.changeset_modifications.push(FileModificationRecord {
                    path: path.to_string(),
                    preimage_hash: pre.clone(),
                    postimage_hash: post.clone(),
                });
            }
        }

        if let Some(diff) = &result.diff {
            self.accumulated_diffs.push(diff.clone());
        }

        if result.exit_code.is_some() {
            let stmt_digest = hex::encode(Sha256::digest(result.output.as_bytes()));
            self.runner_statements.push(RunnerExecutionStatementRecord {
                action_id: format!("act-{}", self.runner_statements.len() + 1),
                statement_digest: stmt_digest,
                signature: hex::encode([0u8; 64]), // Signed statement placeholder
                sandbox_backend: result.sandbox_backend.clone(),
                sandbox_enforcement: result.sandbox_enforcement.clone(),
            });
        }

        // Record turn in history
        self.history.push(ContextMessage {
            role: "assistant".to_string(),
            content: format!("Executed tool: {}", tool_name),
            tool_call_id: Some(action_digest),
            name: Some(tool_name.clone()),
        });

        self.history.push(ContextMessage {
            role: "tool".to_string(),
            content: result.output.clone(),
            tool_call_id: None,
            name: Some(tool_name),
        });

        Ok(result)
    }

    /// The isolation this run actually had, which is not always the one it was
    /// configured with.
    ///
    /// `docs/PLATFORM_MATRIX.md` defines full access as host execution with no
    /// OS sandbox and says it must never be described as sandboxed in a
    /// receipt. So a single action that ran unsandboxed decides the label for
    /// the whole run, however the config was written. Before this, the daemon's
    /// hardcoded "guarded" was copied into every receipt regardless.
    ///
    /// A runner that starts no process reports `no_process` and does not
    /// trigger the downgrade: there was nothing to confine, which is not the
    /// same as something having escaped confinement.
    fn observed_isolation_label(&self) -> String {
        let ran_unsandboxed = self.runner_statements.iter().any(|s| {
            s.sandbox_backend
                .as_deref()
                .map(|b| b == vitna_runner::BACKEND_NONE)
                .unwrap_or(false)
        });

        if ran_unsandboxed {
            return "full_access".to_string();
        }
        self.config.sandbox_guarantee.clone()
    }

    /// Finalizes the run, runs verification command if configured, and signs the cryptographic receipt.
    pub async fn finalize_run(&mut self) -> Result<VitnaRunReceiptV1, String> {
        // Run optional verification check to produce sandbox_captured evidence
        if let Some(cmd) = self.config.verification_command.clone() {
            let verify_res = self
                .execute_task_step(StepType::RunVerification { command: cmd.clone() })
                .await?;

            let evidence_id = format!("ev-proof-{}", self.evidence_items.len() + 1);
            let grade = if verify_res.success {
                "sandbox_captured".to_string()
            } else {
                "model_reported".to_string()
            };

            self.evidence_items.push(EvidenceItemRecord {
                evidence_id,
                grade,
                description: format!("Verification command '{}' completed with success={}", cmd, verify_res.success),
                artifact_digest: Some(hex::encode(Sha256::digest(verify_res.output.as_bytes()))),
            });
        }

        // Calculate Merkle root over event hashes
        let merkle_root = VitnaRunReceiptV1::compute_event_merkle_root(&self.recorded_event_hashes);

        // Calculate combined diff digest
        let all_diffs = self.accumulated_diffs.join("\n");
        let diff_digest = hex::encode(Sha256::digest(all_diffs.as_bytes()));

        let ws_fingerprint = hex::encode(Sha256::digest(
            self.config.workspace_root.to_string_lossy().as_bytes(),
        ));

        let mut receipt = VitnaRunReceiptV1 {
            schema_version: "vitna-run-receipt-v1".to_string(),
            run_id: self.config.run_id.clone(),
            session_id: self.config.session_id.clone(),
            workspace_fingerprint: ws_fingerprint,
            base_commit_sha: "0000000000000000000000000000000000000000".to_string(),
            model_selection: ModelSelectionRecord {
                provider: self.config.provider_name.clone(),
                model_sku: self.config.model_sku.clone(),
                routing_reason: "pinned_profile".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: merkle_root,
            isolation_label: self.observed_isolation_label(),
            completion_state: "completed_with_evidence".to_string(),
            evidence_items: self.evidence_items.clone(),
            changeset: ChangeSetRecord {
                files_modified: self.changeset_modifications.clone(),
                diff_digest,
            },
            runner_execution_statements: self.runner_statements.clone(),
            child_receipt_roots: Vec::new(),
            device_signature: String::new(),
        };

        // Sign the receipt with the device key
        receipt
            .sign(&self.signing_key)
            .map_err(|e| format!("Failed to sign receipt: {}", e))?;

        // Write receipt to disk: .vitna/receipts/{run_id}.json
        let receipt_dir = self.config.workspace_root.join(".vitna").join("receipts");
        fs::create_dir_all(&receipt_dir)
            .map_err(|e| format!("Failed to create receipt directory: {}", e))?;

        let receipt_path = receipt_dir.join(format!("{}.json", self.config.run_id));
        let receipt_json = serde_json::to_string_pretty(&receipt)
            .map_err(|e| format!("Failed to serialize receipt: {}", e))?;

        fs::write(&receipt_path, receipt_json)
            .map_err(|e| format!("Failed to write receipt file: {}", e))?;

        self.record_event(
            crate::audit::RECEIPT_GENERATED,
            &serde_json::json!({
                "run_id": self.config.run_id,
                "receipt_path": receipt_path.to_string_lossy(),
                "device_signature": receipt.device_signature,
            }),
        )?;

        Ok(receipt)
    }
}
