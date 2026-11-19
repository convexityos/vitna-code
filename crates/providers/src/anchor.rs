//! Local-first provider adapter for the `vitna-anchor` inference engine.
//!
//! Connects to a locally running `vitna-anchor serve` instance (default
//! `http://127.0.0.1:8765`), supports `/health` and `/v1/models` discovery,
//! speaks OpenAI-compatible `/v1/chat/completions` with tool calls, and
//! computes deterministic SHA-256 checkpoint attestations for run receipts.
//!
//! By design, this module reads zero environment variables (preserving
//! `vitna-providers`'s no-ambient-environment invariant) and restricts
//! endpoints to loopback hosts unless explicitly overridden.

use crate::{Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default base URL for `vitna-anchor serve` (matches `engine/src/main.c` port 8765).
pub const DEFAULT_ANCHOR_BASE_URL: &str = "http://127.0.0.1:8765";

/// Configuration for [`VitnaAnchorProvider`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorConfig {
    /// Base URL without trailing slash (e.g. `http://127.0.0.1:8765`).
    pub base_url: String,
    /// Optional explicit Bearer token if the local server sits behind auth.
    pub bearer_token: Option<String>,
    /// Optional local GGUF file or model directory hashed into [`AnchorAttestation`].
    pub checkpoint_path: Option<PathBuf>,
    /// When true (the default), refuses non-loopback endpoints fail-closed to
    /// prevent accidental exfiltration of local workspace context.
    pub enforce_loopback_only: bool,
    /// HTTP timeout for discovery and completion calls.
    pub timeout: Duration,
}

impl Default for AnchorConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_ANCHOR_BASE_URL.to_string(),
            bearer_token: None,
            checkpoint_path: None,
            enforce_loopback_only: true,
            timeout: Duration::from_secs(120),
        }
    }
}

impl AnchorConfig {
    pub fn local_default() -> Self {
        Self::default()
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = normalize_base_url(&base_url.into());
        self
    }

    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    pub fn with_checkpoint_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.checkpoint_path = Some(path.into());
        self
    }

    /// Explicitly permits a non-loopback endpoint (for example a LAN GPU host).
    pub fn allow_remote(mut self) -> Self {
        self.enforce_loopback_only = false;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// Response shape of `vitna-anchor`'s `GET /health` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorHealth {
    pub ok: bool,
    pub engine: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub generation: bool,
    #[serde(default)]
    pub context: Option<usize>,
    #[serde(default)]
    pub parallel: Option<usize>,
    #[serde(default)]
    pub chat_template: Option<String>,
    #[serde(default)]
    pub embeddings: Option<bool>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub checkpoint_sha256: Option<String>,
}

/// One model entry returned by `vitna-anchor`'s `GET /v1/models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorModelEntry {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AnchorModelsResponse {
    object: String,
    data: Vec<AnchorModelEntry>,
}

/// Cryptographic provenance snapshot describing the local `vitna-anchor`
/// server and checkpoint backing a coding run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnchorAttestation {
    pub engine: String,
    pub endpoint: String,
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint_sha256: Option<String>,
}

impl AnchorAttestation {
    /// Deterministic SHA-256 hex digest over canonical attestation JSON.
    ///
    /// Suitable for recording in `ModelSelectionRecord.policy_digest` or
    /// `EvidenceItemRecord.artifact_digest` inside `VitnaRunReceiptV1`.
    pub fn attestation_digest(&self) -> String {
        let mut map = serde_json::Map::new();
        if let Some(ref tpl) = self.chat_template {
            map.insert("chat_template".to_string(), json!(tpl));
        }
        if let Some(ref ckpt) = self.checkpoint_sha256 {
            map.insert("checkpoint_sha256".to_string(), json!(ckpt));
        }
        if let Some(ctx) = self.context_window {
            map.insert("context_window".to_string(), json!(ctx));
        }
        map.insert("endpoint".to_string(), json!(self.endpoint));
        map.insert("engine".to_string(), json!(self.engine));
        map.insert("model_id".to_string(), json!(self.model_id));
        // serde_json::Map preserves BTreeMap ordering when `preserve_order` is off;
        // we also insert keys in exact ASCII sorted order above.
        let canonical = serde_json::to_string(&serde_json::Value::Object(map))
            .unwrap_or_default();
        hex::encode(Sha256::digest(canonical.as_bytes()))
    }
}

/// Provider adapter targeting a local (or explicitly permitted remote) `vitna-anchor` server.
pub struct VitnaAnchorProvider {
    config: AnchorConfig,
    client: reqwest::Client,
}

impl VitnaAnchorProvider {
    /// Builds a `VitnaAnchorProvider`, enforcing loopback-only policy unless
    /// `config.enforce_loopback_only` is false.
    pub fn new(mut config: AnchorConfig) -> Result<Self, String> {
        config.base_url = normalize_base_url(&config.base_url);
        let parsed = reqwest::Url::parse(&config.base_url)
            .map_err(|e| format!("invalid vitna-anchor base_url '{}': {}", config.base_url, e))?;
        match parsed.scheme() {
            "http" | "https" => {}
            other => {
                return Err(format!(
                    "unsupported vitna-anchor URL scheme '{}' (expected http or https)",
                    other
                ));
            }
        }
        let loopback = is_loopback_host(parsed.host_str());
        if config.enforce_loopback_only && !loopback {
            return Err(format!(
                "vitna-anchor loopback guard refused non-loopback endpoint '{}'; call AnchorConfig::allow_remote() to override",
                config.base_url
            ));
        }

        let mut builder = reqwest::Client::builder().timeout(config.timeout);
        if loopback {
            // Loopback calls must never route through an outbound HTTP proxy.
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|e| format!("failed to build vitna-anchor HTTP client: {}", e))?;

        Ok(Self { config, client })
    }

    pub fn config(&self) -> &AnchorConfig {
        &self.config
    }

    pub fn health_endpoint(&self) -> String {
        format!("{}/health", self.config.base_url)
    }

    pub fn models_endpoint(&self) -> String {
        format!("{}/v1/models", self.config.base_url)
    }

    pub fn chat_completions_endpoint(&self) -> String {
        format!("{}/v1/chat/completions", self.config.base_url)
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref tok) = self.config.bearer_token {
            req.header("Authorization", format!("Bearer {}", tok))
        } else {
            req
        }
    }

    /// Probes `GET /health` and verifies the peer reports `engine == "vitna-anchor"`.
    pub async fn health(&self) -> Result<AnchorHealth, String> {
        let resp = self
            .apply_auth(self.client.get(self.health_endpoint()))
            .send()
            .await
            .map_err(|e| format!("vitna-anchor /health network error: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("vitna-anchor /health error (status {}): {}", status, body));
        }

        let health: AnchorHealth = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse vitna-anchor /health JSON: {}", e))?;

        if !health.ok {
            return Err("vitna-anchor /health reported ok=false".to_string());
        }
        if health.engine != "vitna-anchor" {
            return Err(format!(
                "unexpected inference engine '{}' at {} (expected 'vitna-anchor')",
                health.engine, self.config.base_url
            ));
        }
        Ok(health)
    }

    /// Probes `GET /health` and fails closed unless a generation model is loaded.
    pub async fn discover_generation_model(&self) -> Result<AnchorHealth, String> {
        let health = self.health().await?;
        if !health.generation || health.model.is_none() {
            let detail = health
                .message
                .as_deref()
                .unwrap_or("server is not serving a text generation model");
            return Err(format!(
                "vitna-anchor at {} cannot serve coding completions: {}",
                self.config.base_url, detail
            ));
        }
        Ok(health)
    }

    /// Lists loaded models from `GET /v1/models`.
    pub async fn list_models(&self) -> Result<Vec<AnchorModelEntry>, String> {
        let resp = self
            .apply_auth(self.client.get(self.models_endpoint()))
            .send()
            .await
            .map_err(|e| format!("vitna-anchor /v1/models network error: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "vitna-anchor /v1/models error (status {}): {}",
                status, body
            ));
        }

        let parsed: AnchorModelsResponse = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse vitna-anchor /v1/models JSON: {}", e))?;
        Ok(parsed.data)
    }

    /// Probes `/health` and builds an [`AnchorAttestation`].
    ///
    /// If `checkpoint_path` is configured, hashes the local checkpoint files and
    /// fails closed if the server also advertises a conflicting `checkpoint_sha256`.
    /// Otherwise uses any server-reported `checkpoint_sha256` from `/health`.
    pub async fn build_attestation(&self) -> Result<AnchorAttestation, String> {
        let health = self.discover_generation_model().await?;
        let model_id = health
            .model
            .ok_or_else(|| "vitna-anchor reported generation=true without model id".to_string())?;
        let checkpoint_sha256 = match (&self.config.checkpoint_path, health.checkpoint_sha256) {
            (Some(path), Some(server_hex)) => {
                let local_hex = compute_checkpoint_digest(path)?;
                if !local_hex.eq_ignore_ascii_case(&server_hex) {
                    return Err(format!(
                        "vitna-anchor checkpoint_sha256 mismatch: local '{}' != server '{}'",
                        local_hex, server_hex
                    ));
                }
                Some(local_hex)
            }
            (Some(path), None) => Some(compute_checkpoint_digest(path)?),
            (None, Some(server_hex)) => Some(server_hex.to_ascii_lowercase()),
            (None, None) => None,
        };
        Ok(AnchorAttestation {
            engine: health.engine,
            endpoint: self.config.base_url.clone(),
            model_id,
            context_window: health.context,
            chat_template: health.chat_template,
            checkpoint_sha256,
        })
    }

    /// Formats a [`ProviderRequest`] for `vitna-anchor`'s `/v1/chat/completions`.
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

        if let Some(temp) = req.temperature {
            payload["temperature"] = json!(temp);
        }
        if let Some(mt) = req.max_tokens {
            payload["max_tokens"] = json!(mt);
        }
        if !req.tools.is_empty() {
            let mut tools = Vec::with_capacity(req.tools.len());
            for tool in &req.tools {
                tools.push(json!({
                    "type": "function",
                    "function": {
                        "name": tool.get("name").unwrap_or(&json!("")),
                        "description": tool.get("description").unwrap_or(&json!("")),
                        "parameters": tool.get("parameters_schema").unwrap_or(&json!({}))
                    }
                }));
            }
            payload["tools"] = json!(tools);
        }
        payload
    }

    /// Parses an OpenAI-compatible SSE `data:` line emitted by `vitna-anchor`.
    pub fn parse_sse_event_line(&self, data: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        let mut events = Vec::new();
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return Ok(events);
        }
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
                        if !content.is_empty() {
                            events.push(ProviderStreamEvent::TextDelta(content.to_string()));
                        }
                    }
                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|tc| tc.as_array()) {
                        for tc in tool_calls {
                            let index =
                                tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                            let id = tc.get("id").and_then(|i| i.as_str()).map(str::to_string);
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                .map(str::to_string);
                            let arguments_delta = tc
                                .get("function")
                                .and_then(|f| f.get("arguments"))
                                .and_then(|a| a.as_str())
                                .unwrap_or_default()
                                .to_string();

                            events.push(ProviderStreamEvent::ToolCallDelta {
                                index,
                                id,
                                name,
                                arguments_delta,
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
            let prompt_tokens = usage
                .get("prompt_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32;
            let completion_tokens = usage
                .get("completion_tokens")
                .and_then(|t| t.as_u64())
                .unwrap_or(0) as u32;
            events.push(ProviderStreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
            });
        }

        Ok(events)
    }
}

#[async_trait]
impl Provider for VitnaAnchorProvider {
    fn name(&self) -> &str {
        "vitna-anchor"
    }

    fn parse_sse_event(
        &self,
        _event_type: &str,
        data: &str,
    ) -> Result<Vec<ProviderStreamEvent>, String> {
        self.parse_sse_event_line(data)
    }

    async fn complete(&self, req: &ProviderRequest) -> Result<ProviderResponse, String> {
        let mut payload = self.format_request_payload(req);
        payload["stream"] = json!(false);

        let resp = self
            .apply_auth(self.client.post(self.chat_completions_endpoint()))
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("vitna-anchor completion network error: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!(
                "vitna-anchor API error (status {}): {}",
                status, body
            ));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("failed to parse vitna-anchor response JSON: {}", e))?;

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
                            let id = tc
                                .get("id")
                                .and_then(|i| i.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let args_val = match tc.get("function").and_then(|f| f.get("arguments")) {
                                Some(serde_json::Value::String(s)) => {
                                    serde_json::from_str(s).unwrap_or_else(|_| json!({}))
                                }
                                Some(other) => other.clone(),
                                None => json!({}),
                            };
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

        let model = body
            .get("model")
            .and_then(|m| m.as_str())
            .map(str::to_string);

        Ok(ProviderResponse {
            content,
            tool_calls,
            prompt_tokens,
            completion_tokens,
            finish_reason,
            model,
        })
    }
}

/// Computes a deterministic SHA-256 hex digest for a local checkpoint file
/// (e.g. a `.gguf` file) or a checkpoint directory (sorted manifest of all
/// `.gguf`, `.safetensors`, `config.json`, and `tokenizer*.json` files).
pub fn compute_checkpoint_digest(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path)
        .map_err(|e| format!("cannot stat checkpoint '{}': {}", path.display(), e))?;
    if meta.is_file() {
        return hash_file_sha256(path);
    }
    if !meta.is_dir() {
        return Err(format!(
            "checkpoint '{}' is neither a file nor a directory",
            path.display()
        ));
    }

    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for entry in fs::read_dir(path)
        .map_err(|e| format!("cannot read checkpoint directory '{}': {}", path.display(), e))?
    {
        let entry = entry.map_err(|e| format!("dir entry error: {}", e))?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_checkpoint_artifact(&name) {
            entries.push((name, p));
        }
    }
    if entries.is_empty() {
        return Err(format!(
            "checkpoint directory '{}' contains no recognized model artifacts (.gguf, .safetensors, config.json, tokenizer*.json)",
            path.display()
        ));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut manifest_hasher = Sha256::new();
    for (name, file_path) in entries {
        let file_hex = hash_file_sha256(&file_path)?;
        manifest_hasher.update(name.as_bytes());
        manifest_hasher.update(b":");
        manifest_hasher.update(file_hex.as_bytes());
        manifest_hasher.update(b"\n");
    }
    Ok(hex::encode(manifest_hasher.finalize()))
}

fn is_checkpoint_artifact(name: &str) -> bool {
    name.ends_with(".gguf")
        || name.ends_with(".safetensors")
        || name == "config.json"
        || name == "tokenizer.json"
        || name == "tokenizer_config.json"
}

fn hash_file_sha256(path: &Path) -> Result<String, String> {
    let mut f = File::open(path)
        .map_err(|e| format!("cannot open '{}': {}", path.display(), e))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("error reading '{}': {}", path.display(), e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn normalize_base_url(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn is_loopback_host(host: Option<&str>) -> bool {
    match host {
        Some("localhost") | Some("127.0.0.1") | Some("::1") | Some("[::1]") => true,
        Some(h) if h.starts_with("127.") => h
            .parse::<std::net::Ipv4Addr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false),
        _ => false,
    }
}
