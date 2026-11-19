//! Incremental Server-Sent Events (SSE) framing and stream response assembly
//! shared by HTTP provider adapters.

use crate::{ProviderResponse, ProviderStreamEvent, ProviderToolCall};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use vitna_telemetry::{EgressRecord, TelemetryCollector};

/// Byte-oriented incremental WHATWG SSE parser.
///
/// Splits incoming network chunks strictly on `0x0A` (`\n`) before UTF-8
/// decoding so arbitrary TCP packet boundaries can never split a multi-byte
/// UTF-8 scalar value.
#[derive(Debug, Default)]
pub(crate) struct SseParser {
    line_buf: Vec<u8>,
    event_type: String,
    data_lines: Vec<String>,
}

impl SseParser {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feeds raw bytes from the HTTP response stream and returns every complete
    /// `(event_type, data)` SSE frame delimited so far.
    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
        self.line_buf.extend_from_slice(bytes);
        let mut frames = Vec::new();

        let mut start = 0usize;
        while let Some(rel) = self.line_buf[start..].iter().position(|&b| b == b'\n') {
            let nl = start + rel;
            let mut end = nl;
            if end > start && self.line_buf[end - 1] == b'\r' {
                end -= 1;
            }
            let line = String::from_utf8_lossy(&self.line_buf[start..end]).into_owned();
            if let Some(frame) = self.process_line(&line) {
                frames.push(frame);
            }
            start = nl + 1;
        }

        if start > 0 {
            self.line_buf.drain(..start);
        }

        frames
    }

    /// Flushes any trailing unterminated line and pending event at stream EOF.
    pub(crate) fn finish(&mut self) -> Option<(String, String)> {
        let mut flushed = None;
        if !self.line_buf.is_empty() {
            let mut end = self.line_buf.len();
            if end > 0 && self.line_buf[end - 1] == b'\r' {
                end -= 1;
            }
            let line = String::from_utf8_lossy(&self.line_buf[..end]).into_owned();
            self.line_buf.clear();
            flushed = self.process_line(&line);
        }
        if flushed.is_some() {
            return flushed;
        }
        if !self.data_lines.is_empty() {
            let event_type = std::mem::take(&mut self.event_type);
            let data = self.data_lines.join("\n");
            self.data_lines.clear();
            return Some((event_type, data));
        }
        None
    }

    fn process_line(&mut self, line: &str) -> Option<(String, String)> {
        if line.is_empty() {
            if self.data_lines.is_empty() {
                self.event_type.clear();
                return None;
            }
            let event_type = std::mem::take(&mut self.event_type);
            let data = self.data_lines.join("\n");
            self.data_lines.clear();
            return Some((event_type, data));
        }

        // Lines beginning with ':' are SSE comments / keep-alives.
        if line.starts_with(':') {
            return None;
        }

        let (field, raw_value) = match line.split_once(':') {
            Some((f, rest)) => (f, rest.strip_prefix(' ').unwrap_or(rest)),
            None => (line, ""),
        };

        match field {
            "event" => {
                self.event_type = raw_value.to_string();
            }
            "data" => {
                self.data_lines.push(raw_value.to_string());
            }
            _ => {}
        }

        None
    }
}

#[derive(Debug, Default)]
struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Assembles normalized [`ProviderStreamEvent`] items into a final
/// [`ProviderResponse`] while forwarding text deltas to the caller immediately.
#[derive(Debug)]
pub(crate) struct StreamAccumulator {
    content: String,
    tool_calls: BTreeMap<usize, PartialToolCall>,
    prompt_tokens: u32,
    completion_tokens: u32,
    finish_reason: Option<String>,
    model: Option<String>,
    default_finish_reason: &'static str,
}

impl StreamAccumulator {
    pub(crate) fn new(default_finish_reason: &'static str) -> Self {
        Self {
            content: String::new(),
            tool_calls: BTreeMap::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            finish_reason: None,
            model: None,
            default_finish_reason,
        }
    }

    /// Captures the served model identifier from raw OpenAI or Anthropic SSE
    /// JSON frames (`model` or `message.model`).
    pub(crate) fn observe_raw_json(&mut self, data: &str) {
        if self.model.is_some() {
            return;
        }
        let trimmed = data.trim();
        if trimmed.is_empty() || trimmed == "[DONE]" {
            return;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            return;
        };
        let model = val
            .get("model")
            .and_then(|m| m.as_str())
            .or_else(|| {
                val.get("message")
                    .and_then(|msg| msg.get("model"))
                    .and_then(|m| m.as_str())
            })
            .filter(|m| !m.is_empty());
        if let Some(m) = model {
            self.model = Some(m.to_string());
        }
    }

    pub(crate) fn apply_events(
        &mut self,
        events: Vec<ProviderStreamEvent>,
        on_delta: &(dyn Fn(String) + Send + Sync),
    ) {
        for event in events {
            match event {
                ProviderStreamEvent::TextDelta(text) => {
                    if !text.is_empty() {
                        self.content.push_str(&text);
                        on_delta(text);
                    }
                }
                ProviderStreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments_delta,
                } => {
                    let entry = self.tool_calls.entry(index).or_default();
                    if let Some(id) = id {
                        if !id.is_empty() {
                            entry.id = id;
                        }
                    }
                    if let Some(name) = name {
                        if !name.is_empty() {
                            entry.name = name;
                        }
                    }
                    entry.arguments.push_str(&arguments_delta);
                }
                ProviderStreamEvent::Usage {
                    prompt_tokens,
                    completion_tokens,
                } => {
                    if prompt_tokens > 0 {
                        self.prompt_tokens = prompt_tokens;
                    }
                    if completion_tokens > 0 {
                        self.completion_tokens = completion_tokens;
                    }
                }
                ProviderStreamEvent::Finish(reason) => {
                    if reason.is_empty() {
                        continue;
                    }
                    // Preserve a more specific finish reason (such as `tool_use`
                    // or `tool_calls`) when a trailing sentinel (`[DONE]` or
                    // `message_stop`) emits a generic `stop`/`end_turn`.
                    let is_generic = matches!(reason.as_str(), "stop" | "end_turn");
                    if self.finish_reason.is_none() || !is_generic {
                        self.finish_reason = Some(reason);
                    }
                }
            }
        }
    }

    pub(crate) fn into_response(self) -> ProviderResponse {
        let tool_calls = self
            .tool_calls
            .into_values()
            .map(|tc| {
                let arguments = if tc.arguments.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    match serde_json::from_str(&tc.arguments) {
                        Ok(val) => val,
                        Err(_) => serde_json::Value::String(tc.arguments),
                    }
                };
                ProviderToolCall {
                    id: tc.id,
                    name: tc.name,
                    arguments,
                }
            })
            .collect();

        ProviderResponse {
            content: self.content,
            tool_calls,
            prompt_tokens: self.prompt_tokens,
            completion_tokens: self.completion_tokens,
            finish_reason: self
                .finish_reason
                .unwrap_or_else(|| self.default_finish_reason.to_string()),
            model: self.model,
        }
    }
}

pub(crate) fn is_event_stream(headers: &reqwest::header::HeaderMap) -> bool {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.to_ascii_lowercase().contains("text/event-stream"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn record_http_telemetry(
    telemetry: Option<&Arc<TelemetryCollector>>,
    provider_label: String,
    host: String,
    endpoint: String,
    bytes_sent: u64,
    bytes_received: u64,
    status: reqwest::StatusCode,
    elapsed_ms: f64,
) {
    if let Some(telemetry) = telemetry {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        telemetry.record_egress(EgressRecord {
            provider: provider_label.clone(),
            host,
            endpoint,
            bytes_sent,
            bytes_received,
            timestamp_ms: now_ms,
        });
        let mut labels = BTreeMap::new();
        labels.insert("provider".to_string(), provider_label);
        labels.insert("status".to_string(), status.as_u16().to_string());
        telemetry.record_metric("provider_http_latency_ms", elapsed_ms, labels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_reassembles_split_utf8_and_crlf_frames() {
        let mut parser = SseParser::new();
        let payload = "event: content_block_delta\r\ndata: {\"text\":\"héllo 🚀\"}\r\n\r\n: keepalive\r\ndata: [DONE]\n\n";
        let raw = payload.as_bytes();

        // Feed 3 bytes at a time to slice right through multi-byte UTF-8 and CRLF.
        let mut frames = Vec::new();
        for chunk in raw.chunks(3) {
            frames.extend(parser.push(chunk));
        }
        if let Some(tail) = parser.finish() {
            frames.push(tail);
        }

        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0],
            (
                "content_block_delta".to_string(),
                "{\"text\":\"héllo 🚀\"}".to_string()
            )
        );
        assert_eq!(frames[1], (String::new(), "[DONE]".to_string()));
    }

    #[test]
    fn accumulator_preserves_specific_finish_reason_and_malformed_tool_json() {
        let mut acc = StreamAccumulator::new("end_turn");
        let mut seen = Vec::new();
        let on_delta = |s: String| {
            let _ = s;
        };
        acc.observe_raw_json(r#"{"type":"message_start","message":{"model":"claude-3-7-sonnet"}}"#);
        acc.apply_events(
            vec![
                ProviderStreamEvent::Usage {
                    prompt_tokens: 21,
                    completion_tokens: 1,
                },
                ProviderStreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some("call_1".to_string()),
                    name: Some("read_file".to_string()),
                    arguments_delta: "{\"path\":".to_string(),
                },
                ProviderStreamEvent::ToolCallDelta {
                    index: 0,
                    id: None,
                    name: None,
                    arguments_delta: "\"README.md\"}".to_string(),
                },
                ProviderStreamEvent::ToolCallDelta {
                    index: 1,
                    id: Some("call_2".to_string()),
                    name: Some("broken".to_string()),
                    arguments_delta: "{not valid".to_string(),
                },
                ProviderStreamEvent::Usage {
                    prompt_tokens: 0,
                    completion_tokens: 34,
                },
                ProviderStreamEvent::Finish("tool_use".to_string()),
                ProviderStreamEvent::Finish("end_turn".to_string()),
            ],
            &on_delta,
        );
        seen.push("ok");

        let resp = acc.into_response();
        assert_eq!(seen, vec!["ok"]);
        assert_eq!(resp.model.as_deref(), Some("claude-3-7-sonnet"));
        assert_eq!(resp.prompt_tokens, 21);
        assert_eq!(resp.completion_tokens, 34);
        assert_eq!(resp.finish_reason, "tool_use");
        assert_eq!(resp.tool_calls.len(), 2);
        assert_eq!(
            resp.tool_calls[0].arguments,
            serde_json::json!({"path": "README.md"})
        );
        assert_eq!(
            resp.tool_calls[1].arguments,
            serde_json::Value::String("{not valid".to_string())
        );
    }
}
