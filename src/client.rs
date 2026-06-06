//! Native (reqwest) transport for remote MCP servers. Feature `native`.

use crate::auth::McpAuth;
use crate::error::{McpError, ProtoError};
use crate::proto::{self, McpToolDef, ToolOutput};
use serde_json::Value;
use std::time::Duration;
use url::Url;

const SESSION_HEADER: &str = "Mcp-Session-Id";

#[derive(Debug, Clone)]
pub struct McpClientOptions {
    /// Per-request timeout (connect + read).
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

/// One logical session against a remote MCP server. Request ids are
/// monotonically increasing; the `Mcp-Session-Id` returned by the server (if
/// any) is replayed on every subsequent request.
pub struct McpHttpClient {
    http: reqwest::Client,
    endpoint: Url,
    auth: Option<McpAuth>,
    opts: McpClientOptions,
    session_id: Option<String>,
    next_id: u64,
}

impl McpHttpClient {
    pub fn new(
        endpoint: Url,
        auth: Option<McpAuth>,
        opts: McpClientOptions,
    ) -> Result<Self, McpError> {
        let http = reqwest::Client::builder().timeout(opts.timeout).build()?;
        Ok(Self {
            http,
            endpoint,
            auth,
            opts,
            session_id: None,
            next_id: 1,
        })
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// POST one JSON-RPC payload. For requests (`expected_id` set) returns
    /// the matching envelope; for notifications returns `None` after the
    /// status check. As a side-effect, any `Mcp-Session-Id` header present in
    /// the response is captured and replayed on all subsequent requests.
    ///
    /// HTTP 4xx/5xx responses surface as [`McpError::Transport`] via
    /// `error_for_status`; any JSON-RPC error body the server attaches to such
    /// a response is **not** parsed.
    async fn post(
        &mut self,
        payload: &Value,
        expected_id: Option<u64>,
    ) -> Result<Option<Value>, McpError> {
        let mut req = self
            .http
            .post(self.endpoint.clone())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(payload.to_string());
        if let Some(auth) = &self.auth {
            let (name, value) = auth.header();
            req = req.header(name, value);
        }
        if let Some(session) = &self.session_id {
            req = req.header(SESSION_HEADER, session.clone());
        }
        let resp = req.send().await?;
        if let Some(session) = resp
            .headers()
            .get(SESSION_HEADER)
            .and_then(|v| v.to_str().ok())
        {
            self.session_id = Some(session.to_string());
        }
        let resp = resp.error_for_status()?;
        let Some(expected_id) = expected_id else {
            return Ok(None);
        };
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let body = resp.bytes().await?;
        let envelope = proto::parse_jsonrpc_response(&content_type, &body, expected_id)?;
        Ok(Some(envelope))
    }

    /// `initialize` + `notifications/initialized` handshake. Captures the
    /// server's session id for subsequent requests.
    pub async fn initialize(&mut self) -> Result<ServerInfo, McpError> {
        let id = self.take_id();
        let payload = proto::build_initialize(
            id,
            proto::PROTOCOL_VERSION,
            &self.opts.client_name,
            &self.opts.client_version,
        );
        let envelope = self
            .post(&payload, Some(id))
            .await?
            .ok_or(McpError::Proto(ProtoError::NoEnvelope))?;
        let result = proto::extract_result(&envelope)?;
        let server_info = result.get("serverInfo").ok_or_else(|| {
            McpError::BadInitialize("missing serverInfo in initialize result".to_string())
        })?;
        let info = ServerInfo {
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
        };
        self.post(&proto::build_initialized(), None).await?;
        Ok(info)
    }

    /// `tools/list` → mapped definitions. Requires a completed `initialize`.
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, McpError> {
        let id = self.take_id();
        let envelope = self
            .post(&proto::build_tools_list(id), Some(id))
            .await?
            .ok_or(McpError::Proto(ProtoError::NoEnvelope))?;
        let result = proto::extract_result(&envelope)?;
        Ok(proto::map_tools_list(&result))
    }

    /// `tools/call`. Server-side tool failure (`isError`) maps to
    /// `McpError::ToolCall`; JSON-RPC failures to `McpError::Server`.
    pub async fn call_tool(&mut self, name: &str, args: &Value) -> Result<ToolOutput, McpError> {
        let id = self.take_id();
        let envelope = self
            .post(&proto::build_tools_call(id, name, args), Some(id))
            .await?
            .ok_or(McpError::Proto(ProtoError::NoEnvelope))?;
        let result = proto::extract_result(&envelope)?;
        Ok(proto::extract_tool_output(&result)?)
    }
}
