//! The loop that asks a model what to do and then does it.
//!
//! This is the piece that did not exist. `vitna-providers` carried working
//! Anthropic and OpenAI adapters with nothing calling them: `.complete()` had
//! no caller anywhere in the workspace, and the engine's `provider_name` was a
//! string it copied into the receipt, defaulted to `"fake"`. So a run would
//! produce a signed receipt attesting to a model that had never been asked
//! anything, which is the one claim a receipt exists to make.
//!
//! What ran before was keyword matching: the daemon looked for "create" or
//! "edit" in the prompt and wrote a fixed file. That is gone. The model picks
//! the tools now, and every call it picks still goes through
//! [`OrchestrationEngine::execute_tool`], so the approval gate, the event chain
//! and the evidence capture are the same ones a scripted step would meet.

use vitna_providers::{Provider, ProviderMessage, ProviderRequest};

use crate::engine::OrchestrationEngine;

/// What a finished turn is worth reporting.
#[derive(Debug, Clone, Default)]
pub struct AgentTurn {
    /// The assistant's closing message.
    pub text: String,
    /// None when the provider reported no usage. Zero is a different claim.
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    /// How many times the model was asked. One means it answered without
    /// reaching for a tool.
    pub rounds: usize,
    /// Every tool the model actually ran, in order.
    pub tools_run: Vec<String>,
    /// True when the loop stopped at `max_rounds` with the model still
    /// working. The caller has to say so rather than present a cut-off run as
    /// a finished one.
    pub hit_round_limit: bool,
}

/// The system prompt. Deliberately short: the tool schemas carry the detail,
/// and a long prompt asserting capabilities the tools do not have is how a
/// model ends up describing work it never did.
fn system_prompt(workspace_root: &std::path::Path) -> String {
    format!(
        "You are Vitna Code, a coding agent working in a local checkout at {}.\n\
         Use the provided tools to inspect and change the workspace. Paths are \
         relative to the workspace root.\n\
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
    /// A tool that fails is reported to the model as a failed result rather
    /// than aborting the turn, because recovering from its own bad argument is
    /// ordinary work. A tool the model invents, or one the approval gate
    /// refuses, is reported the same way.
    pub async fn run_agent_turn(
        &mut self,
        provider: &dyn Provider,
        prompt: &str,
        max_rounds: usize,
    ) -> Result<AgentTurn, String> {
        let tools: Vec<serde_json::Value> = self
            .tool_registry
            .definitions()
            .iter()
            .map(|d| serde_json::to_value(d).unwrap_or(serde_json::Value::Null))
            .collect();

        let mut messages: Vec<ProviderMessage> = vec![ProviderMessage::user(prompt)];
        let mut turn = AgentTurn::default();
        let mut prompt_tokens: u64 = 0;
        let mut completion_tokens: u64 = 0;
        let mut saw_usage = false;

        for round in 0..max_rounds {
            turn.rounds = round + 1;

            let request = ProviderRequest {
                model: self.config.model_sku.clone(),
                system_prompt: Some(system_prompt(&self.config.workspace_root)),
                messages: messages.clone(),
                tools: tools.clone(),
                temperature: None,
                max_tokens: Some(8192),
            };

            self.record_event(
                "vitna.v1.ModelRequested",
                &serde_json::json!({
                    "provider": provider.name(),
                    "model_sku": self.config.model_sku,
                    "round": turn.rounds,
                    "message_count": messages.len(),
                    "tool_count": tools.len(),
                }),
            )?;

            let response = provider.complete(&request).await.map_err(|e| {
                // The reason reaches the caller intact; a turn that failed
                // because a key is wrong must not read like a model refusal.
                format!("{} request failed: {}", provider.name(), e)
            })?;

            if response.prompt_tokens > 0 || response.completion_tokens > 0 {
                saw_usage = true;
                prompt_tokens += response.prompt_tokens as u64;
                completion_tokens += response.completion_tokens as u64;
            }

            self.record_event(
                "vitna.v1.ModelResponded",
                &serde_json::json!({
                    "provider": provider.name(),
                    "finish_reason": response.finish_reason,
                    "tool_calls": response.tool_calls.len(),
                    "prompt_tokens": response.prompt_tokens,
                    "completion_tokens": response.completion_tokens,
                }),
            )?;

            if !response.content.is_empty() {
                turn.text = response.content.clone();
            }

            if response.tool_calls.is_empty() {
                turn.prompt_tokens = saw_usage.then_some(prompt_tokens.min(u32::MAX as u64) as u32);
                turn.completion_tokens =
                    saw_usage.then_some(completion_tokens.min(u32::MAX as u64) as u32);
                return Ok(turn);
            }

            messages.push(ProviderMessage::assistant(
                response.content.clone(),
                response.tool_calls.clone(),
            ));

            for call in &response.tool_calls {
                // Every call, including one naming a tool that does not exist,
                // goes through the same guarded path and comes back as a
                // result the model can read.
                let outcome = self.execute_tool(&call.name, call.arguments.clone()).await;

                let content = match &outcome {
                    Ok(result) if result.success => {
                        turn.tools_run.push(call.name.clone());
                        result.output.clone()
                    }
                    Ok(result) => {
                        turn.tools_run.push(call.name.clone());
                        format!("{} failed: {}", call.name, result.output)
                    }
                    Err(e) => format!("{} could not run: {}", call.name, e),
                };

                messages.push(ProviderMessage::tool_result(&call.id, content));
            }
        }

        // A run that stopped because it ran out of rounds is not a completed
        // one, and the receipt has to say so before it is signed.
        turn.hit_round_limit = true;
        self.completion_state = vitna_receipts::STOPPED_AT_ROUND_LIMIT.to_string();
        turn.prompt_tokens = saw_usage.then_some(prompt_tokens.min(u32::MAX as u64) as u32);
        turn.completion_tokens = saw_usage.then_some(completion_tokens.min(u32::MAX as u64) as u32);
        Ok(turn)
    }
}
