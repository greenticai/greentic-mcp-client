//! Native-transport tests against a wiremock fake MCP server.

use greentic_mcp_client::client::{McpClientOptions, McpHttpClient};
use greentic_mcp_client::{McpAuth, McpError};
use serde_json::json;
use std::time::Duration;
use url::Url;
use wiremock::matchers::{body_partial_json, header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client_for(server: &MockServer, auth: Option<McpAuth>) -> McpHttpClient {
    McpHttpClient::new(
        Url::parse(&server.uri()).expect("mock uri parses"),
        auth,
        McpClientOptions {
            timeout: Duration::from_secs(2),
            client_name: "test-client".into(),
            client_version: "0.0.0".into(),
        },
    )
    .expect("client builds")
}

fn initialize_ok() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("Mcp-Session-Id", "sess-123")
        .set_body_json(json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "protocolVersion": "2025-06-18",
                "serverInfo": { "name": "fake-server", "version": "1.0.0" }
            }
        }))
}

#[tokio::test]
async fn handshake_returns_server_info_and_replays_session_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .respond_with(initialize_ok())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({ "method": "notifications/initialized" }),
        ))
        .and(header("Mcp-Session-Id", "sess-123"))
        .respond_with(ResponseTemplate::new(202))
        .expect(1)
        .mount(&server)
        .await;

    let mut client = client_for(&server, None);
    let info = client.initialize().await.expect("handshake succeeds");
    assert_eq!(info.name, "fake-server");
    assert_eq!(info.version, "1.0.0");
    assert_eq!(info.protocol_version, "2025-06-18");
}

#[tokio::test]
async fn default_auth_sends_authorization_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .and(header("Authorization", "Bearer tok-1"))
        .respond_with(initialize_ok())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({ "method": "notifications/initialized" }),
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;

    let mut client = client_for(&server, Some(McpAuth::bearer("tok-1")));
    client.initialize().await.expect("auth header accepted");
}

#[tokio::test]
async fn custom_auth_header_sends_raw_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .and(header("X-Api-Key", "tok-raw"))
        .respond_with(initialize_ok())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({ "method": "notifications/initialized" }),
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;

    let auth = McpAuth {
        header_name: Some("X-Api-Key".into()),
        token: "tok-raw".into(),
    };
    let mut client = client_for(&server, Some(auth));
    client.initialize().await.expect("custom header accepted");
}

#[tokio::test]
async fn http_error_status_is_transport_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let mut client = client_for(&server, None);
    let err = client.initialize().await.expect_err("401 must fail");
    assert!(matches!(err, McpError::Transport(_)), "got: {err}");
}

async fn mount_handshake(server: &MockServer) {
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "initialize" })))
        .respond_with(initialize_ok())
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            json!({ "method": "notifications/initialized" }),
        ))
        .respond_with(ResponseTemplate::new(202))
        .mount(server)
        .await;
}

#[tokio::test]
async fn list_tools_maps_definitions() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "tools/list" })))
        .and(header("Mcp-Session-Id", "sess-123"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "tools": [
                { "name": "echo", "description": "Echo back",
                  "inputSchema": { "type": "object", "properties": { "msg": { "type": "string" } } } }
            ]}
        })))
        .mount(&server)
        .await;

    let mut client = client_for(&server, None);
    client.initialize().await.expect("handshake");
    let tools = client.list_tools().await.expect("tools listed");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].input_schema["properties"]["msg"]["type"], "string");
}

#[tokio::test]
async fn list_tools_handles_sse_response() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    let sse_body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"tools\":[{\"name\":\"t\",\"description\":\"d\"}]}}\n\n";
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "tools/list" })))
        .respond_with(
            // wiremock 0.6: set_body_raw sets both body and mime together;
            // insert_header + set_body_string does NOT work because the internal
            // `mime` field always wins over headers for Content-Type.
            ResponseTemplate::new(200)
                .set_body_raw(sse_body.as_bytes().to_vec(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let mut client = client_for(&server, None);
    client.initialize().await.expect("handshake");
    let tools = client.list_tools().await.expect("sse tools listed");
    assert_eq!(tools[0].name, "t");
}

#[tokio::test]
async fn call_tool_returns_structured_output() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({
            "method": "tools/call",
            "params": { "name": "echo", "arguments": { "msg": "hi" } }
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "structuredContent": { "echoed": "hi" } }
        })))
        .mount(&server)
        .await;

    let mut client = client_for(&server, None);
    client.initialize().await.expect("handshake");
    let out = client
        .call_tool("echo", &json!({ "msg": "hi" }))
        .await
        .expect("tool call ok");
    assert_eq!(out.to_value(), json!({ "echoed": "hi" }));
}

#[tokio::test]
async fn call_tool_is_error_surfaces_tool_call_error() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "tools/call" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 2,
            "result": { "isError": true, "content": [{ "type": "text", "text": "boom" }] }
        })))
        .mount(&server)
        .await;

    let mut client = client_for(&server, None);
    client.initialize().await.expect("handshake");
    let err = client
        .call_tool("echo", &json!({}))
        .await
        .expect_err("isError must map to Err");
    match err {
        McpError::ToolCall(e) => assert_eq!(e.message, "boom"),
        other => panic!("expected ToolCall, got: {other}"),
    }
}

#[tokio::test]
async fn jsonrpc_error_surfaces_server_error() {
    let server = MockServer::start().await;
    mount_handshake(&server).await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({ "method": "tools/list" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0", "id": 2,
            "error": { "code": -32000, "message": "session expired" }
        })))
        .mount(&server)
        .await;

    let mut client = client_for(&server, None);
    client.initialize().await.expect("handshake");
    let err = client.list_tools().await.expect_err("rpc error must fail");
    match err {
        McpError::Server(e) => {
            assert_eq!(e.code, -32000);
            assert_eq!(e.message, "session expired");
        }
        other => panic!("expected Server, got: {other}"),
    }
}

#[tokio::test]
async fn slow_server_times_out_as_transport_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(initialize_ok().set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;

    let mut client = client_for(&server, None); // 2s timeout
    let err = client.initialize().await.expect_err("must time out");
    assert!(matches!(err, McpError::Transport(_)), "got: {err}");
}
