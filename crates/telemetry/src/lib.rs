//! Local-only observability, egress accounting, metrics recording, and redacted trace export.
//!
//! This crate performs zero network I/O. Every recorded metric and egress record
//! stays in local memory until explicitly exported through `export_redacted_json`,
//! which scrubs secret-shaped tokens before serialization.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const REDACTED_SECRET_PLACEHOLDER: &str = "[REDACTED_SECRET]";

/// Accounting record for a single provider or remote endpoint egress call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRecord {
    pub provider: String,
    pub host: String,
    pub endpoint: String,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub timestamp_ms: u64,
}

/// Local numeric metric event with structured labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricEvent {
    pub name: String,
    pub value: f64,
    pub labels: BTreeMap<String, String>,
    pub timestamp_ms: u64,
}

/// Self-describing redacted telemetry bundle exported locally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedactedTelemetryExport {
    pub schema_version: String,
    pub local_only: bool,
    pub egress_records: Vec<EgressRecord>,
    pub metrics: Vec<MetricEvent>,
}

#[derive(Default)]
struct CollectorState {
    egress: Vec<EgressRecord>,
    metrics: Vec<MetricEvent>,
}

/// In-memory local-only telemetry collector.
#[derive(Default)]
pub struct TelemetryCollector {
    state: Mutex<CollectorState>,
}

impl TelemetryCollector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an egress call locally with secret scrubbing applied before storage.
    pub fn record_egress(&self, record: EgressRecord) {
        let sanitized = sanitize_egress(&record);
        tracing::debug!(
            target: "vitna::telemetry::egress",
            provider = %sanitized.provider,
            host = %sanitized.host,
            endpoint = %sanitized.endpoint,
            bytes_sent = sanitized.bytes_sent,
            bytes_received = sanitized.bytes_received,
            "recorded local egress accounting entry"
        );
        if let Ok(mut guard) = self.state.lock() {
            guard.egress.push(sanitized);
        }
    }

    /// Records a local metric event with secret scrubbing applied to names and labels.
    pub fn record_metric(
        &self,
        name: impl Into<String>,
        value: f64,
        labels: BTreeMap<String, String>,
    ) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let event = sanitize_metric(&MetricEvent {
            name: name.into(),
            value,
            labels,
            timestamp_ms: now_ms,
        });
        tracing::debug!(
            target: "vitna::telemetry::metric",
            metric = %event.name,
            value = event.value,
            "recorded local metric"
        );
        if let Ok(mut guard) = self.state.lock() {
            guard.metrics.push(event);
        }
    }

    pub fn egress_records(&self) -> Vec<EgressRecord> {
        self.state
            .lock()
            .map(|g| g.egress.clone())
            .unwrap_or_default()
    }

    pub fn metrics(&self) -> Vec<MetricEvent> {
        self.state
            .lock()
            .map(|g| g.metrics.clone())
            .unwrap_or_default()
    }

    /// Exports all collected metrics and egress records as redacted JSON.
    pub fn export_redacted_json(&self) -> Result<String, serde_json::Error> {
        let (egress_records, metrics) = match self.state.lock() {
            Ok(guard) => (
                guard.egress.iter().map(sanitize_egress).collect(),
                guard.metrics.iter().map(sanitize_metric).collect(),
            ),
            Err(_) => (Vec::new(), Vec::new()),
        };

        let bundle = RedactedTelemetryExport {
            schema_version: "vitna-telemetry-export-v1".to_string(),
            local_only: true,
            egress_records,
            metrics,
        };
        serde_json::to_string_pretty(&bundle)
    }
}

fn sanitize_egress(rec: &EgressRecord) -> EgressRecord {
    EgressRecord {
        provider: redact_secrets(&rec.provider),
        host: redact_secrets(&rec.host),
        endpoint: redact_secrets(&rec.endpoint),
        bytes_sent: rec.bytes_sent,
        bytes_received: rec.bytes_received,
        timestamp_ms: rec.timestamp_ms,
    }
}

fn sanitize_metric(ev: &MetricEvent) -> MetricEvent {
    let labels = ev
        .labels
        .iter()
        .map(|(k, v)| (redact_secrets(k), redact_secrets(v)))
        .collect();
    MetricEvent {
        name: redact_secrets(&ev.name),
        value: ev.value,
        labels,
        timestamp_ms: ev.timestamp_ms,
    }
}

/// Known secret token prefixes and minimum suffix lengths to scrub.
const SECRET_PREFIXES: &[(&str, usize)] = &[
    ("sk-ant-", 8),
    ("sk-proj-", 8),
    ("sk-or-", 8),
    ("sk-", 12),
    ("ghp_", 12),
    ("gho_", 12),
    ("ghu_", 12),
    ("ghs_", 12),
    ("ghr_", 12),
    ("xoxb-", 8),
    ("xoxp-", 8),
    ("xoxa-", 8),
    ("AKIA", 12),
    ("AIza", 12),
];

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'+' | b'/' | b'=')
}

/// Scrubs Bearer tokens, common API key prefixes, and `api_key=...` query/header
/// secrets from arbitrary text.
pub fn redact_secrets(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        let rest = &input[i..];

        // 1. Case-insensitive "Bearer <token>"
        if rest.len() > 7 && rest[..7].eq_ignore_ascii_case("bearer ") {
            let after_bearer = i + 7;
            let mut end = after_bearer;
            while end < bytes.len() && is_token_byte(bytes[end]) {
                end += 1;
            }
            if end > after_bearer {
                out.push_str(&input[i..after_bearer]);
                out.push_str(REDACTED_SECRET_PLACEHOLDER);
                i = end;
                continue;
            }
        }

        // 2. Query / header style "api_key=<token>" or "token=<token>"
        let mut matched_kv = false;
        for kw in ["api_key=", "apikey=", "access_token=", "auth_token="] {
            if rest.len() > kw.len() && rest[..kw.len()].eq_ignore_ascii_case(kw) {
                let val_start = i + kw.len();
                let mut end = val_start;
                while end < bytes.len() && is_token_byte(bytes[end]) {
                    end += 1;
                }
                if end > val_start {
                    out.push_str(&input[i..val_start]);
                    out.push_str(REDACTED_SECRET_PLACEHOLDER);
                    i = end;
                    matched_kv = true;
                    break;
                }
            }
        }
        if matched_kv {
            continue;
        }

        // 3. Known provider / cloud key prefixes at token boundaries
        let at_boundary = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let mut matched_prefix = false;
        if at_boundary {
            for &(prefix, min_suffix) in SECRET_PREFIXES {
                if rest.starts_with(prefix) {
                    let mut end = i + prefix.len();
                    while end < bytes.len() && is_token_byte(bytes[end]) {
                        end += 1;
                    }
                    if end - (i + prefix.len()) >= min_suffix {
                        out.push_str(REDACTED_SECRET_PLACEHOLDER);
                        i = end;
                        matched_prefix = true;
                        break;
                    }
                }
            }
        }
        if matched_prefix {
            continue;
        }

        let ch = rest.chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_secrets_scrubs_known_token_formats() {
        let raw = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.secret.sig \
                   anthropic=sk-ant-api03-abcdef1234567890 \
                   openrouter=sk-or-v1-1234567890abcdef \
                   openai=sk-1234567890abcdef1234 \
                   github=ghp_1234567890abcdef1234 \
                   slack=xoxb-1234567890-abcdef \
                   aws=AKIAIOSFODNN7EXAMPLE \
                   url=https://api.example.com/v1?api_key=supersecretvalue123";

        let scrubbed = redact_secrets(raw);
        for leaked in [
            "eyJhbGciOiJIUzI1NiJ9",
            "sk-ant-api03-abcdef1234567890",
            "sk-or-v1-1234567890abcdef",
            "sk-1234567890abcdef1234",
            "ghp_1234567890abcdef1234",
            "xoxb-1234567890-abcdef",
            "AKIAIOSFODNN7EXAMPLE",
            "supersecretvalue123",
        ] {
            assert!(
                !scrubbed.contains(leaked),
                "leaked token '{leaked}' remained in: {scrubbed}"
            );
        }
        assert!(scrubbed.contains("Bearer [REDACTED_SECRET]"));
        assert!(scrubbed.contains("api_key=[REDACTED_SECRET]"));
    }

    #[test]
    fn redact_secrets_preserves_normal_text() {
        let text = "model=claude-sonnet-4 latency_ms=128 path=src/lib.rs sk-short";
        assert_eq!(redact_secrets(text), text);
    }

    #[test]
    fn collector_exports_local_only_redacted_json() {
        let collector = TelemetryCollector::new();
        collector.record_egress(EgressRecord {
            provider: "anthropic".to_string(),
            host: "api.anthropic.com".to_string(),
            endpoint: "https://api.anthropic.com/v1/messages?api_key=sk-ant-secret123456789".to_string(),
            bytes_sent: 1024,
            bytes_received: 2048,
            timestamp_ms: 1_700_000_000_000,
        });

        let mut labels = BTreeMap::new();
        labels.insert("auth".to_string(), "Bearer tok_secret_987654321".to_string());
        labels.insert("lane".to_string(), "coding".to_string());
        collector.record_metric("turn_latency_ms", 42.5, labels);

        let exported = collector.export_redacted_json().expect("export json");
        assert!(!exported.contains("sk-ant-secret123456789"));
        assert!(!exported.contains("tok_secret_987654321"));

        let parsed: RedactedTelemetryExport = serde_json::from_str(&exported).expect("parse export");
        assert_eq!(parsed.schema_version, "vitna-telemetry-export-v1");
        assert!(parsed.local_only);
        assert_eq!(parsed.egress_records.len(), 1);
        assert_eq!(parsed.egress_records[0].host, "api.anthropic.com");
        assert_eq!(parsed.metrics.len(), 1);
        assert_eq!(parsed.metrics[0].labels.get("lane").map(String::as_str), Some("coding"));
        assert_eq!(
            parsed.metrics[0].labels.get("auth").map(String::as_str),
            Some("Bearer [REDACTED_SECRET]")
        );
    }
}