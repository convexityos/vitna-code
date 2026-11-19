//! Model Context Protocol (MCP) client, tool discovery, and exact-action capability brokering.

pub mod broker;
pub mod client;
pub mod protocol;
pub mod transport;

pub use broker::McpToolBridge;
pub use client::McpClient;
pub use protocol::{
    JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse, McpContentItem,
    McpToolCallResult, McpToolDescriptor, McpToolsListResult,
};
pub use transport::{
    HttpMcpTransport, McpTransport, StdioMcpTransport, DEFAULT_TIMEOUT_MS, MAX_MCP_FRAME_BYTES,
};