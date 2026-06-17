//! Native transports for MCP servers (feature `native`).
//!
//! Two transports share one protocol core ([`crate::proto`]) and one public
//! shape ([`McpClient`]):
//!
//! - [`McpHttpClient`] — remote servers over Streamable HTTP / SSE (reqwest).
//! - [`McpStdioClient`] — local subprocess servers over newline-delimited
//!   JSON-RPC on the child's stdin/stdout (tokio process).

mod http;
mod stdio;

pub use http::McpHttpClient;
pub use stdio::McpStdioClient;

use crate::error::McpError;
use crate::proto::{McpToolDef, ToolOutput};
use serde_json::Value;
use std::time::Duration;

/// Shared client configuration. The `timeout` field applies per transport:
/// HTTP uses it as a connect+read timeout; stdio uses it as a per-request
/// read timeout while waiting for the child's response line.
#[derive(Debug, Clone)]
pub struct McpClientOptions {
    /// Per-request timeout.
    pub timeout: Duration,
    /// Reported in the `initialize` clientInfo. Name your consumer.
    pub client_name: String,
    pub client_version: String,
}

impl Default for McpClientOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            client_name: "greentic-mcp-client".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// Server identity from the `initialize` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
}

impl ServerInfo {
    /// Build [`ServerInfo`] from an `initialize` result object. Missing string
    /// fields default to empty; a missing `serverInfo` object is an error.
    pub(crate) fn from_initialize_result(result: &Value) -> Result<Self, McpError> {
        let server_info = result.get("serverInfo").ok_or_else(|| {
            McpError::BadInitialize("missing serverInfo in initialize result".to_string())
        })?;
        Ok(Self {
            name: server_info
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            version: server_info
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            protocol_version: result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
    }
}

/// The common surface every native transport exposes. Lets callers treat HTTP
/// and stdio sessions uniformly (e.g. `Box<dyn McpClient>`).
///
/// All methods take `&mut self`: a session threads a monotonic request id and,
/// for HTTP, a server-assigned session id, so calls are inherently sequential.
#[allow(async_fn_in_trait)]
pub trait McpClient {
    /// `initialize` + `notifications/initialized` handshake.
    async fn initialize(&mut self) -> Result<ServerInfo, McpError>;
    /// `tools/list` → mapped definitions. Requires a completed `initialize`.
    async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, McpError>;
    /// `tools/call`. Server-side tool failure (`isError`) maps to
    /// [`McpError::ToolCall`]; JSON-RPC failures to [`McpError::Server`].
    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<ToolOutput, McpError>;
}
