//! An MCP tool's action digest, which binds an approval to a tool served by a
//! server this crate's own definition calls untrusted.
//!
//! Both names in the identity come from that server. The digest used to splice
//! them together with an unescaped `:`, so a server could choose names that
//! made its tool share a digest with a different tool on a different server.

use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use vitna_mcp::{McpClient, McpToolBridge, McpToolDescriptor};
use vitna_tools::Tool;

fn bridge(server: &str, tool: &str) -> McpToolBridge {
    McpToolBridge::new(
        server,
        McpToolDescriptor {
            name: tool.to_string(),
            description: None,
            input_schema: json!({ "type": "object" }),
        },
        Arc::new(Mutex::new(McpClient::new(server))),
        true,
    )
}

#[test]
fn a_separator_in_a_server_or_tool_name_cannot_make_two_tools_one_action() {
    let args = json!({ "q": 1 });
    let first = bridge("a:b", "c").compute_action_digest(&args);
    let second = bridge("a", "b:c").compute_action_digest(&args);

    assert_ne!(
        first, second,
        "tool `c` on server `a:b` and tool `b:c` on server `a` are different \
         tools; an approval for one must not cover the other"
    );
}

#[test]
fn the_same_tool_on_two_servers_is_two_actions() {
    let args = json!({ "q": 1 });
    assert_ne!(
        bridge("server-one", "query").compute_action_digest(&args),
        bridge("server-two", "query").compute_action_digest(&args),
        "approving a tool on one server must not approve its namesake on another"
    );
}

/// A server can name its tool after a built-in one. The built-in identity
/// carries no server, so the two can never share a digest.
#[test]
fn an_mcp_tool_never_shares_a_digest_with_a_builtin_of_the_same_name() {
    let args = json!({ "path": "p", "content": "c" });
    let builtin = vitna_tools::WriteFileTool.compute_action_digest(&args);
    let impostor = bridge("any-server", "write_file").compute_action_digest(&args);

    assert_ne!(
        builtin, impostor,
        "an MCP server naming its tool write_file must not inherit approvals for the real one"
    );
}
