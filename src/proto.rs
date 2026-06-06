//! Sans-io MCP (Streamable HTTP / JSON-RPC 2.0) protocol helpers.
//! No network or WIT imports — usable from native hosts and wasm guests alike.

use crate::error::{ProtoError, ServerError, ToolCallError};
use serde_json::{Value, json};

/// MCP protocol revision this crate speaks by default.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Build a JSON-RPC `initialize` request.
#[must_use]
pub fn build_initialize(
    id: u64,
    protocol_version: &str,
    client_name: &str,
    client_version: &str,
) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": protocol_version,
            "capabilities": {},
            "clientInfo": { "name": client_name, "version": client_version }
        }
    })
}

/// Build the `notifications/initialized` notification (no id).
#[must_use]
pub fn build_initialized() -> Value {
    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
}

/// Build a JSON-RPC `tools/list` request.
#[must_use]
pub fn build_tools_list(id: u64) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {} })
}

/// Build a JSON-RPC `tools/call` request.
#[must_use]
pub fn build_tools_call(id: u64, name: &str, arguments: &Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    })
}

/// Consider one fully-accumulated SSE event payload. Returns `Some(value)`
/// when it is the JSON-RPC envelope matching `expected_id`; otherwise records
/// the first result/error-bearing envelope as `fallback` and returns `None`.
fn sse_consider(data: &str, expected_id: u64, fallback: &mut Option<Value>) -> Option<Value> {
    let value = serde_json::from_str::<Value>(data.trim()).ok()?;
    let has_body = value.get("result").is_some() || value.get("error").is_some();
    if !has_body {
        return None;
    }
    if value.get("id").and_then(Value::as_u64) == Some(expected_id) {
        return Some(value);
    }
    if fallback.is_none() {
        *fallback = Some(value);
    }
    None
}

/// Parse a JSON-RPC response from an HTTP body, unwrapping SSE framing when
/// the response is `text/event-stream`. Returns the JSON-RPC envelope whose
/// `id` matches `expected_id` (falling back to the first envelope carrying a
/// result or error if no id matches).
pub fn parse_jsonrpc_response(
    content_type: &str,
    body: &[u8],
    expected_id: u64,
) -> Result<Value, ProtoError> {
    let text = std::str::from_utf8(body)?;

    if content_type
        .to_ascii_lowercase()
        .contains("text/event-stream")
    {
        let mut current = String::new();
        let mut fallback: Option<Value> = None;
        for line in text.lines() {
            if line.is_empty() {
                if let Some(matched) = sse_consider(&current, expected_id, &mut fallback) {
                    return Ok(matched);
                }
                current.clear();
                continue;
            }
            if let Some(payload) = line.trim_start().strip_prefix("data:") {
                let payload = payload.strip_prefix(' ').unwrap_or(payload);
                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(payload);
            }
        }
        // Trailing event when the stream does not end with a blank line.
        if let Some(matched) = sse_consider(&current, expected_id, &mut fallback) {
            return Ok(matched);
        }
        fallback.ok_or(ProtoError::NoEnvelope)
    } else {
        Ok(serde_json::from_str::<Value>(text)?)
    }
}

/// One tool surfaced by a remote MCP `tools/list`.
#[derive(Debug, Clone, PartialEq)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool arguments; `{}` when the server omits it.
    pub input_schema: Value,
}

/// Successful `tools/call` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// `structuredContent` when the server returned one.
    pub structured: Option<Value>,
    /// Concatenated text `content` blocks (may be empty).
    pub text: String,
}

impl ToolOutput {
    /// `structuredContent` when present, else the joined text as a JSON string.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match &self.structured {
            Some(v) => v.clone(),
            None => Value::String(self.text.clone()),
        }
    }
}

/// Map a `tools/list` result into tool definitions. Tools without a name are
/// dropped; a missing `inputSchema` becomes `{}`.
#[must_use]
pub fn map_tools_list(result: &Value) -> Vec<McpToolDef> {
    let Some(tools) = result.get("tools").and_then(Value::as_array) else {
        return Vec::new();
    };
    tools
        .iter()
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?.to_string();
            let description = tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Some(McpToolDef {
                name,
                description,
                input_schema,
            })
        })
        .collect()
}

/// Extract a `tools/call` result. Honors the MCP `isError` flag (mapped to
/// `Err`); keeps structured content and joined text blocks separately.
pub fn extract_tool_output(result: &Value) -> Result<ToolOutput, ToolCallError> {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let structured = result.get("structuredContent").cloned();
    let mut parts: Vec<String> = Vec::new();
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for block in content {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }
    let text = parts.join("\n");
    if is_error {
        let message = match &structured {
            Some(v) => v.to_string(),
            None if text.is_empty() => "tool call returned isError".to_string(),
            None => text,
        };
        return Err(ToolCallError { message });
    }
    Ok(ToolOutput { structured, text })
}

/// Return the `result` object, or the typed error a JSON-RPC `error` carries.
pub fn extract_result(envelope: &Value) -> Result<Value, crate::error::McpError> {
    if let Some(error) = envelope.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error")
            .to_string();
        return Err(ServerError { code, message }.into());
    }
    envelope
        .get("result")
        .cloned()
        .ok_or(crate::error::McpError::Proto(ProtoError::MissingResult))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_has_protocol_and_client_info() {
        let req = build_initialize(1, PROTOCOL_VERSION, "test-client", "9.9.9");
        assert_eq!(req["jsonrpc"], "2.0");
        assert_eq!(req["id"], 1);
        assert_eq!(req["method"], "initialize");
        assert_eq!(req["params"]["protocolVersion"], "2025-06-18");
        assert_eq!(req["params"]["clientInfo"]["name"], "test-client");
        assert_eq!(req["params"]["clientInfo"]["version"], "9.9.9");
    }

    #[test]
    fn initialized_is_a_notification_without_id() {
        let req = build_initialized();
        assert_eq!(req["method"], "notifications/initialized");
        assert!(req.get("id").is_none());
    }

    #[test]
    fn tools_list_shape() {
        let req = build_tools_list(2);
        assert_eq!(req["id"], 2);
        assert_eq!(req["method"], "tools/list");
    }

    #[test]
    fn tools_call_carries_name_and_arguments() {
        let req = build_tools_call(
            2,
            "get_issue",
            &json!({ "owner": "o", "repo": "r", "issue_number": 1 }),
        );
        assert_eq!(req["method"], "tools/call");
        assert_eq!(req["params"]["name"], "get_issue");
        assert_eq!(req["params"]["arguments"]["repo"], "r");
    }

    #[test]
    fn parses_plain_json_response() {
        let body = br#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#;
        let value = parse_jsonrpc_response("application/json", body, 2).unwrap();
        assert_eq!(value["id"], 2);
        assert!(value["result"]["tools"].is_array());
    }

    #[test]
    fn unwraps_sse_response_and_matches_id() {
        let body =
            b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n\n";
        let value = parse_jsonrpc_response("text/event-stream; charset=utf-8", body, 2).unwrap();
        assert_eq!(value["result"]["ok"], true);
    }

    #[test]
    fn sse_picks_matching_id_among_multiple_events() {
        let body = b"data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"first\":true}}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"second\":true}}\n\n";
        let value = parse_jsonrpc_response("text/event-stream", body, 2).unwrap();
        assert_eq!(value["result"]["second"], true);
    }

    #[test]
    fn sse_concatenates_multiline_data() {
        let body = b"data: {\"jsonrpc\":\"2.0\",\"id\":2,\ndata: \"result\":{\"ok\":true}}\n\n";
        let value = parse_jsonrpc_response("text/event-stream", body, 2).unwrap();
        assert_eq!(value["result"]["ok"], true);
    }

    #[test]
    fn sse_with_only_notifications_is_no_envelope() {
        let body = b"data: {\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n\n";
        let err = parse_jsonrpc_response("text/event-stream", body, 2).unwrap_err();
        assert!(matches!(err, ProtoError::NoEnvelope));
    }

    #[test]
    fn malformed_json_body_is_decode_error() {
        let err = parse_jsonrpc_response("application/json", b"{not json", 2).unwrap_err();
        assert!(matches!(err, ProtoError::Decode(_)));
    }

    #[test]
    fn extract_result_returns_result() {
        let env = json!({ "jsonrpc": "2.0", "id": 2, "result": { "x": 1 } });
        assert_eq!(extract_result(&env).unwrap()["x"], 1);
    }

    #[test]
    fn extract_result_surfaces_server_error() {
        let env = json!({ "jsonrpc": "2.0", "id": 2, "error": { "code": -32602, "message": "bad params" } });
        let err = extract_result(&env).unwrap_err();
        match err {
            crate::error::McpError::Server(ServerError { code, message }) => {
                assert_eq!(code, -32602);
                assert_eq!(message, "bad params");
            }
            other => panic!("expected Server error, got: {other}"),
        }
    }

    #[test]
    fn extract_result_missing_both_is_proto_error() {
        let env = json!({ "jsonrpc": "2.0", "id": 2 });
        let err = extract_result(&env).unwrap_err();
        assert!(matches!(
            err,
            crate::error::McpError::Proto(ProtoError::MissingResult)
        ));
    }

    #[test]
    fn maps_tools_list_with_value_schema() {
        let result = json!({
            "tools": [
                { "name": "get_issue", "description": "Get an issue",
                  "inputSchema": { "type": "object", "properties": { "repo": { "type": "string" } } } },
                { "name": "search_code", "description": "Search code", "inputSchema": { "type": "object" } }
            ]
        });
        let defs = map_tools_list(&result);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "get_issue");
        assert_eq!(defs[0].input_schema["properties"]["repo"]["type"], "string");
    }

    #[test]
    fn map_tools_list_missing_tools_returns_empty() {
        assert!(map_tools_list(&json!({})).is_empty());
    }

    #[test]
    fn map_tools_list_drops_tool_without_name() {
        let result = json!({ "tools": [
            { "name": "good_tool", "description": "ok" },
            { "description": "no name here" }
        ]});
        let defs = map_tools_list(&result);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "good_tool");
    }

    #[test]
    fn map_tools_list_missing_schema_defaults_to_empty_object() {
        let result = json!({ "tools": [{ "name": "t", "description": "d" }] });
        let defs = map_tools_list(&result);
        assert_eq!(defs[0].input_schema, json!({}));
    }

    #[test]
    fn extract_tool_output_joins_text_content() {
        let result = json!({ "content": [ { "type": "text", "text": "line1" }, { "type": "text", "text": "line2" } ] });
        let out = extract_tool_output(&result).unwrap();
        assert_eq!(out.text, "line1\nline2");
        assert!(out.structured.is_none());
        assert_eq!(out.to_value(), json!("line1\nline2"));
    }

    #[test]
    fn extract_tool_output_keeps_structured_content() {
        let result = json!({ "structuredContent": { "number": 7 }, "content": [ { "type": "text", "text": "also text" } ] });
        let out = extract_tool_output(&result).unwrap();
        assert_eq!(out.structured, Some(json!({ "number": 7 })));
        assert_eq!(out.text, "also text");
        assert_eq!(out.to_value(), json!({ "number": 7 }));
    }

    #[test]
    fn extract_tool_output_is_error_maps_to_err() {
        let result =
            json!({ "isError": true, "content": [ { "type": "text", "text": "not found" } ] });
        let err = extract_tool_output(&result).unwrap_err();
        assert_eq!(err.message, "not found");
    }

    #[test]
    fn extract_tool_output_is_error_without_payload_has_default_message() {
        let result = json!({ "isError": true });
        let err = extract_tool_output(&result).unwrap_err();
        assert_eq!(err.message, "tool call returned isError");
    }

    #[test]
    fn extract_tool_output_empty_content_returns_empty_ok() {
        let result = json!({ "content": [] });
        let out = extract_tool_output(&result).unwrap();
        assert_eq!(out.text, "");
    }
}
