//! Error types shared by the sans-io proto layer and the native client.

use thiserror::Error;

/// Failure decoding a JSON-RPC envelope from an HTTP body.
#[derive(Debug, Error)]
pub enum ProtoError {
    #[error("response body is not UTF-8: {0}")]
    NotUtf8(#[from] std::str::Utf8Error),
    #[error("decode JSON-RPC response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("no JSON-RPC response found in SSE stream")]
    NoEnvelope,
    #[error("JSON-RPC response has neither result nor error")]
    MissingResult,
}

/// A JSON-RPC `error` object returned by the server.
#[derive(Debug, Error, PartialEq, Eq)]
#[error("JSON-RPC error {code}: {message}")]
pub struct ServerError {
    pub code: i64,
    pub message: String,
}

/// A `tools/call` result carrying `isError: true`. The message is the
/// server-provided error payload (structured content stringified, or the
/// joined text blocks).
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{message}")]
pub struct ToolCallError {
    pub message: String,
}

/// Unified error surface for the native client. The sans-io layer returns the
/// narrower types above; `McpError` wraps them via `From` so `?` flows.
#[derive(Debug, Error)]
pub enum McpError {
    #[cfg(feature = "native")]
    #[error("transport: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("protocol: {0}")]
    Proto(#[from] ProtoError),
    #[error(transparent)]
    Server(#[from] ServerError),
    #[error("tool call failed: {0}")]
    ToolCall(#[from] ToolCallError),
    #[error("initialize result malformed: {0}")]
    BadInitialize(String),
}
