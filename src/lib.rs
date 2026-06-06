#![forbid(unsafe_code)]
//! Client for remote MCP servers over Streamable HTTP (JSON-RPC 2.0).
//!
//! Not to be confused with [`greentic-mcp`](https://github.com/greenticai/greentic-mcp)
//! (`greentic-mcp-exec`), which loads and executes `wasix:mcp` **WASM
//! components** locally. This crate is the network-client side: it connects to
//! remote MCP servers, lists their tools, and invokes them.
//!
//! The [`proto`] module is sans-io and compiles for `wasm32-wasip2`
//! (`default-features = false`); the [`client`] module (feature `native`, on
//! by default) adds a reqwest transport.

pub mod error;
pub mod proto;

pub use error::{McpError, ProtoError, ServerError, ToolCallError};
pub use proto::{McpToolDef, PROTOCOL_VERSION, ToolOutput};
