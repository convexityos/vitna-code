use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vitna_providers::{
    compute_checkpoint_digest, AnchorConfig, Provider, ProviderMessage, ProviderRequest,
    ProviderStreamEvent, VitnaAnchorProvider,
};

struct MockRoute {
    health_body: String,
    models_body: String,
    chat_body: String,
}

async fn spawn_mock_anchor(routes: MockRoute) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let routes = Arc::new(routes);

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let routes = routes.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let n = stream.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or("");
                let path = parts.next().unwrap_or("");

                let (status, body) = match (method, path) {
                    ("GET", "/health") | ("GET", "/v1/health") => (200, routes.health_body.as_str()),
                    ("GET", "/v1/models") => (200, routes.models_body.as_str()),
                    ("POST", "/v1/chat/completions") => (200, routes.chat_body.as_str()),
                    _ => (404, r#"{"error":{"message":"not found"}}"#),
                };

                let resp = format!(
                    "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });

    format!("http://{}", addr)
}

fn temp_test_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("vitna_anchor_prov_{}_{}", tag, std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn loopback_guard_refuses_remote_endpoints_by_default() {
    let err = VitnaAnchorProvider::new(
        AnchorConfig::local_default().with_base_url("https://untrusted.example.com:8765"),
    )
    .err()
    .expect("remote endpoint must be refused by default");
    assert!(err.contains("loopback guard"), "unexpected error: {}", err);

    let ok = VitnaAnchorProvider::new(
        AnchorConfig::local_default()
            .with_base_url("https://gpu-box.internal:8765")
            .allow_remote(),
    );
    assert!(ok.is_ok(), "explicit allow_remote() should succeed");
}

#[tokio::test]
async fn discovers_model_completes_tool_calls_and_attests_local_checkpoint() {
    let dir = temp_test_dir("att");
    let shard1 = dir.join("model-00001-of-00002.gguf");
    let shard2 = dir.join("model-00002-of-00002.gguf");
    fs::write(&shard1, b"GGUF-shard-one-bytes").unwrap();
    fs::write(&shard2, b"GGUF-shard-two-bytes").unwrap();

    let base_url = spawn_mock_anchor(MockRoute {
        health_body: r#"{"ok":true,"engine":"vitna-anchor","model":"qwen3-coder-30b-a3b","generation":true,"context":32768,"parallel":4,"chat_template":"qwen3"}"#.to_string(),
        models_body: r#"{"object":"list","data":[{"id":"qwen3-coder-30b-a3b","object":"model","created":1760000000,"owned_by":"vitna-anchor"}]}"#.to_string(),
        chat_body: r#"{
            "id":"chatcmpl-local-1",
            "object":"chat.completion",
            "model":"qwen3-coder-30b-a3b",
            "choices":[{
                "index":0,
                "message":{
                    "role":"assistant",
                    "content":"Inspecting Cargo.toml.",
                    "tool_calls":[{
                        "id":"call_anchor_01",
                        "type":"function",
                        "function":{
                            "name":"read_file",
                            "arguments":"{\"path\":\"Cargo.toml\"}"
                        }
                    }]
                },
                "finish_reason":"tool_calls"
            }],
            "usage":{"prompt_tokens":42,"completion_tokens":19}
        }"#.to_string(),
    })
    .await;

    let provider = VitnaAnchorProvider::new(
        AnchorConfig::local_default()
            .with_base_url(&base_url)
            .with_checkpoint_path(&dir),
    )
    .expect("construct local provider");

    assert_eq!(provider.name(), "vitna-anchor");

    // 1. Health + model discovery
    let health = provider.discover_generation_model().await.expect("health ok");
    assert_eq!(health.model.as_deref(), Some("qwen3-coder-30b-a3b"));
    assert_eq!(health.context, Some(32768));
    assert_eq!(health.chat_template.as_deref(), Some("qwen3"));

    let models = provider.list_models().await.expect("list models");
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "qwen3-coder-30b-a3b");
    assert_eq!(models[0].owned_by, "vitna-anchor");

    // 2. Local checkpoint attestation + mutation detection
    let att1 = provider.build_attestation().await.expect("build attestation");
    assert_eq!(att1.engine, "vitna-anchor");
    assert_eq!(att1.model_id, "qwen3-coder-30b-a3b");
    assert!(att1.checkpoint_sha256.as_ref().is_some_and(|h| h.len() == 64));
    let digest1 = att1.attestation_digest();
    assert_eq!(digest1.len(), 64);

    // Mutating shard2 must change both checkpoint_sha256 and attestation_digest.
    fs::write(&shard2, b"GGUF-shard-two-tampered").unwrap();
    let att2 = provider.build_attestation().await.expect("rebuild attestation");
    assert_ne!(att1.checkpoint_sha256, att2.checkpoint_sha256);
    assert_ne!(digest1, att2.attestation_digest());

    // Single-file GGUF hashing also produces a 64-char hex SHA-256 digest.
    let single_hash = compute_checkpoint_digest(&shard1).expect("single file hash");
    assert_eq!(single_hash.len(), 64);

    // 3. Tool-calling completion
    let response = provider
        .complete(&ProviderRequest {
            model: "qwen3-coder-30b-a3b".to_string(),
            system_prompt: Some("You are a local coding agent.".to_string()),
            messages: vec![ProviderMessage::user("Check Cargo.toml")],
            tools: vec![json!({
                "name": "read_file",
                "description": "Read a workspace file",
                "parameters_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
            })],
            temperature: Some(0.0),
            max_tokens: Some(256),
        })
        .await
        .expect("completion succeeds");

    assert_eq!(response.content, "Inspecting Cargo.toml.");
    assert_eq!(response.finish_reason, "tool_calls");
    assert_eq!(response.prompt_tokens, 42);
    assert_eq!(response.completion_tokens, 19);
    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].id, "call_anchor_01");
    assert_eq!(response.tool_calls[0].name, "read_file");
    assert_eq!(response.tool_calls[0].arguments["path"], "Cargo.toml");

    // 4. SSE stream parsing
    let events = provider
        .parse_sse_event(
            "",
            r#"{"choices":[{"delta":{"content":"Local token"}}],"usage":{"prompt_tokens":10,"completion_tokens":3}}"#,
        )
        .expect("parse SSE");
    assert_eq!(
        events,
        vec![
            ProviderStreamEvent::TextDelta("Local token".to_string()),
            ProviderStreamEvent::Usage {
                prompt_tokens: 10,
                completion_tokens: 3,
            },
        ]
    );

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn fails_closed_when_anchor_serves_embeddings_or_no_model() {
    let base_url = spawn_mock_anchor(MockRoute {
        health_body: r#"{"ok":true,"engine":"vitna-anchor","model":"nomic-embed","generation":false,"embeddings":true}"#.to_string(),
        models_body: r#"{"object":"list","data":[]}"#.to_string(),
        chat_body: r#"{}"#.to_string(),
    })
    .await;

    let provider = VitnaAnchorProvider::new(AnchorConfig::local_default().with_base_url(&base_url))
        .expect("construct provider");

    let err = provider
        .discover_generation_model()
        .await
        .expect_err("embedding-only server must fail closed for coding completions");
    assert!(err.contains("cannot serve coding completions"), "unexpected error: {}", err);
}

#[tokio::test]
async fn adopts_server_checkpoint_sha256_and_fails_closed_on_mismatch() {
    let dir = temp_test_dir("mismatch");
    let ckpt = dir.join("tiny.gguf");
    fs::write(&ckpt, b"expected-gguf-payload").unwrap();
    let expected_hex = compute_checkpoint_digest(&ckpt).unwrap();

    let base_url = spawn_mock_anchor(MockRoute {
        health_body: format!(
            r#"{{"ok":true,"engine":"vitna-anchor","model":"tiny","generation":true,"context":4096,"checkpoint_sha256":"{}"}}"#,
            expected_hex
        ),
        models_body: r#"{"object":"list","data":[]}"#.to_string(),
        chat_body: r#"{}"#.to_string(),
    })
    .await;

    // Without local checkpoint_path, server-reported checkpoint_sha256 is adopted.
    let remote_digest_provider =
        VitnaAnchorProvider::new(AnchorConfig::local_default().with_base_url(&base_url)).unwrap();
    let att = remote_digest_provider.build_attestation().await.unwrap();
    assert_eq!(att.checkpoint_sha256.as_deref(), Some(expected_hex.as_str()));

    // With matching local checkpoint_path, attestation succeeds.
    let matching_provider = VitnaAnchorProvider::new(
        AnchorConfig::local_default()
            .with_base_url(&base_url)
            .with_checkpoint_path(&ckpt),
    )
    .unwrap();
    let att_match = matching_provider.build_attestation().await.unwrap();
    assert_eq!(att_match.checkpoint_sha256.as_deref(), Some(expected_hex.as_str()));

    // Mutate local file -> local hash != server-advertised hash -> fail closed.
    fs::write(&ckpt, b"tampered-gguf-payload").unwrap();
    let err = matching_provider
        .build_attestation()
        .await
        .expect_err("checkpoint mismatch must fail closed");
    assert!(err.contains("checkpoint_sha256 mismatch"), "unexpected error: {}", err);

    let _ = fs::remove_dir_all(&dir);
}
