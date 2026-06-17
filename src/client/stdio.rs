//! Native (tokio process) transport for local stdio MCP servers.
//! Feature `native`; not part of the wasm core.
//!
//! A stdio MCP server is launched as a child process. We speak JSON-RPC 2.0
//! over its stdin/stdout, framing each message as a single line of JSON
//! (newline-delimited — **not** SSE). The child is killed when the client is
//! dropped.

use super::{McpClient, McpClientOptions, ServerInfo};
use crate::error::McpError;
use crate::proto::{self, McpToolDef, ToolOutput};
use serde_json::Value;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

/// One logical session against a local stdio MCP server subprocess. Request ids
/// are monotonically increasing; responses are matched to their request by id,
/// so out-of-band lines (log notifications, server-initiated notifications) are
/// skipped while waiting for a reply.
pub struct McpStdioClient {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    opts: McpClientOptions,
    next_id: u64,
}

impl McpStdioClient {
    /// Spawn `command` with `args` and the extra `env` pairs, wiring its stdin
    /// and stdout to pipes. The child inherits the parent environment; `env`
    /// entries are added on top. Stderr is inherited so server diagnostics are
    /// visible to the operator.
    ///
    /// Returns [`McpError::Spawn`] if the process cannot be launched (e.g. the
    /// command is not found) or its pipes cannot be captured.
    pub fn spawn(
        command: &str,
        args: &[String],
        env: &[(String, String)],
        opts: McpClientOptions,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        for (key, value) in env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|source| McpError::Spawn {
            command: command.to_string(),
            source,
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            McpError::StdioTransport("child stdin pipe was not captured".to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            McpError::StdioTransport("child stdout pipe was not captured".to_string())
        })?;

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            opts,
            next_id: 1,
        })
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Write one JSON-RPC payload as a single newline-terminated line.
    async fn write_message(&mut self, payload: &Value) -> Result<(), McpError> {
        let mut line = payload.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(broken_pipe)?;
        self.stdin.flush().await.map_err(broken_pipe)?;
        Ok(())
    }

    /// Read response lines until one carries the JSON-RPC envelope matching
    /// `expected_id`. Lines that are not JSON, or that are notifications /
    /// other-id envelopes, are skipped. A closed stdout (child exited) surfaces
    /// as [`McpError::StdioTransport`]; exceeding the per-request timeout
    /// surfaces the same way.
    async fn read_response(&mut self, expected_id: u64) -> Result<Value, McpError> {
        loop {
            let next = timeout(self.opts.timeout, self.stdout.next_line())
                .await
                .map_err(|_| McpError::StdioTransport("timed out waiting for response".into()))?
                .map_err(broken_pipe)?;

            let Some(line) = next else {
                // EOF: the child closed stdout (typically because it exited).
                return Err(self.exit_error().await);
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                // Non-JSON line (e.g. a stray log on stdout): ignore and read on.
                continue;
            };
            let has_body = value.get("result").is_some() || value.get("error").is_some();
            if !has_body {
                continue;
            }
            if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
                return Ok(value);
            }
            // Result/error for a different id — skip and keep reading.
        }
    }

    /// Build a transport error describing how the child exited, after stdout
    /// reached EOF. Falls back to a generic message if the status is
    /// unavailable without blocking.
    async fn exit_error(&mut self) -> McpError {
        let detail = match self.child.try_wait() {
            Ok(Some(status)) => format!("child exited ({status}) before responding"),
            Ok(None) => "child closed stdout before responding".to_string(),
            Err(source) => return broken_pipe(source),
        };
        McpError::StdioTransport(detail)
    }

    /// Send a request and read its matching response envelope.
    async fn request(&mut self, payload: &Value, expected_id: u64) -> Result<Value, McpError> {
        self.write_message(payload).await?;
        self.read_response(expected_id).await
    }
}

impl McpClient for McpStdioClient {
    /// `initialize` + `notifications/initialized` handshake.
    async fn initialize(&mut self) -> Result<ServerInfo, McpError> {
        let id = self.take_id();
        let payload = proto::build_initialize(
            id,
            proto::PROTOCOL_VERSION,
            &self.opts.client_name,
            &self.opts.client_version,
        );
        let envelope = self.request(&payload, id).await?;
        let result = proto::extract_result(&envelope)?;
        let info = ServerInfo::from_initialize_result(&result)?;
        // `notifications/initialized` carries no id and expects no reply.
        self.write_message(&proto::build_initialized()).await?;
        Ok(info)
    }

    /// `tools/list` → mapped definitions. Requires a completed `initialize`.
    async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, McpError> {
        let id = self.take_id();
        let envelope = self.request(&proto::build_tools_list(id), id).await?;
        let result = proto::extract_result(&envelope)?;
        Ok(proto::map_tools_list(&result))
    }

    /// `tools/call`. Server-side tool failure (`isError`) maps to
    /// `McpError::ToolCall`; JSON-RPC failures to `McpError::Server`.
    async fn call_tool(&mut self, name: &str, args: &Value) -> Result<ToolOutput, McpError> {
        let id = self.take_id();
        let envelope = self
            .request(&proto::build_tools_call(id, name, args), id)
            .await?;
        let result = proto::extract_result(&envelope)?;
        Ok(proto::extract_tool_output(&result)?)
    }
}

/// Inherent (non-trait) convenience wrappers so callers that hold a concrete
/// `McpStdioClient` need not import the [`McpClient`] trait.
impl McpStdioClient {
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

/// Map a pipe I/O error to the transport error variant.
fn broken_pipe(source: std::io::Error) -> McpError {
    McpError::StdioTransport(format!("stdio pipe error: {source}"))
}
