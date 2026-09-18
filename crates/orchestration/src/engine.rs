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

/// The event a run closes with, naming where its receipt was written. The
/// daemon reads it back to place a session that predates `SessionCreated`, so
/// the writer and the reader share this one spelling.
pub const RECEIPT_GENERATED: &str = "vitna.v1.ReceiptGenerated";

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
    pub last_event_hash: String,
    pub recorded_event_hashes: Vec<String>,
    pub changeset_modifications: Vec<FileModificationRecord>,
    pub accumulated_diffs: Vec<String>,
    pub runner_statements: Vec<RunnerExecutionStatementRecord>,
    pub evidence_items: Vec<EvidenceItemRecord>,
    pub history: Vec<ContextMessage>,
    /// What the receipt will say about how the run ended. Defaults to the
    /// completed case and is lowered by whoever cuts the run short, so a
    /// truncated run cannot be signed as a finished one.
    pub completion_state: String,
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
            sequence_counter: 0,
            last_event_hash: GENESIS_HASH.to_string(),
            recorded_event_hashes: Vec::new(),
            changeset_modifications: Vec::new(),
            accumulated_diffs: Vec::new(),
            runner_statements: Vec::new(),
            evidence_items: Vec::new(),
            history: Vec::new(),
            completion_state: "completed_with_evidence".to_string(),
        }
    }

    /// Records an event to the append-only event store with monotonic sequence and hash chaining.
    pub fn record_event(
        &mut self,
        type_url: &str,
        payload: &serde_json::Value,
    ) -> Result<String, String> {
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
            "vitna.v1.ContextAssembled",
            &serde_json::json!({
                "context_digest": assembled.context_digest,
                "estimated_tokens": assembled.estimated_tokens,
            }),
        )?;

        // Step 2: Dispatch step action
        let (tool_name, args) = match &step {
            StepType::InspectWorkspace => {
                ("list_dir".to_string(), serde_json::json!({ "path": "." }))
            }
            StepType::ProposeEdit { path, content } => (
                "write_file".to_string(),
                serde_json::json!({ "path": path, "content": content }),
            ),
            StepType::RunVerification { command } => (
                "run_command".to_string(),
                serde_json::json!({ "command": command }),
            ),
        };

        self.execute_tool(&tool_name, args).await
    }

    /// The guarded path a tool call takes, whoever chose it.
    ///
    /// `execute_task_step` picks its tool from a fixed `StepType`; the agent
    /// loop picks it from what a model asked for. Both arrive here, so the
    /// approval gate, the hash-chained events and the evidence capture are one
    /// implementation rather than two. That matters more than the duplication
    /// would: the guarantee a receipt claims IS this path, and a second copy of
    /// it is a second thing that can quietly stop matching the first.
    pub async fn execute_tool(
        &mut self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<ToolResult, String> {
        let tool_name = tool_name.to_string();
        let tool = self
            .tool_registry
            .get(&tool_name)
            .ok_or_else(|| format!("Tool not found: {}", tool_name))?;

        // Taken from the definition rather than from the caller, so the flag the
        // gate enforces is the same one the model was shown.
        let is_mutating = tool.definition().is_mutating;
        let action_digest = tool.compute_action_digest(&args);

        // Step 3: Exact-action capability & approval check
        if is_mutating {
            self.record_event(
                "vitna.v1.ApprovalRequested",
                &serde_json::json!({
                    "tool_name": tool_name,
                    "action_digest": action_digest,
                    "arguments": args,
                }),
            )?;

            if !self.config.auto_approve {
                return Err(format!(
                    "Action requires human approval: {} (digest: {})",
                    tool_name, action_digest
                ));
            }

            self.record_event(
                "vitna.v1.ApprovalGranted",
                &serde_json::json!({
                    "tool_name": tool_name,
                    "action_digest": action_digest,
                }),
            )?;
        }

        // Step 4: Execute tool via guarded runner / sandbox
        let ctx = ToolContext {
            workspace_root: self.config.workspace_root.clone(),
            runner: self.runner.clone(),
        };

        self.record_event(
            "vitna.v1.ToolStarted",
            &serde_json::json!({
                "tool_name": tool_name,
                "action_digest": action_digest,
            }),
        )?;

        let result = tool.execute(args.clone(), &ctx).await?;

        self.record_event(
            "vitna.v1.ToolFinished",
            &serde_json::json!({
                "tool_name": tool_name,
                "success": result.success,
                "preimage_hash": result.preimage_hash,
                "postimage_hash": result.postimage_hash,
            }),
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
            base_commit_sha: base_commit_sha(&self.config.workspace_root),
            model_selection: ModelSelectionRecord {
                provider: self.config.provider_name.clone(),
                model_sku: self.config.model_sku.clone(),
                routing_reason: "pinned_profile".to_string(),
                policy_digest: None,
            },
            event_hash_chain_root: merkle_root,
            isolation_label: self.config.sandbox_guarantee.clone(),
            completion_state: self.completion_state.clone(),
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
            RECEIPT_GENERATED,
            &serde_json::json!({
                "run_id": self.config.run_id,
                "receipt_path": receipt_path.to_string_lossy(),
                "device_signature": receipt.device_signature,
            }),
        )?;

        Ok(receipt)
    }
}

/// The commit the run started from, or git's own null sha when there is no
/// commit to name.
///
/// This was the all-zero literal, unconditionally, which reads as "no base
/// commit" and was being written over real repositories. A receipt whose
/// changeset cannot be located against a commit is a receipt nobody can check
/// a diff against.
///
/// Through `host_git`, like every git the host runs: `rev-parse` reads no
/// index and runs no hook, but "nothing builds git directly" is a rule with no
/// exceptions for a reader to weigh. When the hardening itself cannot be set
/// up, git is not run, and the base is as unnamed as on a box with no git.
fn base_commit_sha(workspace_root: &std::path::Path) -> String {
    const NONE: &str = "0000000000000000000000000000000000000000";
    let out = match vitna_git_workspaces::host_git::command(workspace_root) {
        Ok(mut command) => command.args(["rev-parse", "HEAD"]).output(),
        Err(reason) => {
            tracing::warn!(%reason, "base commit not read: host git could not be hardened");
            return NONE.to_string();
        }
    };

    match out {
        Ok(o) if o.status.success() => {
            let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if sha.len() == 40 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                sha
            } else {
                NONE.to_string()
            }
        }
        // Not a repository, no commits yet, or no git on the box. All three
        // genuinely have no base commit to name.
        _ => NONE.to_string(),
    }
}
