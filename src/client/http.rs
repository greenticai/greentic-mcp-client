//! Native (reqwest) transport for remote MCP servers. Feature `native`.

use super::{McpClient, McpClientOptions, ServerInfo};
use crate::auth::McpAuth;
use crate::error::{McpError, ProtoError};
use crate::proto::{self, McpToolDef, ToolOutput};
use serde_json::Value;
use url::Url;

const SESSION_HEADER: &str = "Mcp-Session-Id";

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
}

/// Inherent (non-trait) convenience wrappers so callers that hold a concrete
/// `McpHttpClient` need not import the [`McpClient`] trait.
impl McpHttpClient {
    /// See [`McpClient::initialize`].
    pub async fn initialize(&mut self) -> Result<ServerInfo, McpError> {
        <Self as McpClient>::initialize(self).await
    }

    /// See [`McpClient::list_tools`].
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, McpError> {
        <Self as McpClient>::list_tools(self).await
    }

    /// See [`McpClient::call_tool`].
    pub async fn call_tool(&mut self, name: &str, args: &Value) -> Result<ToolOutput, McpError> {
        <Self as McpClient>::call_tool(self, name, args).await
    }
}

impl McpClient for McpHttpClient {
    /// `initialize` + `notifications/initialized` handshake. Captures the
    /// server's session id for subsequent requests.
    async fn initialize(&mut self) -> Result<ServerInfo, McpError> {
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
        let info = ServerInfo::from_initialize_result(&result)?;
        self.post(&proto::build_initialized(), None).await?;
        Ok(info)
    }

    /// `tools/list` → mapped definitions. Requires a completed `initialize`.
    async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, McpError> {
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
    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<ToolOutput, McpError> {
        let id = self.take_id();
        let envelope = self
            .post(&proto::build_tools_call(id, name, args), Some(id))
            .await?
            .ok_or(McpError::Proto(ProtoError::NoEnvelope))?;
        let result = proto::extract_result(&envelope)?;
        Ok(proto::extract_tool_output(&result)?)
    }
}
