//! Fail-closed `am/1` Agent Mandate enforcement bridge for MCP tools.
//!
//! Wraps any `vitna_tools::Tool` (including `McpToolBridge`) so that every tool
//! invocation is checked against:
//! 1. An explicit `ToolPricingCatalog` entry for `(server_name, tool_name)`.
//!    Unpriced tools are refused fail-closed before execution.
//! 2. Cryptographic verification of the `am/1` Ed25519 mandate delegation chain
//!    (`verify_mandate_chain`), including audience, resource URI, purpose,
//!    task binding, effect rank, per-txn cap, and step-up threshold.
//! 3. Atomic per-ancestor spend reservation in `MandateSpendTracker`.
//!    On tool failure, ONLY `Effect::Read` calls are refunded; `Effect::Write`
//!    and `Effect::Irreversible` calls remain debited to prevent
//!    failed-call-refund-retry budget drain.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use vitna_policy::mandate::{
    verify_mandate_chain, Effect, MandateActionRequest, MandateError, MandateProvenanceRecord,
    MandateSpendTracker, Money, VerifiedMandateChain, VerifyMandateOptions, DEFAULT_SKEW_SECONDS,
};
use vitna_tools::{Tool, ToolContext, ToolDefinition, ToolResult};

/// Pricing and effect classification for a single MCP tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPriceSpec {
    /// Canonical resource URI checked against mandate `aud` and `resources` caveats.
    pub resource_uri: String,
    /// Fixed per-call price in exact decimal `Money`.
    pub amount: Money,
    /// Side-effect class enforced against mandate `cav.effect`.
    pub effect: Effect,
    /// Optional purpose tag matched against mandate `cav.purpose`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
}

/// JSON-serializable tool pricing catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCatalogEntry {
    pub server: String,
    pub tool: String,
    #[serde(flatten)]
    pub spec: ToolPriceSpec,
}

/// Wire format for a JSON pricing catalog file (for example `mandate-pricing.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPricingCatalogFile {
    pub schema_version: String,
    pub tools: Vec<ToolCatalogEntry>,
}

/// Fail-closed lookup table mapping `(server_name, tool_name)` to `ToolPriceSpec`.
#[derive(Debug, Clone, Default)]
pub struct ToolPricingCatalog {
    entries: BTreeMap<(String, String), ToolPriceSpec>,
}

impl ToolPricingCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a pricing spec for `(server_name, tool_name)`.
    pub fn insert(
        &mut self,
        server_name: impl Into<String>,
        tool_name: impl Into<String>,
        spec: ToolPriceSpec,
    ) -> &mut Self {
        self.entries.insert((server_name.into(), tool_name.into()), spec);
        self
    }

    /// Looks up pricing for `(server_name, tool_name)`. Returns `None` if unpriced.
    pub fn get(&self, server_name: &str, tool_name: &str) -> Option<&ToolPriceSpec> {
        self.entries
            .get(&(server_name.to_string(), tool_name.to_string()))
    }

    /// Loads a `ToolPricingCatalog` from a JSON document matching `ToolPricingCatalogFile`.
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        let parsed: ToolPricingCatalogFile = serde_json::from_str(json)?;
        let mut catalog = Self::new();
        for entry in parsed.tools {
            catalog.insert(entry.server, entry.tool, entry.spec);
        }
        Ok(catalog)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns all registered pricing entries in deterministic sorted order.
    pub fn entries(&self) -> Vec<ToolCatalogEntry> {
        self.entries
            .iter()
            .map(|((server, tool), spec)| ToolCatalogEntry {
                server: server.clone(),
                tool: tool.clone(),
                spec: spec.clone(),
            })
            .collect()
    }
}

fn system_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Wraps a `Tool` with fail-closed `am/1` Agent Mandate verification and spend tracking.
pub struct MandatedToolBridge {
    server_name: String,
    inner: Arc<dyn Tool>,
    tokens: Vec<String>,
    trusted_root_issuers: HashSet<String>,
    ledger: String,
    pricing: Arc<ToolPricingCatalog>,
    tracker: Arc<MandateSpendTracker>,
    task_id: Option<String>,
    step_up_approved: bool,
    skew_seconds: u64,
    now_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl MandatedToolBridge {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        server_name: impl Into<String>,
        inner: Arc<dyn Tool>,
        tokens: Vec<String>,
        trusted_root_issuers: HashSet<String>,
        ledger: impl Into<String>,
        pricing: Arc<ToolPricingCatalog>,
        tracker: Arc<MandateSpendTracker>,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            inner,
            tokens,
            trusted_root_issuers,
            ledger: ledger.into(),
            pricing,
            tracker,
            task_id: None,
            step_up_approved: false,
            skew_seconds: DEFAULT_SKEW_SECONDS,
            now_fn: Arc::new(system_unix_secs),
        }
    }

    pub fn with_task_id(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn with_step_up_approved(mut self, approved: bool) -> Self {
        self.step_up_approved = approved;
        self
    }

    pub fn with_skew_seconds(mut self, skew_seconds: u64) -> Self {
        self.skew_seconds = skew_seconds;
        self
    }

    pub fn with_clock<F>(mut self, clock: F) -> Self
    where
        F: Fn() -> u64 + Send + Sync + 'static,
    {
        self.now_fn = Arc::new(clock);
        self
    }

    /// Verifies the mandate chain itself (without an action request).
    pub fn verify_chain(&self) -> Result<VerifiedMandateChain, MandateError> {
        verify_mandate_chain(
            &self.tokens,
            &VerifyMandateOptions {
                trusted_root_issuers: self.trusted_root_issuers.clone(),
                ledger: self.ledger.clone(),
                now: (self.now_fn)(),
                skew_seconds: self.skew_seconds,
                request: None,
                step_up_approved: self.step_up_approved,
            },
        )
    }

    /// Builds a `MandateProvenanceRecord` suitable for embedding in `VitnaRunReceiptV1`.
    pub fn provenance(&self, currency: &str) -> Result<MandateProvenanceRecord, MandateError> {
        let verified = self.verify_chain()?;
        Ok(self.tracker.provenance_for(&verified, currency))
    }
}

/// JSON configuration file loaded via `--mandate <FILE>` to enforce an `am/1`
/// Agent Mandate delegation chain and tool pricing catalog on a live daemon or
/// CLI run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateBundleConfig {
    pub schema_version: String,
    pub tokens: Vec<String>,
    pub trusted_root_issuers: Vec<String>,
    pub ledger: String,
    pub currency: String,
    #[serde(default)]
    pub pricing: Vec<ToolCatalogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default)]
    pub step_up_approved: bool,
}

pub const MANDATE_CONFIG_SCHEMA_V1: &str = "vitna-mandate-config-v1";
const MAX_MANDATE_CONFIG_BYTES: u64 = 1024 * 1024;

impl MandateBundleConfig {
    pub fn from_json_str(json: &str) -> Result<Self, String> {
        let cfg: Self = serde_json::from_str(json)
            .map_err(|e| format!("failed to parse mandate config JSON: {e}"))?;
        if cfg.schema_version != MANDATE_CONFIG_SCHEMA_V1 {
            return Err(format!(
                "unsupported mandate config schema_version '{}': expected '{}'",
                cfg.schema_version, MANDATE_CONFIG_SCHEMA_V1
            ));
        }
        if cfg.tokens.is_empty() {
            return Err("mandate config must contain at least one am/1 JWS token".to_string());
        }
        if cfg.trusted_root_issuers.is_empty() {
            return Err("mandate config must specify at least one trusted_root_issuers did:key".to_string());
        }
        if cfg.ledger.trim().is_empty() {
            return Err("mandate config ledger must be non-empty".to_string());
        }
        if cfg.currency.trim().is_empty() {
            return Err("mandate config currency must be non-empty".to_string());
        }
        Ok(cfg)
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let meta = std::fs::metadata(path)
            .map_err(|e| format!("cannot stat mandate file '{}': {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("mandate path '{}' is not a regular file", path.display()));
        }
        if meta.len() > MAX_MANDATE_CONFIG_BYTES {
            return Err(format!(
                "mandate file '{}' exceeds {} bytes",
                path.display(),
                MAX_MANDATE_CONFIG_BYTES
            ));
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read mandate file '{}': {e}", path.display()))?;
        Self::from_json_str(&raw)
    }
}

/// Verified, spend-tracked `am/1` Agent Mandate session attached to a daemon or
/// CLI run. Wraps attached external MCP tools with fail-closed enforcement and
/// emits live [`MandateProvenanceRecord`] snapshots for signed run receipts.
pub struct LiveMandateSession {
    tokens: Vec<String>,
    trusted_root_issuers: HashSet<String>,
    ledger: String,
    currency: String,
    pricing: Arc<ToolPricingCatalog>,
    tracker: Arc<MandateSpendTracker>,
    verified_chain: VerifiedMandateChain,
    task_id: Option<String>,
    step_up_approved: bool,
    now_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl LiveMandateSession {
    /// Verifies the mandate delegation chain eagerly (failing closed on expired,
    /// untrusted, tampered, or attenuation-violating chains) and prepares a
    /// shared spend tracker for all MCP tools attached to this session.
    pub fn from_bundle(bundle: MandateBundleConfig) -> Result<Self, String> {
        Self::from_bundle_with_clock(bundle, Arc::new(system_unix_secs))
    }

    pub fn from_bundle_with_clock(
        bundle: MandateBundleConfig,
        now_fn: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Self, String> {
        let trusted_root_issuers: HashSet<String> =
            bundle.trusted_root_issuers.into_iter().collect();
        let verified_chain = verify_mandate_chain(
            &bundle.tokens,
            &VerifyMandateOptions {
                trusted_root_issuers: trusted_root_issuers.clone(),
                ledger: bundle.ledger.clone(),
                now: (now_fn)(),
                skew_seconds: DEFAULT_SKEW_SECONDS,
                request: None,
                step_up_approved: bundle.step_up_approved,
            },
        )
        .map_err(|e| format!("mandate verification failed ({}): {}", e.code.as_str(), e.detail))?;

        let mut catalog = ToolPricingCatalog::new();
        for entry in bundle.pricing {
            catalog.insert(entry.server, entry.tool, entry.spec);
        }

        Ok(Self {
            tokens: bundle.tokens,
            trusted_root_issuers,
            ledger: bundle.ledger,
            currency: bundle.currency,
            pricing: Arc::new(catalog),
            tracker: Arc::new(MandateSpendTracker::new()),
            verified_chain,
            task_id: bundle.task_id,
            step_up_approved: bundle.step_up_approved,
            now_fn,
        })
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let bundle = MandateBundleConfig::from_file(path)?;
        Self::from_bundle(bundle)
    }

    pub fn root_jti(&self) -> &str {
        &self.verified_chain.root_jti
    }

    pub fn chain_hash(&self) -> &str {
        &self.verified_chain.chain_hash
    }

    pub fn verified_chain(&self) -> &VerifiedMandateChain {
        &self.verified_chain
    }

    pub fn currency(&self) -> &str {
        &self.currency
    }

    pub fn task_id(&self) -> Option<&str> {
        self.task_id.as_deref()
    }

    pub fn step_up_approved(&self) -> bool {
        self.step_up_approved
    }

    pub fn pricing_entries(&self) -> Vec<ToolCatalogEntry> {
        self.pricing.entries()
    }

    /// Performs a fail-closed dry-run authorization check for `(server_name, tool_name)`
    /// against the pricing catalog and `am/1` delegation chain without reserving spend.
    pub fn dry_run_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        step_up_override: Option<bool>,
    ) -> Result<ToolPriceSpec, String> {
        let ns_prefix = format!("mcp__{server_name}__");
        let bare_name = tool_name.strip_prefix(&ns_prefix).unwrap_or(tool_name);
        let spec = self
            .pricing
            .get(server_name, tool_name)
            .or_else(|| self.pricing.get(server_name, bare_name))
            .ok_or_else(|| {
                format!(
                    "mandate_refused:unpriced_tool: tool '{server_name}:{tool_name}' has no pricing entry in catalog"
                )
            })?;

        verify_mandate_chain(
            &self.tokens,
            &VerifyMandateOptions {
                trusted_root_issuers: self.trusted_root_issuers.clone(),
                ledger: self.ledger.clone(),
                now: (self.now_fn)(),
                skew_seconds: DEFAULT_SKEW_SECONDS,
                request: Some(MandateActionRequest {
                    resource: spec.resource_uri.clone(),
                    amount: spec.amount.clone(),
                    effect: Some(spec.effect),
                    purpose: spec.purpose.clone(),
                    task_id: self.task_id.clone(),
                }),
                step_up_approved: step_up_override.unwrap_or(self.step_up_approved),
            },
        )
        .map_err(|e| format!("mandate_refused:{}: {}", e.code.as_str(), e.detail))?;

        Ok(spec.clone())
    }

    /// Wraps an attached external MCP tool in [`MandatedToolBridge`] sharing
    /// this session's verified chain and atomic spend tracker.
    pub fn wrap_tool(&self, server_name: &str, inner: Arc<dyn Tool>) -> Arc<dyn Tool> {
        let mut bridge = MandatedToolBridge::new(
            server_name,
            inner,
            self.tokens.clone(),
            self.trusted_root_issuers.clone(),
            self.ledger.clone(),
            Arc::clone(&self.pricing),
            Arc::clone(&self.tracker),
        )
        .with_step_up_approved(self.step_up_approved);
        if let Some(ref tid) = self.task_id {
            bridge = bridge.with_task_id(tid.clone());
        }
        let clock = Arc::clone(&self.now_fn);
        bridge = bridge.with_clock(move || (clock)());
        Arc::new(bridge)
    }

    /// Snapshots the live [`MandateProvenanceRecord`] (including exact
    /// accumulated `total_spent` and `spend_digests`) for embedding in a signed
    /// `VitnaRunReceiptV1`.
    pub fn provenance_record(&self) -> MandateProvenanceRecord {
        self.tracker
            .provenance_for(&self.verified_chain, &self.currency)
    }
}

#[async_trait]
impl Tool for MandatedToolBridge {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn definition(&self) -> ToolDefinition {
        self.inner.definition()
    }

    fn compute_action_digest(&self, args: &serde_json::Value) -> String {
        self.inner.compute_action_digest(args)
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult, String> {
        let tool_name = self.inner.name();
        let ns_prefix = format!("mcp__{}__", self.server_name);
        let bare_name = tool_name.strip_prefix(&ns_prefix).unwrap_or(tool_name);
        let spec = self
            .pricing
            .get(&self.server_name, tool_name)
            .or_else(|| self.pricing.get(&self.server_name, bare_name))
            .ok_or_else(|| {
                format!(
                    "mandate_refused:unpriced_tool: tool '{}:{}' has no pricing entry in catalog",
                    self.server_name, tool_name
                )
            })?;

        let now = (self.now_fn)();
        let verified = verify_mandate_chain(
            &self.tokens,
            &VerifyMandateOptions {
                trusted_root_issuers: self.trusted_root_issuers.clone(),
                ledger: self.ledger.clone(),
                now,
                skew_seconds: self.skew_seconds,
                request: Some(MandateActionRequest {
                    resource: spec.resource_uri.clone(),
                    amount: spec.amount.clone(),
                    effect: Some(spec.effect),
                    purpose: spec.purpose.clone(),
                    task_id: self.task_id.clone(),
                }),
                step_up_approved: self.step_up_approved,
            },
        )
        .map_err(|e| format!("mandate_refused:{}: {}", e.code.as_str(), e.detail))?;

        let spend = self
            .tracker
            .reserve(&verified, &spec.resource_uri, &spec.amount, spec.effect, now)
            .map_err(|e| format!("mandate_refused:{}: {}", e.code.as_str(), e.detail))?;

        match self.inner.execute(args, ctx).await {
            Ok(result) => {
                if !result.success {
                    self.tracker.refund_if_reversible(&spend.spend_id);
                }
                Ok(result)
            }
            Err(err) => {
                self.tracker.refund_if_reversible(&spend.spend_id);
                Err(err)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use vitna_policy::generate_policy_signing_key;
    use vitna_policy::mandate::{
        did_key_from_verifying_key, jwk_thumbprint_from_verifying_key, sign_mandate_jws, CnfClaim,
        MandateCaveats, MandateClaims, StepUpCaveat, PROTOCOL_VERSION,
    };

    struct StubTool {
        tool_name: String,
        calls: Arc<AtomicUsize>,
        fail_mode: StubFailMode,
    }

    #[derive(Clone, Copy)]
    enum StubFailMode {
        Ok,
        ResultFailure,
        ErrFailure,
    }

    #[async_trait]
    impl Tool for StubTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.tool_name.clone(),
                description: "stub MCP tool".to_string(),
                parameters_schema: serde_json::json!({"type": "object"}),
                is_mutating: false,
                requires_approval: true,
            }
        }

        async fn execute(&self, _args: serde_json::Value, _ctx: &ToolContext) -> Result<ToolResult, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.fail_mode {
                StubFailMode::Ok => Ok(ToolResult {
                    call_id: "c1".to_string(),
                    tool_name: self.tool_name.clone(),
                    success: true,
                    output: "ok".to_string(),
                    ..Default::default()
                }),
                StubFailMode::ResultFailure => Ok(ToolResult {
                    call_id: "c1".to_string(),
                    tool_name: self.tool_name.clone(),
                    success: false,
                    output: "upstream 503".to_string(),
                    ..Default::default()
                }),
                StubFailMode::ErrFailure => Err("transport timeout".to_string()),
            }
        }
    }

    fn test_ctx() -> ToolContext {
        let journal_path = std::env::temp_dir().join(format!(
            "vitna_mandate_mcp_test_{}.journal",
            std::process::id()
        ));
        let fake_runner = vitna_runner::FakeRunner::new(&journal_path).expect("fake runner");
        let runner = Arc::new(std::sync::Mutex::new(fake_runner));
        ToolContext::new(PathBuf::from("."), runner)
    }

    fn issue_test_mandate(cav: MandateCaveats) -> (Vec<String>, HashSet<String>) {
        let principal = generate_policy_signing_key();
        let agent = generate_policy_signing_key();
        let p_did = did_key_from_verifying_key(&principal.verifying_key());
        let a_did = did_key_from_verifying_key(&agent.verifying_key());
        let claims = MandateClaims {
            v: PROTOCOL_VERSION.to_string(),
            typ: "mandate".to_string(),
            jti: "mnd_root_mcp".to_string(),
            iss: p_did.clone(),
            sub: a_did,
            par: None,
            iat: 1000,
            nbf: 1000,
            exp: 5000,
            cnf: CnfClaim {
                jkt: jwk_thumbprint_from_verifying_key(&agent.verifying_key()),
            },
            ledger: "https://ledger.convexityos.com".to_string(),
            cav,
            st: None,
        };
        let tok = sign_mandate_jws(&claims, &principal, None);
        let mut trusted = HashSet::new();
        trusted.insert(p_did);
        (vec![tok], trusted)
    }

    #[tokio::test]
    async fn enforces_pricing_effects_budgets_refunds_and_step_up() {
        let (tokens, trusted) = issue_test_mandate(MandateCaveats {
            currency: Some("USD".to_string()),
            max_total: Some(Money::new("USD", "0.50")),
            max_per_txn: Some(Money::new("USD", "0.30")),
            aud: Some(vec!["convexityos.com".to_string()]),
            resources: Some(vec!["https://convexityos.com/api/v1/mcp/*".to_string()]),
            purpose: Some(vec!["market.research".to_string()]),
            effect: Some(Effect::Read),
            step_up: Some(StepUpCaveat {
                over: Money::new("USD", "0.20"),
            }),
            ..Default::default()
        });

        let catalog = Arc::new(
            ToolPricingCatalog::from_json_str(
                r#"{
                    "schema_version": "am-mcp-pricing-v1",
                    "tools": [
                        {
                            "server": "convexity",
                            "tool": "get_quote",
                            "resource_uri": "https://convexityos.com/api/v1/mcp/get_quote",
                            "amount": { "cur": "USD", "amt": "0.15" },
                            "effect": "read",
                            "purpose": "market.research"
                        },
                        {
                            "server": "convexity",
                            "tool": "create_webhook",
                            "resource_uri": "https://convexityos.com/api/v1/mcp/create_webhook",
                            "amount": { "cur": "USD", "amt": "0.10" },
                            "effect": "write",
                            "purpose": "market.research"
                        },
                        {
                            "server": "convexity",
                            "tool": "deep_scan",
                            "resource_uri": "https://convexityos.com/api/v1/mcp/deep_scan",
                            "amount": { "cur": "USD", "amt": "0.25" },
                            "effect": "read",
                            "purpose": "market.research"
                        }
                    ]
                }"#,
            )
            .expect("valid catalog json"),
        );
        assert_eq!(catalog.len(), 3);

        let tracker = Arc::new(MandateSpendTracker::new());
        let ctx = test_ctx();

        // 1. Unpriced tool fails closed before touching inner tool.
        let unpriced_calls = Arc::new(AtomicUsize::new(0));
        let unpriced_bridge = MandatedToolBridge::new(
            "convexity",
            Arc::new(StubTool {
                tool_name: "unknown_tool".to_string(),
                calls: unpriced_calls.clone(),
                fail_mode: StubFailMode::Ok,
            }),
            tokens.clone(),
            trusted.clone(),
            "https://ledger.convexityos.com",
            catalog.clone(),
            tracker.clone(),
        )
        .with_clock(|| 2000);

        let err = unpriced_bridge
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("unpriced_tool"), "unexpected error: {}", err);
        assert_eq!(unpriced_calls.load(Ordering::SeqCst), 0);

        // 2. Write tool under Read-only mandate fails closed before touching inner tool.
        let write_calls = Arc::new(AtomicUsize::new(0));
        let write_bridge = MandatedToolBridge::new(
            "convexity",
            Arc::new(StubTool {
                tool_name: "create_webhook".to_string(),
                calls: write_calls.clone(),
                fail_mode: StubFailMode::Ok,
            }),
            tokens.clone(),
            trusted.clone(),
            "https://ledger.convexityos.com",
            catalog.clone(),
            tracker.clone(),
        )
        .with_clock(|| 2000);

        let err = write_bridge
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("effect_not_permitted"), "unexpected error: {}", err);
        assert_eq!(write_calls.load(Ordering::SeqCst), 0);

        // 3. Step-up threshold ($0.25 > $0.20) fails without step-up approval.
        let scan_calls = Arc::new(AtomicUsize::new(0));
        let scan_bridge = MandatedToolBridge::new(
            "convexity",
            Arc::new(StubTool {
                tool_name: "deep_scan".to_string(),
                calls: scan_calls.clone(),
                fail_mode: StubFailMode::Ok,
            }),
            tokens.clone(),
            trusted.clone(),
            "https://ledger.convexityos.com",
            catalog.clone(),
            tracker.clone(),
        )
        .with_clock(|| 2000);

        let err = scan_bridge
            .execute(serde_json::json!({}), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("step_up_required"), "unexpected error: {}", err);
        assert_eq!(scan_calls.load(Ordering::SeqCst), 0);

        // 4. Failing Read call is refunded automatically.
        let failing_read_calls = Arc::new(AtomicUsize::new(0));
        let failing_read = MandatedToolBridge::new(
            "convexity",
            Arc::new(StubTool {
                tool_name: "get_quote".to_string(),
                calls: failing_read_calls.clone(),
                fail_mode: StubFailMode::ErrFailure,
            }),
            tokens.clone(),
            trusted.clone(),
            "https://ledger.convexityos.com",
            catalog.clone(),
            tracker.clone(),
        )
        .with_clock(|| 2000);

        assert!(failing_read.execute(serde_json::json!({}), &ctx).await.is_err());
        assert_eq!(failing_read_calls.load(Ordering::SeqCst), 1);
        assert_eq!(tracker.spent_on("mnd_root_mcp"), Some(Money::new("USD", "0")));

        // 5. Happy-path Read calls accumulate up to $0.50 cap (3 x $0.15 = $0.45, 4th fails).
        let ok_read_calls = Arc::new(AtomicUsize::new(0));
        let ok_read = MandatedToolBridge::new(
            "convexity",
            Arc::new(StubTool {
                tool_name: "get_quote".to_string(),
                calls: ok_read_calls.clone(),
                fail_mode: StubFailMode::Ok,
            }),
            tokens.clone(),
            trusted.clone(),
            "https://ledger.convexityos.com",
            catalog.clone(),
            tracker.clone(),
        )
        .with_clock(|| 2000);

        for _ in 0..3 {
            let res = ok_read.execute(serde_json::json!({"symbol": "NVDA"}), &ctx).await.unwrap();
            assert!(res.success);
        }
        assert_eq!(ok_read_calls.load(Ordering::SeqCst), 3);

        let err = ok_read.execute(serde_json::json!({}), &ctx).await.unwrap_err();
        assert!(err.contains("budget_exhausted"), "unexpected error: {}", err);
        assert_eq!(ok_read_calls.load(Ordering::SeqCst), 3);

        let prov = ok_read.provenance("USD").expect("provenance must build");
        assert_eq!(prov.root_jti, "mnd_root_mcp");
        assert_eq!(prov.total_spent, "0.45");
        assert_eq!(prov.spend_digests.len(), 3);
    }

    #[tokio::test]
    async fn failed_write_tool_is_never_refunded() {
        let (tokens, trusted) = issue_test_mandate(MandateCaveats {
            currency: Some("USD".to_string()),
            max_total: Some(Money::new("USD", "0.50")),
            effect: Some(Effect::Write),
            ..Default::default()
        });

        let mut catalog = ToolPricingCatalog::new();
        catalog.insert(
            "convexity",
            "create_webhook",
            ToolPriceSpec {
                resource_uri: "https://convexityos.com/api/v1/mcp/create_webhook".to_string(),
                amount: Money::new("USD", "0.30"),
                effect: Effect::Write,
                purpose: None,
            },
        );

        let tracker = Arc::new(MandateSpendTracker::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let bridge = MandatedToolBridge::new(
            "convexity",
            Arc::new(StubTool {
                tool_name: "create_webhook".to_string(),
                calls: calls.clone(),
                fail_mode: StubFailMode::ResultFailure,
            }),
            tokens,
            trusted,
            "https://ledger.convexityos.com",
            Arc::new(catalog),
            tracker.clone(),
        )
        .with_clock(|| 2000);

        let res = bridge.execute(serde_json::json!({}), &test_ctx()).await.unwrap();
        assert!(!res.success);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Spend must STILL be debited (0.30), so a second 0.30 call hits budget_exhausted.
        assert_eq!(tracker.spent_on("mnd_root_mcp"), Some(Money::new("USD", "0.3")));
        let err = bridge.execute(serde_json::json!({}), &test_ctx()).await.unwrap_err();
        assert!(err.contains("budget_exhausted"), "unexpected error: {}", err);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
