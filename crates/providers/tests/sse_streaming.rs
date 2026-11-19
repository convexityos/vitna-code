//! Loopback integration tests for token-level SSE streaming across
//! `OpenAIProvider`, `AnthropicProvider`, and `RuntimeLaneProvider`.

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use vitna_providers::anthropic::AnthropicProvider;
use vitna_providers::credentials::ProviderCredentials;
use vitna_providers::openai::OpenAIProvider;
use vitna_providers::runtime::{RuntimeLane, RuntimeLaneProvider, RUNTIME_PROVIDER_NAME};
use vitna_providers::{Provider, ProviderMessage, ProviderRequest};
use vitna_telemetry::TelemetryCollector;

async fn spawn_sse_server(chunks: Vec<String>) -> (String, u64) {
    let total_wire_bytes: u64 = chunks.iter().map(|c| c.len() as u64).sum();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback sse server");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n";
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.flush().await;

        for chunk in chunks {
            let _ = socket.write_all(chunk.as_bytes()).await;
            let _ = socket.flush().await;
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        let _ = socket.shutdown().await;
    });

    (format!("http://{addr}"), total_wire_bytes)
}

async fn spawn_json_server(body: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback json server");
    let addr = listener.local_addr().expect("local addr");

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut buf = vec![0u8; 8192];
        let _ = socket.read(&mut buf).await;

        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = socket.write_all(reply.as_bytes()).await;
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}

fn sample_request(model: &str) -> ProviderRequest {
    ProviderRequest {
        model: model.to_string(),
        messages: vec![ProviderMessage::user("inspect Cargo.toml")],
        tools: Vec::new(),
        system_prompt: None,
        max_tokens: Some(64),
        temperature: None,
    }
}

#[tokio::test]
async fn openai_and_runtime_stream_sse_deltas_tool_calls_and_egress() {
    let sse_chunks = vec![
        "data: {\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"Checked \"}}]}\n\n"
            .to_string(),
        "data: {\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"Cargo.toml \"}}]}\n\n"
            .to_string(),
        "data: {\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"content\":\"successfully.\"}}]}\n\n"
            .to_string(),
        "data: {\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n"
            .to_string(),
        "data: {\"model\":\"gpt-4o-mini\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"Cargo.toml\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n"
            .to_string(),
        "data: {\"model\":\"gpt-4o-mini\",\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":17}}\n\n"
            .to_string(),
        "data: [DONE]\n\n".to_string(),
    ];

    let collector = Arc::new(TelemetryCollector::new());
    let (origin, expected_wire_bytes) = spawn_sse_server(sse_chunks.clone()).await;
    let openai = OpenAIProvider::new(ProviderCredentials {
        provider: "openai".to_string(),
        api_key: "sk-test-streaming-12345".to_string(),
        base_url: Some(format!("{origin}/v1/chat/completions")),
    })
    .with_telemetry(Arc::clone(&collector));

    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let recv_clone = Arc::clone(&received);
    let resp = openai
        .complete_streaming(&sample_request("gpt-4o-mini"), &move |delta| {
            recv_clone.lock().unwrap().push(delta);
        })
        .await
        .expect("openai streaming");

    assert_eq!(
        *received.lock().unwrap(),
        vec!["Checked ", "Cargo.toml ", "successfully."]
    );
    assert_eq!(resp.content, "Checked Cargo.toml successfully.");
    assert_eq!(resp.model.as_deref(), Some("gpt-4o-mini"));
    assert_eq!(resp.finish_reason, "tool_calls");
    assert_eq!(resp.prompt_tokens, 42);
    assert_eq!(resp.completion_tokens, 17);
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "call_1");
    assert_eq!(resp.tool_calls[0].name, "read_file");
    assert_eq!(
        resp.tool_calls[0].arguments,
        serde_json::json!({ "path": "Cargo.toml" })
    );

    // Same SSE stream routed through RuntimeLaneProvider.
    let (runtime_origin, runtime_wire_bytes) = spawn_sse_server(sse_chunks).await;
    let lane = RuntimeLane::new(&runtime_origin, "vitna/code").expect("runtime lane");
    let runtime = RuntimeLaneProvider::new(lane).with_telemetry(Arc::clone(&collector));
    let rt_received = Arc::new(Mutex::new(Vec::<String>::new()));
    let rt_clone = Arc::clone(&rt_received);
    let rt_resp = runtime
        .complete_streaming(&sample_request("vitna/code"), &move |delta| {
            rt_clone.lock().unwrap().push(delta);
        })
        .await
        .expect("runtime streaming");

    assert_eq!(
        *rt_received.lock().unwrap(),
        vec!["Checked ", "Cargo.toml ", "successfully."]
    );
    assert_eq!(rt_resp.finish_reason, "tool_calls");
    assert_eq!(rt_resp.tool_calls.len(), 1);

    let records = collector.egress_records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].provider, "openai");
    assert_eq!(records[0].bytes_received, expected_wire_bytes);
    assert_eq!(records[1].provider, RUNTIME_PROVIDER_NAME);
    assert_eq!(records[1].bytes_received, runtime_wire_bytes);
}

#[tokio::test]
async fn anthropic_streams_sse_deltas_tool_calls_and_usage() {
    let sse_chunks = vec![
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-3-7-sonnet\",\"content\":[],\"usage\":{\"input_tokens\":33,\"output_tokens\":1}}}\n\n"
            .to_string(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Inspecting \"}}\n\n"
            .to_string(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"the \"}}\n\n"
            .to_string(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"workspace.\"}}\n\n"
            .to_string(),
        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_9\",\"name\":\"list_dir\",\"input\":{}}}\n\n"
            .to_string(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\": \"}}\n\n"
            .to_string(),
        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\".\\\"}\"}}\n\n"
            .to_string(),
        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":19}}\n\n"
            .to_string(),
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string(),
    ];

    let collector = Arc::new(TelemetryCollector::new());
    let (origin, expected_wire_bytes) = spawn_sse_server(sse_chunks).await;
    let anthropic = AnthropicProvider::new(ProviderCredentials {
        provider: "anthropic".to_string(),
        api_key: "sk-ant-test-streaming-99999".to_string(),
        base_url: Some(format!("{origin}/v1/messages")),
    })
    .with_telemetry(Arc::clone(&collector));

    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let recv_clone = Arc::clone(&received);
    let resp = anthropic
        .complete_streaming(&sample_request("claude-3-7-sonnet"), &move |delta| {
            recv_clone.lock().unwrap().push(delta);
        })
        .await
        .expect("anthropic streaming");

    assert_eq!(
        *received.lock().unwrap(),
        vec!["Inspecting ", "the ", "workspace."]
    );
    assert_eq!(resp.content, "Inspecting the workspace.");
    assert_eq!(resp.model.as_deref(), Some("claude-3-7-sonnet"));
    assert_eq!(resp.prompt_tokens, 33);
    assert_eq!(resp.completion_tokens, 19);
    assert_eq!(resp.finish_reason, "tool_use");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "toolu_9");
    assert_eq!(resp.tool_calls[0].name, "list_dir");
    assert_eq!(resp.tool_calls[0].arguments, serde_json::json!({ "path": "." }));

    let records = collector.egress_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].provider, "anthropic");
    assert_eq!(records[0].bytes_received, expected_wire_bytes);
}

#[tokio::test]
async fn complete_streaming_falls_back_gracefully_on_json_response() {
    let openai_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "choices": [{
            "message": { "role": "assistant", "content": "json fallback ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 7, "completion_tokens": 3 }
    })
    .to_string();
    let origin = spawn_json_server(openai_body).await;
    let openai = OpenAIProvider::new(ProviderCredentials {
        provider: "openai".to_string(),
        api_key: "sk-test-json-fallback".to_string(),
        base_url: Some(format!("{origin}/v1/chat/completions")),
    });

    let deltas = Arc::new(Mutex::new(Vec::<String>::new()));
    let d_clone = Arc::clone(&deltas);
    let resp = openai
        .complete_streaming(&sample_request("gpt-4o-mini"), &move |d| {
            d_clone.lock().unwrap().push(d);
        })
        .await
        .expect("openai json fallback");

    assert_eq!(resp.content, "json fallback ok");
    assert_eq!(*deltas.lock().unwrap(), vec!["json fallback ok"]);

    let anthropic_body = serde_json::json!({
        "model": "claude-3-7-sonnet",
        "content": [{ "type": "text", "text": "anthropic fallback ok" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 8, "output_tokens": 4 }
    })
    .to_string();
    let ant_origin = spawn_json_server(anthropic_body).await;
    let anthropic = AnthropicProvider::new(ProviderCredentials {
        provider: "anthropic".to_string(),
        api_key: "sk-ant-json-fallback".to_string(),
        base_url: Some(format!("{ant_origin}/v1/messages")),
    });

    let ant_deltas = Arc::new(Mutex::new(Vec::<String>::new()));
    let ad_clone = Arc::clone(&ant_deltas);
    let ant_resp = anthropic
        .complete_streaming(&sample_request("claude-3-7-sonnet"), &move |d| {
            ad_clone.lock().unwrap().push(d);
        })
        .await
        .expect("anthropic json fallback");

    assert_eq!(ant_resp.content, "anthropic fallback ok");
    assert_eq!(*ant_deltas.lock().unwrap(), vec!["anthropic fallback ok"]);
}
