//! Real MCP JSON-RPC 2.0 transports: newline-delimited stdio child processes
//! and Streamable HTTP endpoints, both with strict frame bounds, timeouts, and
//! response-ID validation.

use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// Hard upper bound on a single JSON-RPC response frame (4 MiB).
pub const MAX_MCP_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Default per-request timeout for MCP server calls.
pub const DEFAULT_TIMEOUT_MS: u64 = 15_000;

/// Maximum server notifications skipped while waiting for one response.
const MAX_SKIPPED_NOTIFICATIONS: usize = 256;

#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn request(&mut self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, String>;
    async fn notify(&mut self, notification: &JsonRpcNotification) -> Result<(), String>;
}

/// Validates that a JSON-RPC response uses protocol 2.0 and matches `expected_id`.
fn validate_response(resp: JsonRpcResponse, expected_id: u64) -> Result<JsonRpcResponse, String> {
    if resp.jsonrpc != "2.0" {
        return Err(format!(
            "MCP server returned invalid jsonrpc version '{}'",
            resp.jsonrpc
        ));
    }
    if resp.id != expected_id {
        return Err(format!(
            "MCP server response id mismatch: expected {expected_id}, got {}",
            resp.id
        ));
    }
    Ok(resp)
}

/// Reads a single newline-delimited line without ever buffering more than `max_bytes`.
async fn read_bounded_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<Option<String>, String> {
    let mut out = Vec::new();
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|e| format!("MCP stdio read failed: {e}"))?;
        if available.is_empty() {
            if out.is_empty() {
                return Ok(None);
            }
            break;
        }
        if let Some(pos) = available.iter().position(|&b| b == b'\n') {
            if out.len() + pos > max_bytes {
                return Err(format!(
                    "MCP stdio frame exceeded {max_bytes} byte limit"
                ));
            }
            out.extend_from_slice(&available[..pos]);
            reader.consume(pos + 1);
            break;
        } else {
            let len = available.len();
            if out.len() + len > max_bytes {
                return Err(format!(
                    "MCP stdio frame exceeded {max_bytes} byte limit"
                ));
            }
            out.extend_from_slice(available);
            reader.consume(len);
        }
    }
    if out.last() == Some(&b'\r') {
        out.pop();
    }
    String::from_utf8(out).map(Some).map_err(|e| format!("MCP stdio frame is not valid UTF-8: {e}"))
}

/// Newline-delimited JSON-RPC 2.0 transport over stdio (either a spawned child
/// process or any async reader/writer pair).
pub struct StdioMcpTransport {
    _child: Option<Child>,
    reader: BufReader<Box<dyn AsyncRead + Unpin + Send + Sync>>,
    writer: Box<dyn AsyncWrite + Unpin + Send + Sync>,
    timeout: Duration,
    max_frame_bytes: usize,
}

impl StdioMcpTransport {
    /// Spawns a local MCP server process with a cleared environment plus only
    /// the explicitly allowlisted `env` pairs.
    pub fn spawn(
        program: &str,
        args: &[String],
        env: &[(String, String)],
        timeout: Duration,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn MCP server '{program}': {e}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "MCP child stdin not captured".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "MCP child stdout not captured".to_string())?;

        Ok(Self {
            _child: Some(child),
            reader: BufReader::new(Box::new(stdout)),
            writer: Box::new(stdin),
            timeout,
            max_frame_bytes: MAX_MCP_FRAME_BYTES,
        })
    }

    /// Wraps any async reader and writer pair using the same stdio framing,
    /// timeout, and response validation rules.
    pub fn from_streams<R, W>(reader: R, writer: W, timeout: Duration) -> Self
    where
        R: AsyncRead + Unpin + Send + Sync + 'static,
        W: AsyncWrite + Unpin + Send + Sync + 'static,
    {
        Self {
            _child: None,
            reader: BufReader::new(Box::new(reader)),
            writer: Box::new(writer),
            timeout,
            max_frame_bytes: MAX_MCP_FRAME_BYTES,
        }
    }

    pub fn with_max_frame_bytes(mut self, max_frame_bytes: usize) -> Self {
        self.max_frame_bytes = max_frame_bytes;
        self
    }

    async fn write_line(&mut self, payload: &[u8]) -> Result<(), String> {
        self.writer
            .write_all(payload)
            .await
            .map_err(|e| format!("MCP stdio write failed: {e}"))?;
        self.writer
            .write_all(b"\n")
            .await
            .map_err(|e| format!("MCP stdio newline write failed: {e}"))?;
        self.writer
            .flush()
            .await
            .map_err(|e| format!("MCP stdio flush failed: {e}"))
    }
}

#[async_trait]
impl McpTransport for StdioMcpTransport {
    async fn request(&mut self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, String> {
        let encoded =
            serde_json::to_vec(req).map_err(|e| format!("failed to encode MCP request: {e}"))?;
        self.write_line(&encoded).await?;

        let deadline = self.timeout;
        let max_bytes = self.max_frame_bytes;
        let expected_id = req.id;

        tokio::time::timeout(deadline, async {
            let mut skipped = 0usize;
            loop {
                let line = read_bounded_line(&mut self.reader, max_bytes)
                    .await?
                    .ok_or_else(|| "MCP server closed stdout before answering".to_string())?;
                if line.trim().is_empty() {
                    continue;
                }
                let val: serde_json::Value = serde_json::from_str(&line)
                    .map_err(|e| format!("MCP server returned malformed JSON: {e}"))?;
                // Server notifications have no `id` field; skip them boundedly.
                if val.get("id").is_none() || val.get("id").is_some_and(|v| v.is_null()) {
                    skipped += 1;
                    if skipped > MAX_SKIPPED_NOTIFICATIONS {
                        return Err("MCP server sent too many notifications without answering".to_string());
                    }
                    continue;
                }
                let resp: JsonRpcResponse = serde_json::from_value(val)
                    .map_err(|e| format!("MCP server returned invalid JSON-RPC response: {e}"))?;
                return validate_response(resp, expected_id);
            }
        })
        .await
        .map_err(|_| {
            format!(
                "MCP request '{}' timed out after {} ms",
                req.method,
                deadline.as_millis()
            )
        })?
    }

    async fn notify(&mut self, notification: &JsonRpcNotification) -> Result<(), String> {
        let encoded = serde_json::to_vec(notification)
            .map_err(|e| format!("failed to encode MCP notification: {e}"))?;
        self.write_line(&encoded).await
    }
}

/// Streamable HTTP MCP transport (JSON-RPC 2.0 over HTTP POST, supporting both
/// `application/json` and `text/event-stream` SSE responses).
pub struct HttpMcpTransport {
    endpoint: String,
    http: reqwest::Client,
    session_id: Option<String>,
    bearer_token: Option<String>,
    timeout: Duration,
    max_frame_bytes: usize,
}

impl HttpMcpTransport {
    pub fn new(endpoint: impl Into<String>, timeout: Duration) -> Result<Self, String> {
        let endpoint = endpoint.into();
        if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
            return Err(format!(
                "MCP HTTP endpoint must use http:// or https://, got '{endpoint}'"
            ));
        }
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| format!("failed to build MCP HTTP client: {e}"))?;
        Ok(Self {
            endpoint,
            http,
            session_id: None,
            bearer_token: None,
            timeout,
            max_frame_bytes: MAX_MCP_FRAME_BYTES,
        })
    }

    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    pub fn with_max_frame_bytes(mut self, max_frame_bytes: usize) -> Self {
        self.max_frame_bytes = max_frame_bytes;
        self
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    async fn read_bounded_body(
        &self,
        mut response: reqwest::Response,
    ) -> Result<Vec<u8>, String> {
        if let Some(len) = response.content_length() {
            if len as usize > self.max_frame_bytes {
                return Err(format!(
                    "MCP HTTP response Content-Length {len} exceeds {} byte limit",
                    self.max_frame_bytes
                ));
            }
        }
        let mut buf = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| format!("MCP HTTP body read failed: {e}"))?
        {
            if buf.len() + chunk.len() > self.max_frame_bytes {
                return Err(format!(
                    "MCP HTTP response body exceeded {} byte limit",
                    self.max_frame_bytes
                ));
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }

    fn build_post<T: serde::Serialize>(&self, body: &T) -> reqwest::RequestBuilder {
        let mut req = self
            .http
            .post(&self.endpoint)
            .timeout(self.timeout)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(body);
        if let Some(sid) = &self.session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        if let Some(token) = &self.bearer_token {
            req = req.bearer_auth(token);
        }
        req
    }
}

fn parse_sse_jsonrpc_response(body: &str, expected_id: u64) -> Result<JsonRpcResponse, String> {
    let mut data_lines: Vec<&str> = Vec::new();
    let flush = |lines: &mut Vec<&str>| -> Result<Option<JsonRpcResponse>, String> {
        if lines.is_empty() {
            return Ok(None);
        }
        let joined = lines.join("\n");
        lines.clear();
        let trimmed = joined.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let val: serde_json::Value = serde_json::from_str(trimmed)
            .map_err(|e| format!("MCP SSE event contains malformed JSON: {e}"))?;
        if val.get("id").is_none() || val.get("id").is_some_and(|v| v.is_null()) {
            return Ok(None);
        }
        let resp: JsonRpcResponse = serde_json::from_value(val)
            .map_err(|e| format!("MCP SSE event is not a valid JSON-RPC response: {e}"))?;
        Ok(Some(validate_response(resp, expected_id)?))
    };

    for raw_line in body.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            if let Some(resp) = flush(&mut data_lines)? {
                return Ok(resp);
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.trim_start());
        }
    }
    if let Some(resp) = flush(&mut data_lines)? {
        return Ok(resp);
    }
    Err("MCP SSE response ended without a JSON-RPC response frame".to_string())
}

#[async_trait]
impl McpTransport for HttpMcpTransport {
    async fn request(&mut self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, String> {
        let response = self
            .build_post(req)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP request '{}' failed: {e}", req.method))?;

        if let Some(sid) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            if !sid.is_empty() {
                self.session_id = Some(sid.to_string());
            }
        }

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();

        let bytes = self.read_bounded_body(response).await?;
        if !status.is_success() {
            let preview = String::from_utf8_lossy(&bytes);
            return Err(format!(
                "MCP HTTP server returned {}: {}",
                status.as_u16(),
                preview.chars().take(256).collect::<String>()
            ));
        }

        if content_type.contains("text/event-stream") {
            let text = String::from_utf8(bytes)
                .map_err(|e| format!("MCP SSE body is not valid UTF-8: {e}"))?;
            parse_sse_jsonrpc_response(&text, req.id)
        } else {
            let resp: JsonRpcResponse = serde_json::from_slice(&bytes)
                .map_err(|e| format!("MCP HTTP server returned malformed JSON-RPC response: {e}"))?;
            validate_response(resp, req.id)
        }
    }

    async fn notify(&mut self, notification: &JsonRpcNotification) -> Result<(), String> {
        let response = self
            .build_post(notification)
            .send()
            .await
            .map_err(|e| format!("MCP HTTP notification '{}' failed: {e}", notification.method))?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "MCP HTTP server rejected notification '{}' with status {}",
                notification.method,
                status.as_u16()
            ));
        }
        Ok(())
    }
}
