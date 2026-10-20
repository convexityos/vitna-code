//! The loop that asks a model what to do and then does it.
//!
//! Before this, no model was ever asked anything on `main`: `vitna run`
//! looked for "create" or "edit" in the prompt and wrote a fixed file, and the
//! providers crate carried working adapters whose `.complete()` had no caller.
//! The loop was written on `native-desktop-app` and lands here rewired to the
//! declared protocol (ADR-0007): the model picks the tools, and every call it
//! picks goes through [`OrchestrationEngine::execute_tool`], so the approval
//! gate, the hash-chained events and the evidence capture are the same ones a
//! scripted step meets.

use std::collections::BTreeSet;

use vitna_protocol::messages::{run_state, Diagnostic, MessageDelta, UsageUpdated};
use vitna_protocol::type_url;
use vitna_providers::{Provider, ProviderMessage, ProviderRequest};
use vitna_receipts::EvidenceItemRecord;

use crate::engine::OrchestrationEngine;

/// How many times one turn may ask the model before it stops.
///
/// A turn that reaches it is not a finished one: the receipt is signed
/// `failed` with evidence naming the limit, never as completed.
pub const MAX_ROUNDS: usize = 24;

/// What a finished turn is worth reporting.
#[derive(Debug, Clone, Default)]
pub struct AgentTurn {
    /// The model's last message.
    pub text: String,
    /// None when no response reported usage. Zero is a different claim.
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// How many times the model was asked.
    pub rounds: usize,
    /// Every tool that ran, in order, whatever its result.
    pub tools_run: Vec<String>,
    /// True when the loop stopped at its round limit with the model still
    /// working.
    pub hit_round_limit: bool,
    /// Every model a response said served it, in order of first appearance.
    pub served_models: Vec<String>,
}

/// Deliberately short: the tool schemas carry the detail, and a long prompt
/// asserting capabilities the tools do not have is how a model ends up
/// describing work it never did.
fn system_prompt(workspace_root: &std::path::Path) -> String {
    format!(
        "You are Vitna Code, a coding agent working in a local checkout at {}.\n\
         Use the provided tools to inspect and change the workspace. Paths are \
         relative to the workspace root. Every change and every command waits \
         for the operator's approval, and a refusal comes back as the tool's \
         result.\n\
         Prefer reading before writing. When you have finished, reply with a \
         short summary of what you changed and how it was verified.\n\
         State plainly what you did not do or could not check. Never claim a \
         command passed unless a tool result shows it.",
        workspace_root.display()
    )
}

impl OrchestrationEngine {
    /// Runs one turn to completion: ask, execute what is asked for, feed the
    /// results back, repeat until the model stops or `max_rounds` is reached.
    ///
    /// `prior` is the session's conversation so far, which the model sees
    /// before this turn's prompt. A tool that fails, one the operator refuses,
    /// and one the model invents all come back to the model as a result it can
    /// read, because recovering from them is ordinary work. A provider that
    /// fails ends the turn: the receipt is still signed, `failed`, since the
    /// rounds before it may already have changed the workspace.
    pub async fn run_agent_turn(
        &mut self,
        provider: &dyn Provider,
        prior: &[ProviderMessage],
        prompt: &str,
        max_rounds: usize,
    ) -> Result<AgentTurn, String> {
        let tools: Vec<serde_json::Value> = self
            .tool_registry
            .definitions()
            .iter()
            .map(|d| serde_json::to_value(d).unwrap_or(serde_json::Value::Null))
            .collect();

        let mut messages: Vec<ProviderMessage> = prior.to_vec();
        messages.push(ProviderMessage::user(prompt));
        let mut turn = AgentTurn::default();
        let mut served: Vec<String> = Vec::new();
        let (mut prompt_tokens, mut completion_tokens, mut saw_usage) = (0u64, 0u64, false);

        self.record_event(
            type_url::event::MESSAGE_DELTA,
            &MessageDelta {
                message_id: format!("{}-user", self.config.run_id),
                role: "user".to_string(),
                delta_text: prompt.to_string(),
            },
        )?;

        for round in 0..max_rounds {
            turn.rounds = round + 1;
            self.set_run_state(run_state::RUNNING_MODEL, "asking the model")?;

            let request = ProviderRequest {
                model: self.config.model_sku.clone(),
                system_prompt: Some(system_prompt(&self.config.workspace_root)),
                messages: messages.clone(),
                tools: tools.clone(),
                temperature: None,
                max_tokens: Some(8192),
            };

            self.record_event(
                crate::audit::MODEL_REQUESTED,
                &serde_json::json!({
                    "provider": provider.name(),
                    "model_sku": self.config.model_sku,
                    "round": turn.rounds,
                    "message_count": messages.len(),
                    "tool_count": tools.len(),
                }),
            )?;

            let response = match provider.complete(&request).await {
                Ok(response) => response,
                Err(e) => {
                    // The reason reaches the operator intact: a turn that
                    // failed because the runtime is down must not read like a
                    // model that declined.
                    let reason = format!("{} request failed: {e}", provider.name());
                    self.record_event(
                        type_url::event::DIAGNOSTIC,
                        &Diagnostic {
                            level: "error".to_string(),
                            code: "provider_failed".to_string(),
                            message: reason.clone(),
                            details: String::new(),
                        },
                    )?;
                    self.completion_state = "failed".to_string();
                    self.settle_model(&served);
                    turn.served_models = served;
                    return Err(reason);
                }
            };

            if let Some(model) = &response.model {
                if !served.contains(model) {
                    served.push(model.clone());
                }
            }

            self.record_event(
                crate::audit::MODEL_RESPONDED,
                &serde_json::json!({
                    "provider": provider.name(),
                    "model": response.model,
                    "finish_reason": response.finish_reason,
                    "tool_calls": response.tool_calls.len(),
                    "prompt_tokens": response.prompt_tokens,
                    "completion_tokens": response.completion_tokens,
                }),
            )?;

            if response.prompt_tokens > 0 || response.completion_tokens > 0 {
                saw_usage = true;
                prompt_tokens += u64::from(response.prompt_tokens);
                completion_tokens += u64::from(response.completion_tokens);
                self.record_event(
                    type_url::event::USAGE_UPDATED,
                    &UsageUpdated {
                        run_id: self.config.run_id.clone(),
                        provider: provider.name().to_string(),
                        model_sku: response
                            .model
                            .clone()
                            .unwrap_or_else(|| self.config.model_sku.clone()),
                        prompt_tokens,
                        completion_tokens,
                        cached_tokens: 0,
                        // Nothing here prices a call. The runtime's ledger
                        // does, and a zero beside `false` claims nothing.
                        cost_known: false,
                        cost_usd: 0.0,
                    },
                )?;
            }

            if !response.content.is_empty() {
                turn.text = response.content.clone();
                self.record_event(
                    type_url::event::MESSAGE_DELTA,
                    &MessageDelta {
                        message_id: format!("{}-m{}", self.config.run_id, turn.rounds),
                        role: "model".to_string(),
                        delta_text: response.content.clone(),
                    },
                )?;
            }

            if response.tool_calls.is_empty() {
                turn.prompt_tokens = saw_usage.then_some(prompt_tokens);
                turn.completion_tokens = saw_usage.then_some(completion_tokens);
                self.settle_model(&served);
                turn.served_models = served;
                return Ok(turn);
            }

            messages.push(ProviderMessage::assistant(
                response.content.clone(),
                response.tool_calls.clone(),
            ));

            for call in &response.tool_calls {
                if self.tool_registry.get(&call.name).is_none() {
                    // Said out loud, since an invented tool never reaches
                    // `ToolProposed` and would otherwise leave no trace a
                    // client can see.
                    self.record_event(
                        type_url::event::DIAGNOSTIC,
                        &Diagnostic {
                            level: "warn".to_string(),
                            code: "unknown_tool".to_string(),
                            message: format!(
                                "the model asked for {}, which is not a tool here",
                                call.name
                            ),
                            details: call.id.clone(),
                        },
                    )?;
                }

                let outcome = self
                    .execute_tool(&call.name, call.arguments.clone(), Some(&call.id))
                    .await;

                let content = match &outcome {
                    Ok(result) if result.success => {
                        turn.tools_run.push(call.name.clone());
                        result.output.clone()
                    }
                    Ok(result) => {
                        turn.tools_run.push(call.name.clone());
                        format!("{} failed: {}", call.name, result.output)
                    }
                    Err(e) => format!("{} did not run: {}", call.name, e),
                };

                messages.push(ProviderMessage::tool_result(&call.id, content));
            }
        }

        // A run that stopped because it ran out of rounds is not a completed
        // one, and the receipt has to say so before it is signed.
        turn.hit_round_limit = true;
        self.completion_state = "failed".to_string();
        self.evidence_items.push(EvidenceItemRecord {
            evidence_id: format!("ev-round-limit-{}", self.evidence_items.len() + 1),
            grade: "broker_observed".to_string(),
            description: format!(
                "Stopped at the round limit of {max_rounds} with the model still asking for tools"
            ),
            artifact_digest: None,
        });
        turn.prompt_tokens = saw_usage.then_some(prompt_tokens);
        turn.completion_tokens = saw_usage.then_some(completion_tokens);
        self.settle_model(&served);
        turn.served_models = served;
        Ok(turn)
    }

    /// What the receipt names as the model, from what the responses said.
    ///
    /// A lane alias is what was asked for, not what served, so when every
    /// response named the same model the receipt names that one. When they
    /// named several (a lane that fell back mid-turn) it names all of them,
    /// because one of them would be a silent model switch, and when none was
    /// named it keeps the alias and says the served model was not reported.
    fn settle_model(&mut self, served: &[String]) {
        let distinct: BTreeSet<&String> = served.iter().collect();
        match distinct.len() {
            0 => {
                self.routing_reason = format!("{}; served model not reported", self.routing_reason)
            }
            1 => self.config.model_sku = served[0].clone(),
            _ => {
                self.config.model_sku = served.join(",");
                self.routing_reason = format!(
                    "{}; more than one model served the turn",
                    self.routing_reason
                );
            }
        }
    }
}
