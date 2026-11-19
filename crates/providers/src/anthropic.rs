use crate::credentials::ProviderCredentials;
use crate::{Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use vitna_telemetry::TelemetryCollector;

pub struct AnthropicProvider {
    pub credentials: ProviderCredentials,
    telemetry: Option<Arc<TelemetryCollector>>,
}

impl AnthropicProvider {
    pub fn new(credentials: ProviderCredentials) -> Self {
        Self {
            credentials,
            telemetry: None,
        }
    }

    pub fn with_telemetry(mut self, collector: Arc<TelemetryCollector>) -> Self {
        self.telemetry = Some(collector);
        self
    }

    pub fn default_endpoint(&self) -> String {
        self.credentials
            .base_url
            .as_deref()
            .unwrap_or("https://api.anthropic.com/v1/messages")
            .to_string()
    }

    /// Formats request payload adhering to Anthropic Messages API schema.
    pub fn format_request_payload(&self, req: &ProviderRequest) -> serde_json::Value {
        let mut messages: Vec<serde_json::Value> = Vec::new();
        let mut pending_results: Vec<serde_json::Value> = Vec::new();

        // Tool results are `user` turns here, and the API requires strictly
        // alternating roles, so every result answering one assistant turn has
        // to arrive as blocks of ONE user message. Flushing on the next
        // non-result message is what keeps two parallel tool calls from
        // becoming two consecutive user turns, which the API rejects.
        for msg in &req.messages {
            if let Some(call_id) = &msg.tool_call_id {
                pending_results.push(json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": msg.content
                }));
                continue;
            }

            if !pending_results.is_empty() {
                messages.push(json!({
                    "role": "user",
                    "content": std::mem::take(&mut pending_results)
                }));
            }

            if msg.tool_calls.is_empty() {
                messages.push(json!({
                    "role": msg.role,
                    "content": msg.content
                }));
            } else {
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if !msg.content.is_empty() {
                    blocks.push(json!({ "type": "text", "text": msg.content }));
                }
                for call in &msg.tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments
                    }));
                }
                messages.push(json!({ "role": msg.role, "content": blocks }));
            }
        }

        if !pending_results.is_empty() {
            messages.push(json!({ "role": "user", "content": pending_results }));
        }

        let mut payload = json!({
            "model": req.model,
            "messages": messages,
            "max_tokens": req.max_tokens.unwrap_or(4096),
            "stream": true
        });

        if let Some(sys) = &req.system_prompt {
            payload["system"] = json!(sys);
        }

        if !req.tools.is_empty() {
            let mut anthropic_tools = Vec::new();
            for tool in &req.tools {
                anthropic_tools.push(json!({
                    "name": tool.get("name").unwrap_or(&json!("")),
                    "description": tool.get("description").unwrap_or(&json!("")),
                    "input_schema": tool.get("parameters_schema").unwrap_or(&json!({}))
                }));
            }
            payload["tools"] = json!(anthropic_tools);
        }

        payload
    }

    /// Parses an incoming SSE event line from Anthropic stream.
    pub fn parse_sse_event_line(
        &self,
        event_type: &str,
        data: &str,
    ) -> Result<Vec<ProviderStreamEvent>, String> {
        let mut events = Vec::new();
        let val: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Ok(events),
        };

        match event_type {
            "message_start" => {
                if let Some(usage) = val.get("message").and_then(|m| m.get("usage")) {
                    let prompt = usage
                        .get("input_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0) as u32;
                    let comp = usage
                        .get("output_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0) as u32;
                    if prompt > 0 || comp > 0 {
                        events.push(ProviderStreamEvent::Usage {
                            prompt_tokens: prompt,
                            completion_tokens: comp,
                        });
                    }
                }
            }
            "content_block_start" => {
                if let Some(cb) = val.get("content_block") {
                    if cb.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                        let id = cb.get("id").and_then(|i| i.as_str()).map(|s| s.to_string());
                        let name = cb.get("name").and_then(|n| n.as_str()).map(|s| s.to_string());
                        let index = val.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

                        events.push(ProviderStreamEvent::ToolCallDelta {
                            index,
                            id,
                            name,
                            arguments_delta: String::new(),
                        });
                    }
                }
            }
            "content_block_delta" => {
                if let Some(delta) = val.get("delta") {
                    let delta_type = delta.get("type").and_then(|t| t.as_str());
                    let index = val.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;

                    if delta_type == Some("text_delta") {
                        if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                            events.push(ProviderStreamEvent::TextDelta(text.to_string()));
                        }
                    } else if delta_type == Some("input_json_delta") {
                        if let Some(partial) = delta.get("partial_json").and_then(|p| p.as_str()) {
                            events.push(ProviderStreamEvent::ToolCallDelta {
                                index,
                                id: None,
                                name: None,
                                arguments_delta: partial.to_string(),
                            });
                        }
                    }
                }
            }
            "message_delta" => {
                if let Some(usage) = val.get("usage") {
                    let comp = usage.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0) as u32;
                    events.push(ProviderStreamEvent::Usage {
                        prompt_tokens: 0,
                        completion_tokens: comp,
                    });
                }
                if let Some(delta) = val.get("delta") {
                    if let Some(stop) = delta.get("stop_reason").and_then(|s| s.as_str()) {
                        events.push(ProviderStreamEvent::Finish(stop.to_string()));
                    }
                }
            }
            "message_stop" => {
                events.push(ProviderStreamEvent::Finish("end_turn".to_string()));
            }
            _ => {}
        }

        Ok(events)
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn parse_sse_event(&self, event_type: &str, data: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        self.parse_sse_event_line(event_type, data)
    }

    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String> {
        let mut payload = self.format_request_payload(req);
        // format_request_payload asks for SSE, which is right for the streaming
        // path and fatal here: this function parses one JSON body, and an
        // event-stream response fails at resp.json(). Nothing had ever called
        // this path, which is how it went unnoticed.
        payload["stream"] = json!(false);
        let endpoint = self.default_endpoint();
        let req_bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to serialize Anthropic request JSON: {}", e))?;
        let bytes_sent = req_bytes.len() as u64;
        let host = reqwest::Url::parse(&endpoint)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        let provider_label = if self.credentials.provider.is_empty() {
            "anthropic".to_string()
        } else {
            self.credentials.provider.clone()
        };

        let started = Instant::now();
        let client = reqwest::Client::new();
        let resp = client
            .post(&endpoint)
            .header("x-api-key", &self.credentials.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .body(req_bytes)
            .send()
            .await
            .map_err(|e| format!("Anthropic request network error: {}", e))?;

        let status = resp.status();
        let resp_bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read Anthropic response body: {}", e))?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

        crate::sse::record_http_telemetry(
            self.telemetry.as_ref(),
            provider_label,
            host,
            endpoint,
            bytes_sent,
            resp_bytes.len() as u64,
            status,
            elapsed_ms,
        );

        if !status.is_success() {
            let body = String::from_utf8_lossy(&resp_bytes);
            return Err(format!("Anthropic API error (status {}): {}", status, body));
        }

        let body: serde_json::Value = serde_json::from_slice(&resp_bytes)
            .map_err(|e| format!("Failed to parse Anthropic response JSON: {}", e))?;

        Ok(parse_completion(&body))
    }

    async fn complete_streaming(
        &self,
        req: &ProviderRequest,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) -> Result<ProviderResponse, String> {
        let mut payload = self.format_request_payload(req);
        payload["stream"] = json!(true);
        let endpoint = self.default_endpoint();
        let req_bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to serialize Anthropic request JSON: {}", e))?;
        let bytes_sent = req_bytes.len() as u64;
        let host = reqwest::Url::parse(&endpoint)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".to_string());
        let provider_label = if self.credentials.provider.is_empty() {
            "anthropic".to_string()
        } else {
            self.credentials.provider.clone()
        };

        let started = Instant::now();
        let client = reqwest::Client::new();
        let mut resp = client
            .post(&endpoint)
            .header("x-api-key", &self.credentials.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("anthropic-beta", "prompt-caching-2024-07-31")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream, application/json")
            .body(req_bytes)
            .send()
            .await
            .map_err(|e| format!("Anthropic request network error: {}", e))?;

        let status = resp.status();
        let streaming = status.is_success() && crate::sse::is_event_stream(resp.headers());

        if !status.is_success() {
            let resp_bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Anthropic error body: {}", e))?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            crate::sse::record_http_telemetry(
                self.telemetry.as_ref(),
                provider_label,
                host,
                endpoint,
                bytes_sent,
                resp_bytes.len() as u64,
                status,
                elapsed_ms,
            );
            let body = String::from_utf8_lossy(&resp_bytes);
            return Err(format!("Anthropic API error (status {}): {}", status, body));
        }

        if !streaming {
            let resp_bytes = resp
                .bytes()
                .await
                .map_err(|e| format!("Failed to read Anthropic response body: {}", e))?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            crate::sse::record_http_telemetry(
                self.telemetry.as_ref(),
                provider_label,
                host,
                endpoint,
                bytes_sent,
                resp_bytes.len() as u64,
                status,
                elapsed_ms,
            );
            let body: serde_json::Value = serde_json::from_slice(&resp_bytes)
                .map_err(|e| format!("Failed to parse Anthropic response JSON: {}", e))?;
            let parsed = parse_completion(&body);
            if !parsed.content.is_empty() {
                on_delta(parsed.content.clone());
            }
            return Ok(parsed);
        }

        let mut parser = crate::sse::SseParser::new();
        let mut accumulator = crate::sse::StreamAccumulator::new("end_turn");
        let mut bytes_received = 0u64;

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("Failed to read Anthropic SSE chunk: {}", e))?
        {
            bytes_received += chunk.len() as u64;
            for (event_type, data) in parser.push(&chunk) {
                accumulator.observe_raw_json(&data);
                let events = self.parse_sse_event(&event_type, &data)?;
                accumulator.apply_events(events, on_delta);
            }
        }
        if let Some((event_type, data)) = parser.finish() {
            accumulator.observe_raw_json(&data);
            let events = self.parse_sse_event(&event_type, &data)?;
            accumulator.apply_events(events, on_delta);
        }

        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        crate::sse::record_http_telemetry(
            self.telemetry.as_ref(),
            provider_label,
            host,
            endpoint,
            bytes_sent,
            bytes_received,
            status,
            elapsed_ms,
        );

        Ok(accumulator.into_response())
    }
}

/// An Anthropic Messages response body parsed into the normalized shape.
pub fn parse_completion(body: &serde_json::Value) -> ProviderResponse {
    let mut content = String::new();
    let mut tool_calls = Vec::new();

    if let Some(blocks) = body.get("content").and_then(|c| c.as_array()) {
        for b in blocks {
            let block_type = b.get("type").and_then(|t| t.as_str());
            if block_type == Some("text") {
                if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                    content.push_str(t);
                }
            } else if block_type == Some("tool_use") {
                let id = b
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or_default()
                    .to_string();
                let name = b
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                let input = b.get("input").cloned().unwrap_or(json!({}));
                tool_calls.push(ProviderToolCall {
                    id,
                    name,
                    arguments: input,
                });
            }
        }
    }

    let prompt_tokens = body
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0) as u32;

    let completion_tokens = body
        .get("usage")
        .and_then(|u| u.get("output_tokens"))
        .and_then(|t| t.as_u64())
        .unwrap_or(0) as u32;

    let finish_reason = body
        .get("stop_reason")
        .and_then(|s| s.as_str())
        .unwrap_or("end_turn")
        .to_string();

    ProviderResponse {
        content,
        tool_calls,
        prompt_tokens,
        completion_tokens,
        finish_reason,
        model: body
            .get("model")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
            .map(str::to_string),
    }
}
