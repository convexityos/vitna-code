use crate::credentials::ProviderCredentials;
use crate::{Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall};
use async_trait::async_trait;
use serde_json::json;

pub struct OpenAIProvider {
    pub credentials: ProviderCredentials,
}

impl OpenAIProvider {
    pub fn new(credentials: ProviderCredentials) -> Self {
        Self { credentials }
    }

    pub fn default_endpoint(&self) -> String {
        self.credentials
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1/chat/completions")
            .to_string()
    }

    /// Formats request payload adhering to OpenAI Chat Completions API schema.
    pub fn format_request_payload(&self, req: &ProviderRequest) -> serde_json::Value {
        let mut messages = Vec::new();

        if let Some(sys) = &req.system_prompt {
            messages.push(json!({
                "role": "system",
                "content": sys
            }));
        }

        for msg in &req.messages {
            messages.push(json!({
                "role": msg.role,
                "content": msg.content
            }));
        }

        let mut payload = json!({
            "model": req.model,
            "messages": messages,
            "stream": true
        });

        if let Some(mt) = req.max_tokens {
            payload["max_tokens"] = json!(mt);
        }

        if !req.tools.is_empty() {
            let mut openai_tools = Vec::new();
            for tool in &req.tools {
                openai_tools.push(json!({
                    "type": "function",
                    "function": {
                        "name": tool.get("name").unwrap_or(&json!("")),
                        "description": tool.get("description").unwrap_or(&json!("")),
                        "parameters": tool.get("parameters_schema").unwrap_or(&json!({}))
                    }
                }));
            }
            payload["tools"] = json!(openai_tools);
        }

        payload
    }

    /// Parses an incoming SSE event line from OpenAI stream.
    pub fn parse_sse_event_line(
        &self,
        data: &str,
    ) -> Result<Vec<ProviderStreamEvent>, String> {
        let mut events = Vec::new();
        let trimmed = data.trim();

        if trimmed == "[DONE]" {
            events.push(ProviderStreamEvent::Finish("stop".to_string()));
            return Ok(events);
        }

        let val: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => return Ok(events),
        };

        if let Some(choices) = val.get("choices").and_then(|c| c.as_array()) {
            for choice in choices {
                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                        events.push(ProviderStreamEvent::TextDelta(content.to_string()));
                    }

                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
                        for tc in tool_calls {
                            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            let id = tc.get("id").and_then(|i| i.as_str()).map(|s| s.to_string());
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                .map(|s| s.to_string());
                            let args_delta = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                                .unwrap_or_default()
                                .to_string();

                            events.push(ProviderStreamEvent::ToolCallDelta {
                                index,
                                id,
                                name,
                                arguments_delta: args_delta,
                            });
                        }
                    }
                }

                if let Some(finish_reason) = choice.get("finish_reason").and_then(|f| f.as_str()) {
                    events.push(ProviderStreamEvent::Finish(finish_reason.to_string()));
                }
            }
        }

        if let Some(usage) = val.get("usage") {
            let prompt_tokens = usage.get("prompt_tokens").and_then(|t| t.as_u64()).unwrap_or(0) as u32;
            let completion_tokens = usage.get("completion_tokens").and_then(|t| t.as_u64()).unwrap_or(0) as u32;
            events.push(ProviderStreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
            });
        }

        Ok(events)
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn parse_sse_event(&self, _event_type: &str, data: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        self.parse_sse_event_line(data)
    }

    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String> {
        let mut payload = self.format_request_payload(req);
        payload["stream"] = json!(false); // Synchronous completion request
        let endpoint = self.default_endpoint();

        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.credentials.api_key))
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("OpenAI request network error: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("OpenAI API error (status {}): {}", status, body));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse OpenAI response JSON: {}", e))?;

        let mut content = String::new();
        let mut tool_calls = Vec::new();
        let mut finish_reason = "stop".to_string();

        if let Some(choices) = body.get("choices").and_then(|c| c.as_array()) {
            if let Some(first) = choices.first() {
                if let Some(msg) = first.get("message") {
                    if let Some(c) = msg.get("content").and_then(|s| s.as_str()) {
                        content.push_str(c);
                    }

                    if let Some(tcs) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let args_str = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                                .unwrap_or("{}");
                            let args_val: serde_json::Value = serde_json::from_str(args_str).unwrap_or(json!({}));

                            tool_calls.push(ProviderToolCall {
                                id,
                                name,
                                arguments: args_val,
                            });
                        }
                    }
                }

                if let Some(fr) = first.get("finish_reason").and_then(|f| f.as_str()) {
                    finish_reason = fr.to_string();
                }
            }
        }

        let prompt_tokens = body
            .get("usage")
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0) as u32;

        let completion_tokens = body
            .get("usage")
            .and_then(|u| u.get("completion_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0) as u32;

        Ok(ProviderResponse {
            content,
            tool_calls,
            prompt_tokens,
            completion_tokens,
            finish_reason,
        })
    }
}
