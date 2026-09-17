use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use vitna_mcp::{McpClient, McpContentItem, McpToolBridge, McpToolCallResult};
use vitna_runner::FakeRunner;
use vitna_tools::{Tool, ToolContext};

#[tokio::test]
async fn test_mcp_client_handshake_and_tool_brokering() {
    let mut client = McpClient::new("postgres-mcp-server");

    // Register a mock MCP tool on the server
    client.register_mock_tool(
        "query_sql",
        "Execute read-only SQL query against database",
        json!({
            "type": "object",
            "properties": {
                "sql": { "type": "string" }
            },
            "required": ["sql"]
        }),
        |args| {
            let query = args.get("sql").and_then(|s| s.as_str()).unwrap_or_default();
            McpToolCallResult {
                content: vec![McpContentItem {
                    r#type: "text".to_string(),
                    text: Some(format!("QueryResult: 3 rows for query '{}'", query)),
                }],
                is_error: Some(false),
            }
        },
    );

    // 1. Initialize
    let init_res = client.initialize().await.expect("initialize succeeds");
    assert_eq!(init_res["serverInfo"]["name"], "postgres-mcp-server");

    // 2. Discover tools
    let tools = client.list_tools().await.expect("list tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "query_sql");

    let descriptor = tools[0].clone();
    let client_shared = Arc::new(Mutex::new(client));

    // 3. Wrap in Vitna McpToolBridge
    let bridge = McpToolBridge::new("postgres-mcp-server", descriptor, client_shared, true);

    // 4. Verify Vitna tool definition
    let def = bridge.definition();
    assert_eq!(def.name, "query_sql");
    assert!(def.description.contains("[MCP: postgres-mcp-server]"));
    assert!(def.requires_approval, "External MCP tools must strictly require approval");

    // 5. Compute exact-action digest
    let call_args = json!({ "sql": "SELECT id, name FROM users LIMIT 10;" });
    let digest = bridge.compute_action_digest(&call_args);
    assert_eq!(digest.len(), 64);

    // 6. Execute brokered tool
    let temp_dir = std::env::temp_dir();
    let journal_path = temp_dir.join(format!("mcp_test_{}.journal", std::process::id()));
    let fake_runner = FakeRunner::new(&journal_path).expect("runner");
    let runner = Arc::new(std::sync::Mutex::new(fake_runner));

    let ctx = ToolContext {
        workspace_root: PathBuf::from("."),
        runner,
    };

    let result = bridge.execute(call_args, &ctx).await.expect("execute succeeds");
    assert!(result.success);
    assert!(result.output.contains("QueryResult: 3 rows"));

    let _ = std::fs::remove_file(journal_path);
}
