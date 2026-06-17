//! Native-transport tests against a mock stdio MCP server.
//!
//! The mock (`tests/mock_stdio_server.py`) speaks newline-delimited JSON-RPC
//! 2.0 over stdin/stdout. The tests are hermetic: they spawn `python3` on the
//! checked-in helper, so no network or external service is touched.

#![cfg(feature = "native")]

use greentic_mcp_client::McpError;
use greentic_mcp_client::client::{McpClientOptions, McpStdioClient};
use serde_json::json;
use std::path::PathBuf;

fn mock_server_path() -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("mock_stdio_server.py");
    path.to_string_lossy().into_owned()
}

fn spawn_mock() -> McpStdioClient {
    McpStdioClient::spawn(
        "python3",
        &[mock_server_path()],
        &[],
        McpClientOptions {
            client_name: "test-client".into(),
            client_version: "0.0.0".into(),
            ..Default::default()
        },
    )
    .expect("mock stdio server spawns")
}

#[tokio::test]
async fn handshake_returns_server_info() {
    let mut client = spawn_mock();
    let info = client.initialize().await.expect("handshake succeeds");
    assert_eq!(info.name, "mock-stdio");
    assert_eq!(info.version, "9.9.9");
    assert_eq!(info.protocol_version, "2025-06-18");
}

#[tokio::test]
async fn list_tools_maps_definitions() {
    let mut client = spawn_mock();
    client.initialize().await.expect("handshake");
    let tools = client.list_tools().await.expect("tools listed");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].input_schema["properties"]["msg"]["type"], "string");
}

#[tokio::test]
async fn call_tool_returns_structured_output() {
    let mut client = spawn_mock();
    client.initialize().await.expect("handshake");
    let out = client
        .call_tool("echo", &json!({ "msg": "hi" }))
        .await
        .expect("tool call ok");
    assert_eq!(out.to_value(), json!({ "echoed": "hi" }));
}

#[tokio::test]
async fn call_unknown_tool_surfaces_tool_call_error() {
    let mut client = spawn_mock();
    client.initialize().await.expect("handshake");
    let err = client
        .call_tool("nope", &json!({}))
        .await
        .expect_err("unknown tool must map to Err");
    match err {
        McpError::ToolCall(e) => assert_eq!(e.message, "unknown tool"),
        other => panic!("expected ToolCall, got: {other}"),
    }
}

#[tokio::test]
async fn child_crash_surfaces_transport_error() {
    let mut client = spawn_mock();
    client.initialize().await.expect("handshake");
    // The mock exits non-zero when asked to call `boom`; the client's next read
    // hits a closed pipe and must surface a transport error, not hang or panic.
    let err = client
        .call_tool("boom", &json!({}))
        .await
        .expect_err("child crash must fail");
    assert!(
        matches!(err, McpError::StdioTransport(_)),
        "expected StdioTransport, got: {err}"
    );
}

#[tokio::test]
async fn spawn_missing_command_is_spawn_error() {
    let result = McpStdioClient::spawn(
        "this-binary-does-not-exist-xyzzy",
        &[],
        &[],
        McpClientOptions::default(),
    );
    match result {
        Err(McpError::Spawn { .. }) => {}
        Err(other) => panic!("expected Spawn, got: {other}"),
        Ok(_) => panic!("spawning a missing command must fail"),
    }
}

#[tokio::test]
async fn env_vars_are_passed_to_child() {
    // The mock ignores env, but spawning with an env pair must still succeed and
    // not interfere with the handshake — guards the env-plumbing path.
    let mut client = McpStdioClient::spawn(
        "python3",
        &[mock_server_path()],
        &[("MCP_TEST_FLAG".to_string(), "1".to_string())],
        McpClientOptions::default(),
    )
    .expect("spawn with env succeeds");
    let info = client.initialize().await.expect("handshake");
    assert_eq!(info.name, "mock-stdio");
}
