//! Starting the daemon, one way, whichever door it is started through.
//!
//! `vitna-coded` and `vitna serve` are two names for one daemon. Until this
//! module each decided for itself where to listen and where to keep the
//! journal, and `vitna serve` decided wrong: it opened a store beside whatever
//! folder it was run from, printed "Vitna daemon listening on local session
//! channel", and bound nothing, so a client probing the declared endpoint found
//! no daemon however long it waited. The release scripts ship `vitna` and not
//! `vitna-coded`, which made that the only door an installed copy had.
//!
//! Both binaries now resolve their endpoint and journal here and start through
//! [`run`], so the two cannot drift apart again.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use vitna_providers::{
    AnchorConfig, AnthropicProvider, CredentialResolver, KeyringStore, OpenAIProvider, Provider,
    ProviderId, RuntimeLane, RuntimeLaneProvider, VitnaAnchorProvider, DEFAULT_ANCHOR_BASE_URL,
};

use crate::{ipc, DaemonServer, McpServerSpec, ModelRoute};

/// A direct customer-held provider route whose API key is resolved strictly
/// from the OS keychain (`KeyringStore`), never from environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectProvider {
    pub provider: ProviderId,
    pub model: String,
    pub base_url: Option<String>,
}

/// A local-first `vitna-anchor` route (loopback-only by default, no API key
/// required) with optional local checkpoint SHA-256 attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRoute {
    pub model: String,
    pub base_url: String,
    pub checkpoint_path: Option<PathBuf>,
}

/// Where the daemon listens, where it keeps its journal, and which model a
/// turn reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub endpoint: String,
    pub store: PathBuf,
    /// The vitna-runtime lane turns are served through.
    pub route: Option<RuntimeLane>,
    /// Direct OS-keychain provider route (`anthropic` or `openai`).
    pub direct: Option<DirectProvider>,
    /// Local `vitna-anchor` inference server route (`--provider vitna-anchor`).
    pub anchor: Option<AnchorRoute>,
    /// External MCP servers attached at startup before serving IPC.
    pub mcp_servers: Vec<McpServerSpec>,
    /// Operator-configured destination hosts permitted for brokered `net:http_fetch`.
    pub egress_allowlist: Vec<String>,
    /// Requested sandbox guarantee for `run_command` (`read_only`, `guarded`, or `strong`).
    pub sandbox_guarantee: String,
    /// Explicit OCI container image required when `sandbox_guarantee == "strong"`.
    pub sandbox_image: Option<String>,
    /// Optional path to a `vitna-mandate-config-v1` JSON file enforcing an `am/1`
    /// Agent Mandate delegation chain across attached MCP tools and receipts.
    pub mandate_file: Option<PathBuf>,
    /// Optional path to a signed `vitna-model-policy-v1` JSON file enforcing
    /// allowed providers, model lanes, and minimum sandbox guarantee.
    pub policy_file: Option<PathBuf>,
    /// Canonical SHA-256 digest of the verified `SignedModelPolicy` when
    /// `policy_file` is configured, stamped onto signed receipts.
    pub verified_policy_digest: Option<String>,
}

impl Launch {
    /// What was asked for, or else the defaults: the endpoint ADR-0005 declares
    /// for this user, which is the one every client probes, and
    /// `.vitna/daemon.db` under the home directory, so a daemon started from
    /// any folder keeps the one journal.
    ///
    /// Refuses rather than guessing when a default cannot be formed, because a
    /// guessed endpoint is a daemon no client will find, and a guessed journal
    /// is a second history nobody knows to look in.
    pub fn resolve(endpoint: Option<String>, store: Option<PathBuf>) -> Result<Launch, String> {
        let endpoint = match endpoint {
            Some(e) => e,
            None => vitna_protocol::endpoint::preferred()?,
        };
        let store = match store {
            Some(s) => s,
            None => default_store()?,
        };
        Ok(Launch {
            endpoint,
            store,
            route: None,
            direct: None,
            anchor: None,
            mcp_servers: Vec::new(),
            egress_allowlist: Vec::new(),
            sandbox_guarantee: "guarded".to_string(),
            sandbox_image: None,
            mandate_file: None,
            policy_file: None,
            verified_policy_digest: None,
        })
    }

    /// Serves turns through a vitna-runtime lane: `--runtime-url` and
    /// `--lane`, both or neither. One without the other is refused rather than
    /// defaulted, because a default lane is a model nobody chose, and a
    /// default address is a runtime nobody started.
    pub fn with_runtime(
        mut self,
        runtime_url: Option<String>,
        lane: Option<String>,
    ) -> Result<Launch, String> {
        self.route = match (runtime_url, lane) {
            (None, None) => None,
            (Some(url), Some(lane)) => {
                if self.direct.is_some() || self.anchor.is_some() {
                    return Err(
                        "cannot combine --runtime-url/--lane with --provider/--model; pick one model route"
                            .to_string(),
                    );
                }
                Some(RuntimeLane::new(&url, &lane)?)
            }
            (Some(_), None) => {
                return Err("--runtime-url needs --lane, the lane to ask for".to_string())
            }
            (None, Some(_)) => {
                return Err("--lane needs --runtime-url, where the runtime listens".to_string())
            }
        };
        Ok(self)
    }

    /// Serves turns directly against Anthropic, OpenAI (using a customer-held
    /// API key in the OS keychain), or a local `vitna-anchor` server.
    pub fn with_direct_provider(
        mut self,
        provider: Option<String>,
        model: Option<String>,
        base_url: Option<String>,
    ) -> Result<Launch, String> {
        match (provider, model, base_url) {
            (None, None, None) => Ok(self),
            (None, None, Some(_)) => {
                Err("--base-url requires both --provider and --model".to_string())
            }
            (Some(_), None, _) => {
                Err("--provider needs --model, the model SKU to request".to_string())
            }
            (None, Some(_), _) => {
                Err("--model needs --provider (anthropic, openai, or vitna-anchor)".to_string())
            }
            (Some(prov), Some(model), base_url) => {
                if self.route.is_some() {
                    return Err(
                        "cannot combine --provider/--model with --runtime-url/--lane; pick one model route"
                            .to_string(),
                    );
                }
                let normalized = prov.trim().to_ascii_lowercase();
                let model = model.trim().to_string();
                if model.is_empty() {
                    return Err("--model must be non-empty".to_string());
                }
                if normalized == "vitna-anchor" || normalized == "anchor" {
                    let url = base_url
                        .map(|raw| raw.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| DEFAULT_ANCHOR_BASE_URL.to_string());
                    // Validate URL scheme and loopback-only policy eagerly at startup.
                    let cfg = AnchorConfig::default().with_base_url(&url);
                    let validated = VitnaAnchorProvider::new(cfg)?;
                    self.anchor = Some(AnchorRoute {
                        model,
                        base_url: validated.config().base_url.clone(),
                        checkpoint_path: None,
                    });
                    return Ok(self);
                }
                let provider_id = ProviderId::from_name(&normalized).ok_or_else(|| {
                    format!(
                        "unknown provider '{prov}': expected 'anthropic', 'openai', or 'vitna-anchor'"
                    )
                })?;
                let validated_base = match base_url {
                    Some(raw) => {
                        let trimmed = raw.trim();
                        if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
                            return Err(format!(
                                "invalid --base-url '{trimmed}': expected an http:// or https:// URL"
                            ));
                        }
                        Some(trimmed.to_string())
                    }
                    None => None,
                };
                self.direct = Some(DirectProvider {
                    provider: provider_id,
                    model,
                    base_url: validated_base,
                });
                Ok(self)
            }
        }
    }

    /// Attaches a local GGUF checkpoint file or model directory to a
    /// `vitna-anchor` route for SHA-256 checkpoint attestation on receipts.
    pub fn with_checkpoint(mut self, checkpoint: Option<PathBuf>) -> Result<Launch, String> {
        let Some(path) = checkpoint else {
            return Ok(self);
        };
        let Some(anchor) = self.anchor.as_mut() else {
            return Err(
                "--checkpoint is only valid with --provider vitna-anchor".to_string(),
            );
        };
        anchor.checkpoint_path = Some(path);
        Ok(self)
    }

    /// Attaches external MCP servers declared via `--mcp-http` and `--mcp-stdio`.
    pub fn with_mcp_servers(
        mut self,
        http_specs: Vec<String>,
        stdio_specs: Vec<String>,
    ) -> Result<Launch, String> {
        let mut seen = HashSet::new();
        for existing in &self.mcp_servers {
            seen.insert(existing.name().to_string());
        }
        for raw in http_specs {
            let spec = McpServerSpec::parse_http(&raw)?;
            if !seen.insert(spec.name().to_string()) {
                return Err(format!("duplicate MCP server name '{}'", spec.name()));
            }
            self.mcp_servers.push(spec);
        }
        for raw in stdio_specs {
            let spec = McpServerSpec::parse_stdio(&raw)?;
            if !seen.insert(spec.name().to_string()) {
                return Err(format!("duplicate MCP server name '{}'", spec.name()));
            }
            self.mcp_servers.push(spec);
        }
        Ok(self)
    }

    /// Configures destination hosts permitted for brokered `net:http_fetch`.
    ///
    /// Rejects blanket wildcards (`*`), schemes, paths, ports, and empty entries
    /// so misconfigured CLI flags fail closed at startup rather than silently
    /// widening egress policy.
    pub fn with_egress_allowlist(mut self, hosts: Vec<String>) -> Result<Launch, String> {
        let mut seen: HashSet<String> = self.egress_allowlist.iter().cloned().collect();
        for raw in hosts {
            let normalized = vitna_tools::validate_allowlist_entry(&raw)?;
            if seen.insert(normalized.clone()) {
                self.egress_allowlist.push(normalized);
            }
        }
        Ok(self)
    }

    /// Configures the requested sandbox guarantee (`read_only`, `guarded`, or
    /// `strong`) and optional explicit OCI container image for `strong`.
    pub fn with_sandbox(
        mut self,
        guarantee: Option<String>,
        image: Option<String>,
    ) -> Result<Launch, String> {
        let normalized = guarantee
            .as_deref()
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "guarded".to_string());
        match normalized.as_str() {
            "read_only" | "guarded" => {
                if image.is_some() {
                    return Err(format!(
                        "--sandbox-image requires --sandbox-guarantee strong (got '{normalized}')"
                    ));
                }
                self.sandbox_guarantee = normalized;
                self.sandbox_image = None;
                Ok(self)
            }
            "strong" => {
                let raw = image.ok_or_else(|| {
                    "--sandbox-guarantee strong requires --sandbox-image <OCI_IMAGE>".to_string()
                })?;
                let validated = vitna_runner::validate_container_image(&raw)?;
                self.sandbox_guarantee = "strong".to_string();
                self.sandbox_image = Some(validated.to_string());
                Ok(self)
            }
            other => Err(format!(
                "invalid --sandbox-guarantee '{other}': expected 'read_only', 'guarded', or 'strong'"
            )),
        }
    }

    /// Configures an `am/1` Agent Mandate bundle JSON file (`--mandate <FILE>`).
    pub fn with_mandate(mut self, mandate_file: Option<PathBuf>) -> Result<Launch, String> {
        if let Some(ref path) = mandate_file {
            // Validate JSON shape and cryptographic chain eagerly at startup.
            let _ = vitna_mcp::LiveMandateSession::from_file(path)?;
        }
        self.mandate_file = mandate_file;
        Ok(self)
    }

    /// Configures and eagerly enforces a signed `vitna-model-policy-v1` JSON file
    /// (`--policy <FILE>`) against the configured model route and sandbox guarantee.
    pub fn with_policy(mut self, policy_file: Option<PathBuf>) -> Result<Launch, String> {
        let Some(path) = policy_file else {
            self.policy_file = None;
            self.verified_policy_digest = None;
            return Ok(self);
        };
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read model policy {}: {e}", path.display()))?;
        let policy: vitna_policy::SignedModelPolicy = serde_json::from_str(&raw)
            .map_err(|e| format!("invalid model policy {}: {e}", path.display()))?;
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let route_pair: Option<(&str, &str)> = if let Some(lane) = &self.route {
            Some((vitna_providers::RUNTIME_PROVIDER_NAME, lane.lane.as_str()))
        } else if let Some(direct) = &self.direct {
            Some((direct.provider.as_str(), direct.model.as_str()))
        } else if let Some(anchor) = &self.anchor {
            Some(("vitna-anchor", anchor.model.as_str()))
        } else {
            None
        };

        let digest = match route_pair {
            Some((provider, model_sku)) => {
                policy.enforce(now_secs, None, provider, model_sku, &self.sandbox_guarantee)?
            }
            None => {
                let d = policy.verify_for_use(now_secs, None)?;
                if !vitna_policy::sandbox_satisfies_requirement(
                    &self.sandbox_guarantee,
                    &policy.required_sandbox_guarantee,
                ) {
                    return Err(format!(
                        "sandbox guarantee '{}' does not satisfy model policy '{}' requirement '{}'",
                        self.sandbox_guarantee, policy.policy_id, policy.required_sandbox_guarantee
                    ));
                }
                d
            }
        };

        self.policy_file = Some(path);
        self.verified_policy_digest = Some(digest);
        Ok(self)
    }
}

/// `.vitna/daemon.db` under the home directory.
///
/// An empty `HOME` counts as unset. Joined onto an empty path the journal would
/// land beside the current folder instead, which is the defect this module
/// exists to remove.
pub fn default_store() -> Result<PathBuf, String> {
    vitna_home()
        .map(|home| home.join("daemon.db"))
        .map_err(|e| format!("{e}, so --store must be given"))
}

/// `.vitna` under the home directory: this account's Vitna folder, where the
/// daemon keeps its journal by default and the device key always.
pub fn vitna_home() -> Result<PathBuf, String> {
    let home = ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .ok_or_else(|| "neither HOME nor USERPROFILE is set".to_string())?;
    Ok(PathBuf::from(home).join(".vitna"))
}

/// The daemon's log, on stdout, at `info` unless `RUST_LOG` says otherwise.
/// Installs the process's one global subscriber, so it is called once, by the
/// binary about to run the daemon.
pub fn logging() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

/// Opens the journal and serves the declared wire until the listener fails.
///
/// `on_ready` runs once the endpoint is bound and not before, so a line it
/// prints is true when it is printed.
pub async fn run<F: FnOnce()>(launch: &Launch, on_ready: F) -> Result<(), String> {
    let mut server = DaemonServer::open_default(&launch.store)?;
    if let Some(lane) = &launch.route {
        tracing::info!(runtime = %lane.endpoint, lane = %lane.lane, "turns go to a vitna-runtime lane");
        let telemetry = server.telemetry();
        server = server.with_route(ModelRoute {
            provider: Arc::new(RuntimeLaneProvider::new(lane.clone()).with_telemetry(telemetry)),
            model: lane.lane.clone(),
            routing_reason: format!("runtime_lane:{}", lane.lane),
        });
    } else if let Some(direct) = &launch.direct {
        let provider_id = direct.provider;
        let mut creds = tokio::task::spawn_blocking(move || {
            let store = KeyringStore::open()?;
            let resolver = CredentialResolver::new(Arc::new(store));
            resolver.resolve(provider_id)
        })
        .await
        .map_err(|e| format!("keychain lookup task failed: {e}"))?
        .map_err(|e| {
            format!(
                "{e}. Store a key with `vitna auth set {}`",
                provider_id.as_str()
            )
        })?;
        creds.base_url = direct.base_url.clone();
        let telemetry = server.telemetry();
        let adapter: Arc<dyn Provider> = match provider_id {
            ProviderId::Anthropic => Arc::new(AnthropicProvider::new(creds).with_telemetry(telemetry)),
            ProviderId::OpenAi => Arc::new(OpenAIProvider::new(creds).with_telemetry(telemetry)),
        };
        tracing::info!(
            provider = provider_id.as_str(),
            model = %direct.model,
            "turns go directly to customer-held provider via OS keychain"
        );
        server = server.with_route(ModelRoute {
            provider: adapter,
            model: direct.model.clone(),
            routing_reason: format!("direct_keychain:{}", provider_id.as_str()),
        });
    } else if let Some(anchor) = &launch.anchor {
        let mut cfg = AnchorConfig::default().with_base_url(&anchor.base_url);
        if let Some(cp) = &anchor.checkpoint_path {
            cfg = cfg.with_checkpoint_path(cp);
        }
        let adapter = VitnaAnchorProvider::new(cfg)?;
        let checkpoint_digest = if anchor.checkpoint_path.is_some() {
            let att = adapter.build_attestation().await?;
            tracing::info!(
                endpoint = %anchor.base_url,
                model = %anchor.model,
                checkpoint_sha256 = ?att.checkpoint_sha256,
                "verified local vitna-anchor checkpoint attestation"
            );
            att.checkpoint_sha256
        } else {
            match adapter.build_attestation().await {
                Ok(att) => att.checkpoint_sha256,
                Err(_) => None,
            }
        };
        tracing::info!(
            endpoint = %anchor.base_url,
            model = %anchor.model,
            "turns go to local vitna-anchor inference engine"
        );
        server = server
            .with_route(ModelRoute {
                provider: Arc::new(adapter),
                model: anchor.model.clone(),
                routing_reason: format!("vitna_anchor:{}", anchor.model),
            })
            .with_policy_digest(checkpoint_digest.or_else(|| launch.verified_policy_digest.clone()));
    } else {
        tracing::info!(
            "no model route: SubmitTurn is refused until --runtime-url/--lane or --provider/--model are given"
        );
    }

    if launch.anchor.is_none() {
        if let Some(ref digest) = launch.verified_policy_digest {
            tracing::info!(
                policy_digest = %digest,
                "enforcing verified vitna-model-policy-v1 policy"
            );
            server = server.with_policy_digest(Some(digest.clone()));
        }
    }

    server = server.with_sandbox(
        launch.sandbox_guarantee.clone(),
        launch.sandbox_image.clone(),
    );

    if let Some(mandate_path) = &launch.mandate_file {
        let session = vitna_mcp::LiveMandateSession::from_file(mandate_path)?;
        tracing::info!(
            root_jti = session.root_jti(),
            chain_hash = session.chain_hash(),
            "enforcing verified am/1 Agent Mandate on external MCP tools and receipts"
        );
        server = server.with_mandate(Arc::new(session));
    }

    for spec in &launch.mcp_servers {
        let tools = server.attach_mcp_server(spec).await?;
        tracing::info!(
            server = spec.name(),
            tool_count = tools.len(),
            "attached external MCP server"
        );
    }

    if !launch.egress_allowlist.is_empty() {
        tracing::info!(
            hosts = %launch.egress_allowlist.join(","),
            "brokered net:http_fetch egress allowlist enabled"
        );
        server = server.with_egress_allowlist(launch.egress_allowlist.clone());
    }

    let server = Arc::new(server);
    tracing::info!(store = %launch.store.display(), "vitna-coded starting");
    ipc::serve(server, &launch.endpoint, on_ready)
        .await
        .map_err(|e| format!("listening on {} failed: {e}", launch.endpoint))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both or neither. One alone is a route half described, and defaulting
    /// the other half would pick a model or a runtime nobody named.
    #[test]
    fn a_runtime_route_takes_both_flags_or_neither() {
        let base = || Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();
        assert_eq!(base().with_runtime(None, None).unwrap().route, None);
        let lane = base()
            .with_runtime(
                Some("http://127.0.0.1:8787".into()),
                Some("vitna/code".into()),
            )
            .unwrap()
            .route
            .expect("a route");
        assert_eq!(lane.endpoint, "http://127.0.0.1:8787/v1/chat/completions");
        assert_eq!(lane.lane, "vitna/code");
        assert!(base()
            .with_runtime(Some("http://127.0.0.1:8787".into()), None)
            .is_err());
        assert!(base()
            .with_runtime(None, Some("vitna/code".into()))
            .is_err());
        assert!(base()
            .with_runtime(
                Some("https://runtime.example.com".into()),
                Some("vitna/code".into())
            )
            .is_err());
    }

    #[test]
    fn direct_provider_route_validates_flags_and_rejects_ambiguous_runtime_mix() {
        let base = || Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();

        let direct = base()
            .with_direct_provider(
                Some("anthropic".into()),
                Some("claude-sonnet-4-20250514".into()),
                Some("https://api.anthropic.com/v1/messages".into()),
            )
            .unwrap()
            .direct
            .expect("direct route");
        assert_eq!(direct.provider, ProviderId::Anthropic);
        assert_eq!(direct.model, "claude-sonnet-4-20250514");

        // Half-specified or unknown provider rejected.
        assert!(base()
            .with_direct_provider(Some("openai".into()), None, None)
            .is_err());
        assert!(base()
            .with_direct_provider(None, Some("gpt-4o".into()), None)
            .is_err());
        assert!(base()
            .with_direct_provider(Some("unknown".into()), Some("m".into()), None)
            .is_err());
        assert!(base()
            .with_direct_provider(
                Some("openai".into()),
                Some("gpt-4o".into()),
                Some("ftp://bad".into()),
            )
            .is_err());

        // Combining runtime lane and direct provider is refused.
        let with_rt = base()
            .with_runtime(
                Some("http://127.0.0.1:8787".into()),
                Some("vitna/code".into()),
            )
            .unwrap();
        assert!(with_rt
            .with_direct_provider(Some("openai".into()), Some("gpt-4o".into()), None)
            .is_err());
    }

    #[test]
    fn mcp_server_flags_parse_and_reject_duplicate_names() {
        let base = Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();
        let configured = base
            .clone()
            .with_mcp_servers(
                vec!["docs=http://127.0.0.1:9000/mcp".into()],
                vec!["tools=my-mcp --flag".into()],
            )
            .unwrap();
        assert_eq!(configured.mcp_servers.len(), 2);

        assert!(base
            .with_mcp_servers(
                vec!["dup=http://127.0.0.1:9000/mcp".into()],
                vec!["dup=my-mcp".into()],
            )
            .is_err());
    }

    #[test]
    fn egress_allowlist_validates_and_deduplicates_entries() {
        let base = || Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();
        let configured = base()
            .with_egress_allowlist(vec![
                "API.Example.COM".into(),
                "api.example.com".into(),
                "*.docs.rs".into(),
                "127.0.0.1".into(),
            ])
            .expect("valid hosts should pass");
        assert_eq!(
            configured.egress_allowlist,
            vec!["api.example.com", "*.docs.rs", "127.0.0.1"]
        );

        assert!(base().with_egress_allowlist(vec!["*".into()]).is_err());
        assert!(base()
            .with_egress_allowlist(vec!["https://example.com".into()])
            .is_err());
        assert!(base()
            .with_egress_allowlist(vec!["example.com:443".into()])
            .is_err());
        assert!(base()
            .with_egress_allowlist(vec!["example.com/path".into()])
            .is_err());
    }

    #[test]
    fn vitna_anchor_route_enforces_loopback_and_checkpoint_flag() {
        let base = || Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();

        let configured = base()
            .with_direct_provider(
                Some("vitna-anchor".into()),
                Some("qwen3-coder-30b".into()),
                Some("http://127.0.0.1:8765".into()),
            )
            .unwrap()
            .with_checkpoint(Some(PathBuf::from("/tmp/model.gguf")))
            .unwrap();
        let anchor = configured.anchor.expect("anchor route");
        assert_eq!(anchor.model, "qwen3-coder-30b");
        assert_eq!(anchor.base_url, "http://127.0.0.1:8765");
        assert_eq!(anchor.checkpoint_path, Some(PathBuf::from("/tmp/model.gguf")));
        assert!(configured.direct.is_none());

        // Non-loopback vitna-anchor URLs are refused fail-closed.
        assert!(base()
            .with_direct_provider(
                Some("vitna-anchor".into()),
                Some("qwen3-coder-30b".into()),
                Some("https://remote.example.com".into()),
            )
            .is_err());

        // --checkpoint without --provider vitna-anchor is refused.
        assert!(base()
            .with_checkpoint(Some(PathBuf::from("/tmp/model.gguf")))
            .is_err());
    }

    #[test]
    fn sandbox_flags_validate_strong_image_and_reject_flag_injection() {
        let base = || Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();

        let strong = base()
            .with_sandbox(Some("strong".into()), Some("alpine:3.20".into()))
            .expect("valid strong config");
        assert_eq!(strong.sandbox_guarantee, "strong");
        assert_eq!(strong.sandbox_image.as_deref(), Some("alpine:3.20"));

        // Strong without image fails closed.
        assert!(base().with_sandbox(Some("strong".into()), None).is_err());
        // Image without strong fails closed.
        assert!(base()
            .with_sandbox(Some("guarded".into()), Some("alpine:3.20".into()))
            .is_err());
        // Flag-injection image name fails closed.
        assert!(base()
            .with_sandbox(Some("strong".into()), Some("--privileged".into()))
            .is_err());
        // Unknown guarantee fails closed.
        assert!(base().with_sandbox(Some("magic".into()), None).is_err());
    }

    #[test]
    fn what_is_asked_for_is_what_is_used() {
        let launch = Launch::resolve(Some("an-endpoint".into()), Some(PathBuf::from("a.db")))
            .expect("nothing to default");
        assert_eq!(launch.endpoint, "an-endpoint");
        assert_eq!(launch.store, PathBuf::from("a.db"));
    }

    /// The defaults are the endpoint a client probes and one journal under the
    /// home directory, whatever folder the daemon is started from.
    #[test]
    fn the_defaults_are_the_probed_endpoint_and_one_journal_under_home() {
        let (Ok(endpoint), Ok(store)) = (vitna_protocol::endpoint::preferred(), default_store())
        else {
            eprintln!(
                "skipped: this machine has no per-user endpoint or home directory to default to"
            );
            return;
        };
        let launch = Launch::resolve(None, None).expect("the defaults resolve");
        assert_eq!(launch.endpoint, endpoint);
        assert!(
            vitna_protocol::endpoint::candidates().contains(&launch.endpoint),
            "the daemon would bind {} where no client looks",
            launch.endpoint
        );
        assert_eq!(launch.store, store);
        assert!(
            launch
                .store
                .ends_with(std::path::Path::new(".vitna").join("daemon.db")),
            "{}",
            launch.store.display()
        );
        assert!(
            launch.store.is_absolute(),
            "a relative journal moves with the folder the daemon was started from: {}",
            launch.store.display()
        );
    }

    #[test]
    fn with_policy_enforces_route_and_sandbox_fail_closed() {
        let dir = std::env::temp_dir().join(format!(
            "vitna_launch_policy_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let policy_path = dir.join("policy.json");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let key = vitna_policy::generate_policy_signing_key();
        let mut policy = vitna_policy::SignedModelPolicy {
            schema_version: vitna_policy::MODEL_POLICY_SCHEMA_V1.to_string(),
            policy_id: "daemon-launch-policy".to_string(),
            issued_at: now.saturating_sub(60),
            expires_at: now + 3600,
            allowed_providers: vec!["vitna-runtime".to_string()],
            lanes: vec![vitna_policy::ModelLaneConfig {
                lane: "vitna/code".to_string(),
                provider: "vitna-runtime".to_string(),
                model_sku: "vitna/code".to_string(),
            }],
            required_sandbox_guarantee: "strong".to_string(),
            public_key: String::new(),
            signature: String::new(),
        };
        policy.sign(&key).unwrap();
        let expected_digest = policy.policy_digest().unwrap();
        std::fs::write(&policy_path, serde_json::to_string_pretty(&policy).unwrap()).unwrap();

        let base = || Launch::resolve(Some("e".into()), Some(PathBuf::from("a.db"))).unwrap();

        // Matching runtime lane + strong OCI sandbox succeeds and records verified digest.
        let ok = base()
            .with_runtime(
                Some("http://127.0.0.1:8787".into()),
                Some("vitna/code".into()),
            )
            .unwrap()
            .with_sandbox(Some("strong".into()), Some("alpine:3.20".into()))
            .unwrap()
            .with_policy(Some(policy_path.clone()))
            .expect("matching route + strong sandbox allowed");
        assert_eq!(ok.verified_policy_digest.as_deref(), Some(expected_digest.as_str()));

        // Guarded sandbox under a `strong` policy fails closed.
        assert!(base()
            .with_runtime(
                Some("http://127.0.0.1:8787".into()),
                Some("vitna/code".into()),
            )
            .unwrap()
            .with_policy(Some(policy_path.clone()))
            .is_err());

        // Disallowed lane under a matching strong sandbox fails closed.
        assert!(base()
            .with_runtime(
                Some("http://127.0.0.1:8787".into()),
                Some("vitna/other".into()),
            )
            .unwrap()
            .with_sandbox(Some("strong".into()), Some("alpine:3.20".into()))
            .unwrap()
            .with_policy(Some(policy_path.clone()))
            .is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
