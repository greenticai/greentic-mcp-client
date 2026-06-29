# greentic-mcp-client

Client for **MCP servers** over JSON-RPC 2.0 (protocol `2025-06-18`), via two
native transports:

- **HTTP** — remote servers over Streamable HTTP / SSE (`McpHttpClient`).
- **stdio** — local subprocess servers over newline-delimited JSON-RPC on the
  child's stdin/stdout (`McpStdioClient`).

> Not to be confused with [`greentic-mcp`](https://github.com/greenticai/greentic-mcp)
> (`greentic-mcp-exec`), which loads and executes `wasix:mcp` **WASM
> components** locally. This crate is the network-client side.

## Layout

- `proto` — sans-io protocol core: request builders, SSE-aware response
  parsing, tool mapping. Compiles for `wasm32-wasip2`
  (`default-features = false`).
- `client` — native transports (feature `native`, default), both exposing the
  shared `McpClient` shape (`initialize` / `list_tools` / `call_tool`):
  - `McpHttpClient` — reqwest transport, `Mcp-Session-Id` threading,
    configurable auth header.
  - `McpStdioClient` — tokio-process transport; spawns a local MCP server and
    frames messages as line-delimited JSON. The child is killed on drop.

## Usage (native, HTTP)

```rust
use greentic_mcp_client::{McpAuth, McpClientOptions, McpHttpClient};
use url::Url;

let mut client = McpHttpClient::new(
    Url::parse("https://example.com/mcp")?,
    Some(McpAuth::bearer("token")),
    McpClientOptions::default(),
)?;
let info = client.initialize().await?;
let tools = client.list_tools().await?;
let out = client.call_tool("echo", &serde_json::json!({ "msg": "hi" })).await?;
```

## Usage (native, stdio)

```rust
use greentic_mcp_client::{McpClientOptions, McpStdioClient};

let mut client = McpStdioClient::spawn(
    "npx",
    &["-y".into(), "@modelcontextprotocol/server-everything".into()],
    &[], // extra env pairs, on top of the inherited environment
    McpClientOptions::default(),
)?;
let info = client.initialize().await?;
let tools = client.list_tools().await?;
let out = client.call_tool("echo", &serde_json::json!({ "msg": "hi" })).await?;
```

## Usage (wasm extension)

```toml
greentic-mcp-client = { version = "1.2", default-features = false }
```

Use the `proto` builders/parsers with your host's HTTP imports.

## Consumers

- `greentic-designer-admin` — tenant MCP-server test-connection endpoint
- `greentic-designer` — flow-prompt MCP tool source (planned)
- `greentic-aw-runtime` — agentic-worker MCP tool dispatch (planned)
- `component-github-mcp-ext` — proto layer inside the WASM extension (planned refactor)
