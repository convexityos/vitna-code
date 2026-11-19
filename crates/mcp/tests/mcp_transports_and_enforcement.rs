use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex as AsyncMutex;
use vitna_mcp::{
    HttpMcpTransport, McpClient, McpContentItem, McpToolBridge, McpToolCallResult,
    McpToolDescriptor, StdioMcpTransport,
};
use vitna_orchestration::{
    ApprovalDecision, ApprovalRequest, Approver, OrchestrationConfig, OrchestrationEngine,
};
use vitna_runner::FakeRunner;
use vitna_store::EventStore;
use vitna_tools::Tool;

#[tokio::test]
async fn stdio_duplex_initialize_paginated_list_and_call() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (c_read, c_write) = tokio::io::split(client_io);
    let (s_read, mut s_write) = tokio::io::split(server_io);

    let server_task = tokio::spawn(async move {
        let mut reader = BufReader::new(s_read);
        let mut saw_initialized_notification = false;
        let mut line = String::new();
        while reader.read_line(&mut line).await.unwrap() > 0 {
            let msg: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            line.clear();
            let method = msg["method"].as_str().unwrap_or("");
            if msg.get("id").is_none() {
                if method == "notifications/initialized" {
                    saw_initialized_notification = true;
                }
                continue;
            }
            let id = msg["id"].as_u64().unwrap();
            match method {
                "initialize" => {
                    // Send an interleaved server notification first to verify the client skips it.
                    let notif = json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {"data": "booting"}});
                    s_write.write_all(notif.to_string().as_bytes()).await.unwrap();
                    s_write.write_all(b"\n").await.unwrap();
                    let resp = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": { "tools": { "listChanged": false } },
                            "serverInfo": { "name": "test-stdio", "version": "0.1.0" }
                        }
                    });
                    s_write.write_all(resp.to_string().as_bytes()).await.unwrap();
                    s_write.write_all(b"\n").await.unwrap();
                    s_write.flush().await.unwrap();
                }
                "tools/list" => {
                    let cursor = msg
                        .get("params")
                        .and_then(|p| p.get("cursor"))
                        .and_then(|c| c.as_str());
                    let result = match cursor {
                        None => json!({
                            "tools": [{
                                "name": "alpha",
                                "description": "first page tool",
                                "inputSchema": { "type": "object" }
                            }],
                            "nextCursor": "page-2"
                        }),
                        Some("page-2") => json!({
                            "tools": [{
                                "name": "beta",
                                "description": "second page tool",
                                "inputSchema": { "type": "object" }
                            }]
                        }),
                        other => panic!("unexpected cursor: {other:?}"),
                    };
                    let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
                    s_write.write_all(resp.to_string().as_bytes()).await.unwrap();
                    s_write.write_all(b"\n").await.unwrap();
                    s_write.flush().await.unwrap();
                }
                "tools/call" => {
                    let name = msg["params"]["name"].as_str().unwrap();
                    let val = msg["params"]["arguments"]["x"].as_i64().unwrap_or(0);
                    let resp = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": format!("{name}:{}", val + 1) }],
                            "isError": false
                        }
                    });
                    s_write.write_all(resp.to_string().as_bytes()).await.unwrap();
                    s_write.write_all(b"\n").await.unwrap();
                    s_write.flush().await.unwrap();
                }
                other => panic!("unexpected method {other}"),
            }
        }
        saw_initialized_notification
    });

    let transport = StdioMcpTransport::from_streams(c_read, c_write, Duration::from_secs(5));
    let mut client = McpClient::with_transport("test-stdio", Box::new(transport));

    let init = client.initialize().await.expect("initialize");
    assert_eq!(init["serverInfo"]["name"], "test-stdio");
    assert!(client.is_initialized());
    assert!(client.server_capabilities().is_some());

    let tools = client.list_tools().await.expect("paginated tools/list");
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["alpha", "beta"]);

    let res = client
        .call_tool("beta", json!({ "x": 41 }))
        .await
        .expect("tools/call");
    assert_eq!(res.is_error, Some(false));
    assert_eq!(res.content[0].text.as_deref(), Some("beta:42"));

    drop(client);
    assert!(
        server_task.await.unwrap(),
        "client must send notifications/initialized after initialize"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn stdio_spawned_child_process_clears_env_and_answers() {
    // Real POSIX child process reading newline-delimited JSON-RPC over stdin/stdout.
    // Prints the value of ALLOWED_KEY and AMBIENT_SECRET in the tool output so we
    // prove env_clear() strips ambient secrets while preserving explicit allowlists.
    let script = r#"
while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"sh-mcp","version":"1.0"}}}\n'
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"env_echo","description":"checks env","inputSchema":{"type":"object"}}]}}\n'
      ;;
    *'"method":"tools/call"'*)
      printf '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"allowed=%s;path_set=%s"}],"isError":false}}\n' "${ALLOWED_KEY:-unset}" "${HOME:+yes}"
      ;;
  esac
done
"#;

    let mut client = McpClient::connect_stdio(
        "sh-mcp",
        "/bin/sh",
        &["-c".to_string(), script.to_string()],
        &[("ALLOWED_KEY".to_string(), "ok-value".to_string())],
    )
    .expect("spawn stdio child");

    let init = client.initialize().await.expect("child initialize");
    assert_eq!(init["serverInfo"]["name"], "sh-mcp");

    let tools = client.list_tools().await.expect("child list_tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "env_echo");

    let res = client
        .call_tool("env_echo", json!({}))
        .await
        .expect("child call_tool");
    let text = res.content[0].text.as_deref().unwrap_or("");
    assert_eq!(
        text, "allowed=ok-value;path_set=",
        "env_clear must strip inherited HOME while preserving explicit ALLOWED_KEY"
    );
}

#[tokio::test]
async fn http_transport_json_and_sse_with_session_header() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let addr = listener.local_addr().unwrap();

    let server_task = tokio::spawn(async move {
        let mut saw_session_on_followup = false;
        for _ in 0..4 {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let header_end = loop {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&buf[..n]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(pos + 4);
                }
            };
            let Some(body_start) = header_end else { continue };
            let headers = String::from_utf8_lossy(&raw[..body_start]).to_string();
            let content_len: usize = headers
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    if k.eq_ignore_ascii_case("content-length") {
                        v.trim().parse().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            while raw.len() < body_start + content_len {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&buf[..n]);
            }
            let body = &raw[body_start..body_start + content_len];
            let req: serde_json::Value = serde_json::from_slice(body).unwrap();
            let method = req["method"].as_str().unwrap_or("");

            if headers.to_ascii_lowercase().contains("mcp-session-id: sess-http-1") {
                saw_session_on_followup = true;
            }

            let response = match method {
                "initialize" => {
                    let payload = json!({
                        "jsonrpc": "2.0",
                        "id": req["id"],
                        "result": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "http-mcp", "version": "1.0" }
                        }
                    })
                    .to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: sess-http-1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    )
                }
                "notifications/initialized" => {
                    "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                }
                "tools/list" => {
                    // Return Streamable HTTP SSE format with a notification before the response.
                    let resp_line = json!({
                        "jsonrpc": "2.0",
                        "id": req["id"],
                        "result": {
                            "tools": [{
                                "name": "remote_ping",
                                "description": "pings over http",
                                "inputSchema": { "type": "object" }
                            }]
                        }
                    })
                    .to_string();
                    let sse = format!(
                        "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\n\nevent: message\ndata: {resp_line}\n\n"
                    );
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        sse.len(),
                        sse
                    )
                }
                "tools/call" => {
                    let payload = json!({
                        "jsonrpc": "2.0",
                        "id": req["id"],
                        "result": {
                            "content": [{ "type": "text", "text": "pong-http" }],
                            "isError": false
                        }
                    })
                    .to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        payload.len(),
                        payload
                    )
                }
                other => panic!("unexpected HTTP MCP method {other}"),
            };
            sock.write_all(response.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        }
        saw_session_on_followup
    });

    let endpoint = format!("http://{addr}/mcp");
    let mut client = McpClient::connect_http("http-mcp", endpoint).expect("connect_http");
    let init = client.initialize().await.expect("http initialize");
    assert_eq!(init["serverInfo"]["name"], "http-mcp");

    let tools = client.list_tools().await.expect("http sse tools/list");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "remote_ping");

    let res = client
        .call_tool("remote_ping", json!({}))
        .await
        .expect("http call_tool");
    assert_eq!(res.content[0].text.as_deref(), Some("pong-http"));

    assert!(
        server_task.await.unwrap(),
        "HttpMcpTransport must echo Mcp-Session-Id on follow-up requests"
    );
}

#[tokio::test]
async fn hostile_servers_are_rejected_malformed_id_mismatch_oversized_timeout_and_cursor_loop() {
    // 1. Malformed JSON
    {
        let (c, s) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(c);
        let (sr, mut sw) = tokio::io::split(s);
        tokio::spawn(async move {
            let mut r = BufReader::new(sr);
            let mut line = String::new();
            let _ = r.read_line(&mut line).await;
            let _ = sw.write_all(b"{not valid json\n").await;
        });
        let t = StdioMcpTransport::from_streams(cr, cw, Duration::from_secs(2));
        let mut client = McpClient::with_transport("bad-json", Box::new(t));
        let err = client.initialize().await.unwrap_err();
        assert!(err.contains("malformed JSON"), "got {err}");
    }

    // 2. Mismatched JSON-RPC response ID
    {
        let (c, s) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(c);
        let (sr, mut sw) = tokio::io::split(s);
        tokio::spawn(async move {
            let mut r = BufReader::new(sr);
            let mut line = String::new();
            let _ = r.read_line(&mut line).await;
            let _ = sw
                .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":999,\"result\":{}}\n")
                .await;
        });
        let t = StdioMcpTransport::from_streams(cr, cw, Duration::from_secs(2));
        let mut client = McpClient::with_transport("bad-id", Box::new(t));
        let err = client.initialize().await.unwrap_err();
        assert!(err.contains("id mismatch"), "got {err}");
    }

    // 3. Oversized frame exceeds max_frame_bytes
    {
        let (c, s) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(c);
        let (sr, mut sw) = tokio::io::split(s);
        tokio::spawn(async move {
            let mut r = BufReader::new(sr);
            let mut line = String::new();
            let _ = r.read_line(&mut line).await;
            let huge = "a".repeat(2048);
            let _ = sw.write_all(huge.as_bytes()).await;
            let _ = sw.write_all(b"\n").await;
        });
        let t = StdioMcpTransport::from_streams(cr, cw, Duration::from_secs(2))
            .with_max_frame_bytes(256);
        let mut client = McpClient::with_transport("huge", Box::new(t));
        let err = client.initialize().await.unwrap_err();
        assert!(err.contains("exceeded 256 byte limit"), "got {err}");
    }

    // 4. Hung server times out cleanly
    {
        let (c, s) = tokio::io::duplex(4096);
        let (cr, cw) = tokio::io::split(c);
        let _keep_open = s;
        let t = StdioMcpTransport::from_streams(cr, cw, Duration::from_millis(60));
        let mut client = McpClient::with_transport("hung", Box::new(t));
        let err = client.initialize().await.unwrap_err();
        assert!(err.contains("timed out"), "got {err}");
    }

    // 5. Repeating pagination cursor loop rejected
    {
        let (c, s) = tokio::io::duplex(16 * 1024);
        let (cr, cw) = tokio::io::split(c);
        let (sr, mut sw) = tokio::io::split(s);
        tokio::spawn(async move {
            let mut r = BufReader::new(sr);
            let mut line = String::new();
            while r.read_line(&mut line).await.unwrap_or(0) > 0 {
                let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                line.clear();
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": req["id"],
                    "result": { "tools": [], "nextCursor": "same-forever" }
                });
                let _ = sw.write_all(resp.to_string().as_bytes()).await;
                let _ = sw.write_all(b"\n").await;
                let _ = sw.flush().await;
            }
        });
        let t = StdioMcpTransport::from_streams(cr, cw, Duration::from_secs(2));
        let mut client = McpClient::with_transport("looping", Box::new(t));
        let err = client.list_tools().await.unwrap_err();
        assert!(err.contains("repeated tools/list pagination cursor"), "got {err}");
    }
}

#[test]
fn http_transport_refuses_non_http_scheme() {
    assert!(HttpMcpTransport::new("file:///etc/passwd", Duration::from_secs(1)).is_err());
    assert!(HttpMcpTransport::new("ftp://example.com/mcp", Duration::from_secs(1)).is_err());
}

struct RejectAll;

#[async_trait]
impl Approver for RejectAll {
    fn asks_a_person(&self) -> bool {
        true
    }
    async fn decide(&self, _request: &ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Rejected("operator denied external MCP tool".to_string())
    }
}

#[tokio::test]
async fn unapproved_mcp_tool_is_refused_by_orchestration_engine_and_never_executes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);

    let mut client = McpClient::new("external-db");
    client.register_mock_tool(
        "drop_table",
        "Drops a table",
        json!({ "type": "object", "properties": { "table": { "type": "string" } } }),
        move |_args| {
            calls_clone.fetch_add(1, Ordering::SeqCst);
            McpToolCallResult {
                content: vec![McpContentItem {
                    r#type: "text".to_string(),
                    text: Some("dropped".to_string()),
                }],
                is_error: Some(false),
            }
        },
    );
    let descriptor = client.list_tools().await.unwrap().pop().unwrap();
    let bridge = McpToolBridge::new(
        "external-db",
        descriptor,
        Arc::new(AsyncMutex::new(client)),
        true,
    );

    let tmp = std::env::temp_dir().join(format!(
        "vitna_mcp_enforce_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let runner = Arc::new(StdMutex::new(
        FakeRunner::new(tmp.join("runner.journal")).unwrap(),
    ));
    let store = Arc::new(StdMutex::new(EventStore::open_in_memory().unwrap()));
    let mut engine = OrchestrationEngine::new(
        OrchestrationConfig {
            session_id: "sess-mcp".into(),
            run_id: "run-mcp".into(),
            workspace_root: tmp.clone(),
            auto_approve: false,
            model_sku: "test".into(),
            provider_name: "none".into(),
            sandbox_guarantee: "guarded".into(),
            verification_command: None,
            allow_unsandboxed: false,
        },
        store,
        runner,
        vitna_receipts::generate_signing_key(),
    )
    .with_approver(Arc::new(RejectAll));

    engine.tool_registry.register(Arc::new(bridge));

    let result = engine
        .execute_tool("drop_table", json!({ "table": "users" }), Some("call-mcp-1"))
        .await;
    assert!(result.is_err(), "unapproved MCP tool must be refused");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "refused MCP tool handler must NEVER execute"
    );
}

#[test]
fn cross_server_action_digests_are_isolated_and_cannot_collide() {
    let desc_c = McpToolDescriptor {
        name: "c".into(),
        description: None,
        input_schema: json!({}),
    };
    let desc_bc = McpToolDescriptor {
        name: "b:c".into(),
        description: None,
        input_schema: json!({}),
    };
    let c1 = Arc::new(AsyncMutex::new(McpClient::new("a:b")));
    let c2 = Arc::new(AsyncMutex::new(McpClient::new("a")));
    let c3 = Arc::new(AsyncMutex::new(McpClient::new("other")));

    let b1 = McpToolBridge::new("a:b", desc_c.clone(), c1, true);
    let b2 = McpToolBridge::new("a", desc_bc, c2, true);
    let b3 = McpToolBridge::new("other", desc_c, c3, true);

    let args = json!({ "x": 1 });
    assert_ne!(
        b1.compute_action_digest(&args),
        b2.compute_action_digest(&args),
        "colon-spliced server/tool names must never collide"
    );
    assert_ne!(
        b1.compute_action_digest(&args),
        b3.compute_action_digest(&args),
        "same tool name on two servers must produce distinct action digests"
    );
}
