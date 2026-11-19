//! Integration tests verifying Invariant 13 (Provider disclosure): every HTTP
//! provider call records its destination host, redacted endpoint, wire bytes
//! sent and received, and latency in the local-only `TelemetryCollector`.

use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vitna_providers::anthropic::AnthropicProvider;
use vitna_providers::credentials::ProviderCredentials;
use vitna_providers::openai::OpenAIProvider;
use vitna_providers::runtime::{RuntimeLane, RuntimeLaneProvider, RUNTIME_PROVIDER_NAME};
use vitna_providers::{Provider, ProviderMessage, ProviderRequest};
use vitna_telemetry::TelemetryCollector;

async fn spawn_mock_http_server(response_body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback mock server");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        let _ = socket.write_all(reply.as_bytes()).await;
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn openai_anthropic_and_runtime_record_exact_egress_and_scrub_secrets() {
    let collector = Arc::new(TelemetryCollector::new());

    // 1. OpenAIProvider against loopback mock with a secret query param + Bearer key.
    let openai_reply = serde_json::json!({
        "model": "gpt-4o-mini",
        "choices": [{
            "message": { "role": "assistant", "content": "openai ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 2 }
    })
    .to_string();
    let openai_origin = spawn_mock_http_server(openai_reply.clone()).await;
    let openai_provider = OpenAIProvider::new(ProviderCredentials {
        provider: "openai".to_string(),
        api_key: "sk-1234567890abcdef123456".to_string(),
        base_url: Some(format!(
            "{openai_origin}/v1/chat/completions?api_key=sk-proj-secretquery12345"
        )),
    })
    .with_telemetry(Arc::clone(&collector));

    let req = ProviderRequest {
        model: "gpt-4o-mini".to_string(),
        messages: vec![ProviderMessage::user("hello")],
        tools: Vec::new(),
        system_prompt: None,
        max_tokens: Some(32),
        temperature: None,
    };
    let resp = openai_provider.complete(&req).await.expect("openai complete");
    assert_eq!(resp.content, "openai ok");

    // 2. AnthropicProvider against loopback mock.
    let anthropic_reply = serde_json::json!({
        "model": "claude-3-7-sonnet",
        "content": [{ "type": "text", "text": "anthropic ok" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 6, "output_tokens": 3 }
    })
    .to_string();
    let anthropic_origin = spawn_mock_http_server(anthropic_reply.clone()).await;
    let anthropic_provider = AnthropicProvider::new(ProviderCredentials {
        provider: "anthropic".to_string(),
        api_key: "sk-ant-api03-secretvalue123456".to_string(),
        base_url: Some(format!("{anthropic_origin}/v1/messages")),
    })
    .with_telemetry(Arc::clone(&collector));

    let resp = anthropic_provider
        .complete(&req)
        .await
        .expect("anthropic complete");
    assert_eq!(resp.content, "anthropic ok");

    // 3. RuntimeLaneProvider against loopback mock.
    let runtime_reply = serde_json::json!({
        "model": "vitna/code",
        "choices": [{
            "message": { "role": "assistant", "content": "runtime ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
    })
    .to_string();
    let runtime_origin = spawn_mock_http_server(runtime_reply.clone()).await;
    let lane = RuntimeLane::new(&runtime_origin, "vitna/code").expect("runtime lane");
    let runtime_provider =
        RuntimeLaneProvider::new(lane).with_telemetry(Arc::clone(&collector));

    let resp = runtime_provider
        .complete(&req)
        .await
        .expect("runtime complete");
    assert_eq!(resp.content, "runtime ok");

    // Verify all 3 egress records were captured with exact providers, loopback host, and byte sizes.
    let records = collector.egress_records();
    assert_eq!(records.len(), 3);

    assert_eq!(records[0].provider, "openai");
    assert_eq!(records[0].host, "127.0.0.1");
    assert!(records[0].bytes_sent > 0);
    assert_eq!(records[0].bytes_received, openai_reply.len() as u64);

    assert_eq!(records[1].provider, "anthropic");
    assert_eq!(records[1].host, "127.0.0.1");
    assert!(records[1].bytes_sent > 0);
    assert_eq!(records[1].bytes_received, anthropic_reply.len() as u64);

    assert_eq!(records[2].provider, RUNTIME_PROVIDER_NAME);
    assert_eq!(records[2].host, "127.0.0.1");
    assert!(records[2].bytes_sent > 0);
    assert_eq!(records[2].bytes_received, runtime_reply.len() as u64);

    // Latency metrics captured for every call.
    let metrics = collector.metrics();
    assert_eq!(metrics.len(), 3);
    assert!(metrics.iter().all(|m| m.name == "provider_http_latency_ms"));
    assert!(metrics
        .iter()
        .all(|m| m.labels.get("status").map(String::as_str) == Some("200")));

    // Exported redacted JSON must never contain any raw secret tokens.
    let exported = collector.export_redacted_json().expect("export redacted json");
    for (idx, needle) in [
        "sk-1234567890abcdef123456",
        "sk-proj-secretquery12345",
        "sk-ant-api03-secretvalue123456",
    ]
    .iter()
    .enumerate()
    {
        assert!(
            !exported.contains(needle),
            "redacted export leaked test fixture token #{idx}"
        );
    }
    assert!(exported.contains("[REDACTED_SECRET]"));
}
