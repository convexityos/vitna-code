//! Live MCP server attachment for the daemon.
//!
//! External MCP servers (Streamable HTTP or local stdio) are initialized once
//! when attached, their tools are discovered via paginated `tools/list`, and
//! each tool is bridged under a collision-free `mcp__<server>__<tool>` name
//! that strictly requires exact-action approval on every call.

use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use vitna_mcp::{discover_and_bridge_tools, McpClient};
use vitna_tools::Tool;

use crate::DaemonServer;

/// Declarative specification for attaching an external MCP server to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerSpec {
    Http {
        name: String,
        url: String,
    },
    Stdio {
        name: String,
        program: String,
        args: Vec<String>,
    },
}

impl McpServerSpec {
    /// Parses a CLI `--mcp-http` value of the form `<name>=<url>`.
    pub fn parse_http(spec: &str) -> Result<Self, String> {
        let Some((name, url)) = spec.split_once('=') else {
            return Err(format!(
                "invalid --mcp-http spec '{spec}': expected '<name>=<url>'"
            ));
        };
        let name = name.trim();
        let url = url.trim();
        if name.is_empty() || url.is_empty() {
            return Err(format!(
                "invalid --mcp-http spec '{spec}': both name and url must be non-empty"
            ));
        }
        Ok(Self::Http {
            name: name.to_string(),
            url: url.to_string(),
        })
    }

    /// Parses a CLI `--mcp-stdio` value of the form `<name>=<program>[ <arg>...]`
    /// using simple whitespace splitting (no shell evaluation).
    pub fn parse_stdio(spec: &str) -> Result<Self, String> {
        let Some((name, cmd)) = spec.split_once('=') else {
            return Err(format!(
                "invalid --mcp-stdio spec '{spec}': expected '<name>=<program> [args...]'"
            ));
        };
        let name = name.trim();
        let mut parts = cmd.split_whitespace();
        let Some(program) = parts.next() else {
            return Err(format!(
                "invalid --mcp-stdio spec '{spec}': command program is empty"
            ));
        };
        if name.is_empty() {
            return Err(format!(
                "invalid --mcp-stdio spec '{spec}': server name is empty"
            ));
        }
        let args = parts.map(str::to_string).collect();
        Ok(Self::Stdio {
            name: name.to_string(),
            program: program.to_string(),
            args,
        })
    }

    pub fn name(&self) -> &str {
        match self {
            McpServerSpec::Http { name, .. } | McpServerSpec::Stdio { name, .. } => name,
        }
    }
}

impl DaemonServer {
    /// Connects to an external MCP server, runs the MCP initialize handshake,
    /// discovers all paginated tools, and attaches their brokered tool bridges
    /// to every subsequent turn served by this daemon.
    pub async fn attach_mcp_server(&self, spec: &McpServerSpec) -> Result<Vec<String>, String> {
        let client = match spec {
            McpServerSpec::Http { name, url } => McpClient::connect_http(name, url)?,
            McpServerSpec::Stdio {
                name,
                program,
                args,
            } => McpClient::connect_stdio(name, program, args, &[])?,
        };
        self.attach_mcp_client(spec.name(), Arc::new(AsyncMutex::new(client)))
            .await
    }

    /// Attaches a pre-constructed [`McpClient`] (live transport or in-memory mock),
    /// discovering and bridging all of its tools under `mcp__<server>__<tool>`.
    pub async fn attach_mcp_client(
        &self,
        server_name: &str,
        client: Arc<AsyncMutex<McpClient>>,
    ) -> Result<Vec<String>, String> {
        let bridges = discover_and_bridge_tools(server_name, client).await?;
        let mut registered_names = Vec::with_capacity(bridges.len());

        let mut guard = self.mcp_tools.lock().map_err(|e| e.to_string())?;
        for bridge in bridges {
            let tool: Arc<dyn Tool> = match &self.mandate_session {
                Some(session) => session.wrap_tool(server_name, bridge),
                None => bridge,
            };
            let name = tool.name().to_string();
            if guard.iter().any(|existing| existing.name() == name) {
                return Err(format!(
                    "MCP tool '{}' is already attached to this daemon",
                    name
                ));
            }
            registered_names.push(name);
            guard.push(tool);
        }

        Ok(registered_names)
    }

    /// Returns a snapshot of all currently attached external MCP tool bridges.
    pub fn mcp_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.mcp_tools
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mcp_specs_validates_syntax() {
        assert_eq!(
            McpServerSpec::parse_http("docs=http://127.0.0.1:9000/mcp").unwrap(),
            McpServerSpec::Http {
                name: "docs".to_string(),
                url: "http://127.0.0.1:9000/mcp".to_string(),
            }
        );
        assert_eq!(
            McpServerSpec::parse_stdio("fs=my-mcp-server --readonly").unwrap(),
            McpServerSpec::Stdio {
                name: "fs".to_string(),
                program: "my-mcp-server".to_string(),
                args: vec!["--readonly".to_string()],
            }
        );
        assert!(McpServerSpec::parse_http("missing-equals").is_err());
        assert!(McpServerSpec::parse_http("=http://127.0.0.1").is_err());
        assert!(McpServerSpec::parse_stdio("fs=   ").is_err());
    }
}
