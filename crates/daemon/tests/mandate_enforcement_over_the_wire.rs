//! End-to-end daemon test proving live `am/1` Agent Mandate enforcement over
//! attached external MCP tools and cryptographic `mandate_provenance` stamping
//! inside signed `VitnaRunReceiptV1` receipts.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vitna_daemon::launch::Launch;
use vitna_daemon::{DaemonServer, McpServerSpec, ModelRoute};
use vitna_mcp::{
    LiveMandateSession, MandateBundleConfig, ToolCatalogEntry, ToolPriceSpec,
    MANDATE_CONFIG_SCHEMA_V1,
};
use vitna_policy::generate_policy_signing_key;
use vitna_policy::mandate::{
    did_key_from_verifying_key, jwk_thumbprint_from_verifying_key, sign_mandate_jws, CnfClaim,
    Effect, MandateCaveats, MandateClaims, Money, PROTOCOL_VERSION,
};
use vitna_protocol::messages::{ApprovalRequested, RunStateChanged};
use vitna_protocol::type_url;
use vitna_providers::{
    Provider, ProviderRequest, ProviderResponse, ProviderStreamEvent, ProviderToolCall,
};
use vitna_receipt_verify::ReceiptVerifier;
use vitna_receipts::{generate_signing_key, VitnaRunReceiptV1};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "vitna_daemon_mandate_{}_{}_{}",
        tag,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

struct ScriptedProvider {
    replies: Mutex<VecDeque<ProviderResponse>>,
}

impl ScriptedProvider {
    fn new(replies: Vec<ProviderResponse>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
        })
    }
}

#[async_trait]
impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted-mandate"
    }

    fn parse_sse_event(&self, _: &str, _: &str) -> Result<Vec<ProviderStreamEvent>, String> {
        Ok(Vec::new())
    }

    async fn complete(&self, _req: &ProviderRequest) -> Result<ProviderResponse, String> {
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "scripted replies exhausted".to_string())
    }
}

async fn spawn_http_mcp_server(advertised_tools: Vec<Value>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback mcp server");
    let addr = listener.local_addr().expect("local addr");
    let calls_count = Arc::new(AtomicUsize::new(0));
    let count_task = Arc::clone(&calls_count);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let tools = advertised_tools.clone();
            let count = Arc::clone(&count_task);

            tokio::spawn(async move {
                let mut raw = Vec::new();
                let mut buf = [0u8; 4096];
                let header_end;
                let content_length;

                loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        header_end = pos + 4;
                        let head = String::from_utf8_lossy(&raw[..pos]);
                        content_length = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                if k.eq_ignore_ascii_case("content-length") {
                                    v.trim().parse::<usize>().ok()
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(0);
                        break;
                    }
                }
                while raw.len() < header_end + content_length {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                }

                let req: Value = serde_json::from_slice(&raw[header_end..]).unwrap_or(Value::Null);
                let id = req.get("id").cloned().unwrap_or(Value::Null);
                let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");

                let reply = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2025-03-26",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "svc", "version": "1.0.0" }
                        }
                    }),
                    "notifications/initialized" => {
                        let resp = "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                        let _ = socket.write_all(resp.as_bytes()).await;
                        return;
                    }
                    "tools/list" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "tools": tools }
                    }),
                    "tools/call" => {
                        count.fetch_add(1, Ordering::SeqCst);
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": "mandated-mcp-ok" }],
                                "isError": false
                            }
                        })
                    }
                    _ => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "method not found" }
                    }),
                };

                let body = serde_json::to_vec(&reply).unwrap();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            });
        }
    });

    (format!("http://127.0.0.1:{}/mcp", addr.port()), calls_count)
}

fn build_delegated_mandate_bundle(now: u64) -> MandateBundleConfig {
    let root_key = generate_policy_signing_key();
    let root_did = did_key_from_verifying_key(&root_key.verifying_key());
    let agent_key = generate_policy_signing_key();
    let agent_did = did_key_from_verifying_key(&agent_key.verifying_key());
    let agent_jkt = jwk_thumbprint_from_verifying_key(&agent_key.verifying_key());
    let leaf_key = generate_policy_signing_key();
    let leaf_did = did_key_from_verifying_key(&leaf_key.verifying_key());
    let leaf_jkt = jwk_thumbprint_from_verifying_key(&leaf_key.verifying_key());

    let ledger = "https://ledger.convexityos.com".to_string();

    let root_claims = MandateClaims {
        v: PROTOCOL_VERSION.to_string(),
        typ: "mandate".to_string(),
        jti: "mnd_root_live".to_string(),
        iss: root_did.clone(),
        sub: agent_did.clone(),
        par: None,
        iat: now.saturating_sub(60),
        nbf: now.saturating_sub(60),
        exp: now + 3600,
        cnf: CnfClaim { jkt: agent_jkt },
        ledger: ledger.clone(),
        cav: MandateCaveats {
            max_total: Some(Money::new("USD", "0.25")),
            max_per_txn: Some(Money::new("USD", "0.10")),
            aud: Some(vec!["api.example.com".to_string()]),
            resources: Some(vec!["https://api.example.com/*".to_string()]),
            effect: Some(Effect::Write),
            currency: Some("USD".to_string()),
            delegable: Some(true),
            depth: Some(2),
            ..Default::default()
        },
        st: None,
    };
    let root_jws = sign_mandate_jws(&root_claims, &root_key, None);

    let child_claims = MandateClaims {
        v: PROTOCOL_VERSION.to_string(),
        typ: "mandate".to_string(),
        jti: "mnd_child_live".to_string(),
        iss: agent_did,
        sub: leaf_did,
        par: Some("mnd_root_live".to_string()),
        iat: now.saturating_sub(30),
        nbf: now.saturating_sub(30),
        exp: now + 1800,
        cnf: CnfClaim { jkt: leaf_jkt },
        ledger: ledger.clone(),
        cav: MandateCaveats {
            max_total: Some(Money::new("USD", "0.10")),
            max_per_txn: Some(Money::new("USD", "0.06")),
            aud: Some(vec!["api.example.com".to_string()]),
            resources: Some(vec!["https://api.example.com/search".to_string()]),
            effect: Some(Effect::Read),
            currency: Some("USD".to_string()),
            delegable: Some(false),
            depth: Some(0),
            ..Default::default()
        },
        st: None,
    };
    let child_jws = sign_mandate_jws(&child_claims, &agent_key, None);

    MandateBundleConfig {
        schema_version: MANDATE_CONFIG_SCHEMA_V1.to_string(),
        tokens: vec![root_jws, child_jws],
        trusted_root_issuers: vec![root_did],
        ledger,
        currency: "USD".to_string(),
        pricing: vec![ToolCatalogEntry {
            server: "svc".to_string(),
            tool: "priced_read".to_string(),
            spec: ToolPriceSpec {
                resource_uri: "https://api.example.com/search".to_string(),
                amount: Money::new("USD", "0.05"),
                effect: Effect::Read,
                purpose: None,
            },
        }],
        task_id: None,
        step_up_approved: false,
    }
}

fn tool_call_reply(id: &str, name: &str) -> ProviderResponse {
    ProviderResponse {
        tool_calls: vec![ProviderToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments: json!({ "q": "vitna" }),
        }],
        finish_reason: "tool_calls".to_string(),
        prompt_tokens: 10,
        completion_tokens: 5,
        model: Some("scripted-model".to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn live_mandate_enforces_pricing_and_budget_and_stamps_signed_receipt_provenance() {
    let base = scratch_dir("live");
    let ws = base.join("ws");
    fs::create_dir_all(&ws).unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let bundle = build_delegated_mandate_bundle(now);
    let mandate_path = base.join("mandate.json");
    fs::write(&mandate_path, serde_json::to_string_pretty(&bundle).unwrap()).unwrap();

    // Launch::with_mandate accepts the valid bundle and rejects tampered/missing files.
    let launch = Launch::resolve(Some("e".into()), Some(base.join("a.db")))
        .unwrap()
        .with_mandate(Some(mandate_path.clone()))
        .expect("valid mandate file must load");
    assert_eq!(launch.mandate_file.as_deref(), Some(mandate_path.as_path()));
    assert!(Launch::resolve(Some("e".into()), Some(base.join("a.db")))
        .unwrap()
        .with_mandate(Some(base.join("missing.json")))
        .is_err());

    // Start live loopback MCP server with one priced tool and one unpriced tool.
    let (mcp_url, server_hits) = spawn_http_mcp_server(vec![
        json!({
            "name": "priced_read",
            "description": "Priced read tool",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
        }),
        json!({
            "name": "unpriced_tool",
            "description": "Tool missing from pricing catalog",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
        }),
    ])
    .await;

    let provider = ScriptedProvider::new(vec![
        tool_call_reply("tc-1", "mcp__svc__priced_read"),   // 0.05 USD -> OK
        tool_call_reply("tc-2", "mcp__svc__unpriced_tool"), // unpriced -> refused
        tool_call_reply("tc-3", "mcp__svc__priced_read"),   // 0.10 USD -> OK
        tool_call_reply("tc-4", "mcp__svc__priced_read"),   // 0.15 > 0.10 -> budget exhausted
        ProviderResponse {
            content: "Completed under am/1 mandate budget.".to_string(),
            finish_reason: "stop".to_string(),
            prompt_tokens: 10,
            completion_tokens: 6,
            model: Some("scripted-model".to_string()),
            ..Default::default()
        },
    ]);

    let store = EventStore::open_in_memory().expect("store");
    let runner = Arc::new(Mutex::new(
        FakeRunner::new(base.join("runner.journal")).expect("runner"),
    ));
    let key = generate_signing_key();
    let pub_hex = hex::encode(key.verifying_key().to_bytes());
    let mandate_session = Arc::new(LiveMandateSession::from_file(&mandate_path).expect("session"));

    let daemon = DaemonServer::new(store, runner, key)
        .with_mandate(mandate_session)
        .with_route(ModelRoute {
            provider,
            model: "scripted-model".to_string(),
            routing_reason: "test:mandate".to_string(),
        });

    let attached = daemon
        .attach_mcp_server(&McpServerSpec::Http {
            name: "svc".to_string(),
            url: mcp_url,
        })
        .await
        .expect("attach mcp server");
    assert_eq!(attached.len(), 2);

    let session = daemon.create_session(&ws).expect("create session");
    let run_id = daemon
        .submit_turn(&session.session_id, "Run mandated MCP calls")
        .expect("submit turn");

    // Auto-approve every exact-action prompt over the daemon event log so
    // the am/1 mandate bridge decides pricing & budget enforcement.
    let mut answered = HashSet::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let final_state = loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for mandated turn to finish"
        );
        let events = daemon
            .store
            .lock()
            .unwrap()
            .get_events(&run_id, 0)
            .expect("get events");
        let mut done = None;
        for ev in events {
            if ev.type_url == type_url::event::APPROVAL_REQUESTED {
                let req: ApprovalRequested =
                    serde_json::from_slice(&ev.encrypted_payload).unwrap();
                if answered.insert(req.approval_id.clone()) {
                    daemon
                        .answer_approval(
                            &req.approval_id,
                            &req.action_digest,
                            vitna_orchestration::ApprovalDecision::Approved,
                        )
                        .expect("approve action");
                }
            } else if ev.type_url == type_url::event::RUN_STATE_CHANGED {
                let st: RunStateChanged =
                    serde_json::from_slice(&ev.encrypted_payload).unwrap();
                if matches!(st.new_state.as_str(), "completed" | "failed" | "cancelled") {
                    done = Some(st.new_state);
                }
            }
        }
        if let Some(state) = done {
            break state;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert_eq!(final_state, "completed");

    // Only the 2 in-budget priced calls reached the MCP server; unpriced and
    // over-budget calls were stopped fail-closed before touching the server.
    assert_eq!(server_hits.load(Ordering::SeqCst), 2);

    // Inspect and cryptographically verify the signed receipt on disk.
    let receipt_path = ws.join(".vitna").join("receipts").join(format!("{run_id}.json"));
    let raw = fs::read_to_string(&receipt_path).expect("read signed receipt");
    let receipt: VitnaRunReceiptV1 = serde_json::from_str(&raw).expect("parse receipt");

    let mp = receipt
        .mandate_provenance
        .as_ref()
        .expect("signed receipt must embed mandate_provenance");
    assert_eq!(mp.root_jti, "mnd_root_live");
    assert_eq!(mp.path, vec!["mnd_root_live", "mnd_child_live"]);
    assert_eq!(mp.ledger, "https://ledger.convexityos.com");
    assert_eq!(mp.currency, "USD");
    assert_eq!(mp.total_spent, "0.1");
    assert_eq!(mp.spend_digests.len(), 2);
    assert!(mp.chain_hash.starts_with("sha256:"));

    let report = ReceiptVerifier::verify_receipt(&receipt, Some(&pub_hex))
        .expect("verify signed receipt");
    assert!(report.is_valid, "errors: {:?}", report.errors);
    assert!(report.signature_verified);

    let _ = fs::remove_dir_all(&base);
}
