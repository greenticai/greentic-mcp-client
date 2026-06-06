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
