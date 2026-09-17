//! Model Context Protocol (MCP) client, tool discovery, and exact-action capability brokering.

pub mod broker;
pub mod client;
pub mod protocol;

pub use broker::McpToolBridge;
pub use client::McpClient;
pub use protocol::{McpContentItem, McpToolCallResult, McpToolDescriptor};