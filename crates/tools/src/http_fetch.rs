//! Approval-gated, allowlist-enforced `http_fetch` tool (`net:http_fetch`).
//!
//! Fetches HTTP/HTTPS resources exclusively through [`crate::egress::EgressBroker`]
//! so every call is bound by the active egress allowlist, DNS/SSRF validation,
//! redirect re-validation, response byte caps, and cryptographic
//! `EgressCallRecord` emission into the run receipt.

use crate::egress::{EgressBroker, EgressPolicy};
use crate::{Tool, ToolContext, ToolDefinition, ToolResult};
use async_trait::async_trait;
use serde_json::json;

/// Maximum UTF-8 bytes included inline in the tool output handed back to the
/// model (full response bytes up to `DEFAULT_MAX_EGRESS_BYTES` are hashed).
const MAX_INLINE_PREVIEW_BYTES: usize = 64 * 1024;

pub struct HttpFetchTool;

#[async_trait]
impl Tool for HttpFetchTool {
    fn name(&self) -> &str {
        "http_fetch"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "http_fetch".to_string(),
            description: "Fetch an HTTP or HTTPS URL through the fail-closed egress broker. Requires exact-action approval and an operator-configured egress allowlist; every request emits a cryptographic egress_call record in the run receipt.".to_string(),
            parameters_schema: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "HTTP or HTTPS URL on an allowlisted host."
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "HEAD"],
                        "description": "HTTP method (defaults to GET)."
                    }
                },
                "required": ["url"]
            }),
            is_mutating: true,
            requires_approval: true,
        }
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let url = args
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required 'url' parameter".to_string())?;
        let method = args
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");

        let policy = EgressPolicy::with_allowlist(ctx.egress_allowlist.clone());
        let fetched = EgressBroker::fetch(&policy, method, url).await?;

        let truncated = fetched.body.len() > MAX_INLINE_PREVIEW_BYTES;
        let slice = if truncated {
            &fetched.body[..MAX_INLINE_PREVIEW_BYTES]
        } else {
            &fetched.body[..]
        };
        let body_text = String::from_utf8_lossy(slice);

        let mut output = format!(
            "HTTP {} {} -> {}\nSHA-256: {}\nBytes Received: {}\n",
            fetched.record.method,
            fetched.record.redacted_url,
            fetched.status_code,
            fetched.record.response_sha256,
            fetched.record.bytes_received,
        );
        if let Some(ref ct) = fetched.content_type {
            output.push_str(&format!("Content-Type: {ct}\n"));
        }
        output.push('\n');
        output.push_str(&body_text);
        if truncated {
            output.push_str(&format!(
                "\n[truncated inline preview at {} bytes; SHA-256 covers all {} bytes]",
                MAX_INLINE_PREVIEW_BYTES, fetched.record.bytes_received
            ));
        }

        let success = (200..400).contains(&fetched.status_code);
        Ok(ToolResult {
            call_id: String::new(),
            tool_name: "http_fetch".to_string(),
            success,
            output,
            preimage_hash: None,
            postimage_hash: Some(fetched.record.response_sha256.clone()),
            diff: None,
            exit_code: None,
            sandbox_backend: None,
            sandbox_enforcement: None,
            egress_calls: vec![fetched.record],
        })
    }
}
