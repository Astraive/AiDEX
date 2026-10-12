//! MCP JSON-RPC 2.0 messages: building requests, classifying what a server
//! sends, and turning `tools/list` and `tools/call` results into bounded,
//! typed values.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{McpError, McpErrorKind, redact::bounded_redacted, redact::truncate_utf8};

/// The protocol revision AiDEX asks for.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions AiDEX accepts from a server, newest first.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
/// `clientInfo.name` sent in `initialize`.
pub const CLIENT_NAME: &str = "AiDEX";
/// Most tools kept from one server.
pub const MAX_TOOLS: usize = 512;
/// Most `tools/list` pages followed.
pub const MAX_TOOL_PAGES: usize = 64;
/// Text kept from one tool result.
pub const MAX_RESULT_TEXT: usize = 256 * 1024;
const MAX_DESCRIPTION: usize = 4096;
const MAX_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_TOOL_NAME_CHARS: usize = 128;
const MAX_ERROR_TEXT: usize = 1024;

/// One tool a server offers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpToolSpec {
    pub name: String,
    /// The server's description (untrusted text), bounded to 4 KiB.
    pub description: String,
    /// The JSON schema of the arguments (an object schema, at most 64 KiB).
    pub input_schema: Value,
}

/// A `tools/call` result reduced to bounded text. Images, audio, and binary
/// resources are summarised, never inlined.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct McpToolResult {
    pub content_text: String,
    /// The server reported a tool-level failure (`isError`).
    pub is_error: bool,
    /// The text was cut to [`MAX_RESULT_TEXT`].
    pub truncated: bool,
}

/// What a server sent.
#[derive(Debug)]
pub(crate) enum Incoming {
    Response {
        id: Value,
        outcome: Result<Value, McpError>,
    },
    Request {
        id: Value,
        method: String,
    },
    Notification {
        method: String,
        params: Value,
    },
}

pub(crate) fn request(id: i64, method: &str, params: Option<Value>) -> Value {
    let mut message = json!({"jsonrpc": "2.0", "id": id, "method": method});
    if let Some(params) = params {
        message["params"] = params;
    }
    message
}

pub(crate) fn notification(method: &str, params: Option<Value>) -> Value {
    let mut message = json!({"jsonrpc": "2.0", "method": method});
    if let Some(params) = params {
        message["params"] = params;
    }
    message
}

pub(crate) fn initialize_params() -> Value {
    json!({
        "protocolVersion": LATEST_PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {"name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")},
    })
}

/// Whether `message` expects a response (a request rather than a notification
/// or a reply).
pub(crate) fn is_request(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_some()
}

/// Classify one inbound JSON-RPC object.
pub(crate) fn classify(frame: Value) -> Result<Incoming, McpError> {
    let Value::Object(mut object) = frame else {
        return Err(McpError::protocol("a server message must be a JSON object"));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(McpError::protocol("a server message is not JSON-RPC 2.0"));
    }
    let id = object.remove("id");
    match (object.remove("method"), id) {
        (Some(Value::String(method)), Some(id)) => Ok(Incoming::Request { id, method }),
        (Some(Value::String(method)), None) => Ok(Incoming::Notification {
            method,
            params: object.remove("params").unwrap_or(Value::Null),
        }),
        (Some(_), _) => Err(McpError::protocol(
            "a server message has a non-string method",
        )),
        (None, Some(id)) => {
            let outcome = match (object.remove("result"), object.remove("error")) {
                (Some(result), None) => Ok(result),
                (None, Some(error)) => Err(server_error(&error)),
                _ => Err(McpError::protocol(
                    "a server response must carry exactly one of result or error",
                )),
            };
            Ok(Incoming::Response { id, outcome })
        }
        (None, None) => Err(McpError::protocol(
            "a server message has neither a method nor an id",
        )),
    }
}

/// A JSON-RPC error object as an [`McpError`] (bounded, redacted message).
pub(crate) fn server_error(error: &Value) -> McpError {
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("no message");
    McpError::new(
        McpErrorKind::ServerError,
        format!(
            "the MCP server returned error {code}: {}",
            bounded_redacted(message, MAX_ERROR_TEXT)
        ),
    )
}

/// AiDEX's answer to a request the server sent: `ping` succeeds, everything
/// else (sampling, roots, elicitation) is declined as unsupported.
pub(crate) fn reply_to_server_request(id: Value, method: &str) -> Value {
    if method == "ping" {
        json!({"jsonrpc": "2.0", "id": id, "result": {}})
    } else {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": "AiDEX does not support this request"},
        })
    }
}

/// The negotiated protocol version from an `initialize` result.
pub(crate) fn negotiated_version(result: &Value) -> Result<String, McpError> {
    let version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::protocol("the initialize result has no protocolVersion"))?;
    if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
        Ok(version.to_owned())
    } else {
        Err(McpError::protocol(format!(
            "the server speaks MCP {}, which AiDEX does not support ({})",
            bounded_redacted(version, 32),
            SUPPORTED_PROTOCOL_VERSIONS.join(", ")
        )))
    }
}

/// One `tools/list` page: its tools, the next cursor, and notes about skipped
/// or reduced tools.
pub(crate) struct ToolPage {
    pub tools: Vec<McpToolSpec>,
    pub next_cursor: Option<String>,
    pub notes: Vec<String>,
}

pub(crate) fn parse_tools_page(result: &Value) -> Result<ToolPage, McpError> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| McpError::protocol("the tools/list result has no tools array"))?;
    let mut page = ToolPage {
        tools: Vec::new(),
        next_cursor: None,
        notes: Vec::new(),
    };
    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            page.notes.push("skipped a tool without a name".into());
            continue;
        };
        if name.is_empty()
            || name.chars().count() > MAX_TOOL_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            page.notes.push(format!(
                "skipped tool `{}`: invalid name",
                bounded_redacted(name, 64)
            ));
            continue;
        }
        let description = tool
            .get("description")
            .and_then(Value::as_str)
            .or_else(|| tool.get("title").and_then(Value::as_str))
            .unwrap_or_default();
        let (description, _) = truncate_utf8(description, MAX_DESCRIPTION);
        let input_schema = match tool.get("inputSchema") {
            Some(schema @ Value::Object(_))
                if serde_json::to_vec(schema).map_or(0, |b| b.len()) <= MAX_SCHEMA_BYTES =>
            {
                normalize_schema(schema.clone())
            }
            Some(Value::Object(_)) => {
                page.notes.push(format!(
                    "the input schema of `{name}` exceeds 64 KiB and was replaced"
                ));
                json!({"type": "object"})
            }
            _ => json!({"type": "object"}),
        };
        page.tools.push(McpToolSpec {
            name: name.to_owned(),
            description: description.to_owned(),
            input_schema,
        });
    }
    page.next_cursor = match result.get("nextCursor") {
        None | Some(Value::Null) => None,
        Some(Value::String(cursor)) if !cursor.is_empty() && cursor.len() <= 4096 => {
            Some(cursor.clone())
        }
        Some(_) => return Err(McpError::protocol("the tools/list nextCursor is invalid")),
    };
    Ok(page)
}

/// Every object schema advertises `"type": "object"`.
fn normalize_schema(mut schema: Value) -> Value {
    if let Value::Object(map) = &mut schema
        && !map.contains_key("type")
    {
        map.insert("type".into(), Value::String("object".into()));
    }
    schema
}

/// Reduce a `tools/call` result to bounded text.
pub(crate) fn tool_result(result: &Value, max_text: usize) -> Result<McpToolResult, McpError> {
    let object = result
        .as_object()
        .ok_or_else(|| McpError::protocol("the tools/call result is not an object"))?;
    let is_error = object
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut parts: Vec<String> = Vec::new();
    match object.get("content") {
        Some(Value::Array(blocks)) => {
            for block in blocks {
                parts.push(describe_block(block));
            }
        }
        Some(Value::Null) | None => {}
        Some(_) => return Err(McpError::protocol("the tools/call content is not an array")),
    }
    if parts.is_empty()
        && let Some(structured) = object.get("structuredContent")
    {
        parts.push(structured.to_string());
    }
    // Credential patterns are redacted before the text can reach a model, a
    // transcript, or a log.
    let joined = parts.join("\n");
    let text = aidex_events::redact_text(&joined);
    let (kept, truncated) = truncate_utf8(&text, max_text);
    Ok(McpToolResult {
        content_text: kept.to_owned(),
        is_error,
        truncated,
    })
}

fn describe_block(block: &Value) -> String {
    let empty = Map::new();
    let object = block.as_object().unwrap_or(&empty);
    let field = |name: &str| object.get(name).and_then(Value::as_str).unwrap_or_default();
    let encoded_len = |name: &str| field(name).len() / 4 * 3;
    match field("type") {
        "text" => field("text").to_owned(),
        "image" => format!(
            "[image: {}, about {} bytes]",
            or_unknown(field("mimeType")),
            encoded_len("data")
        ),
        "audio" => format!(
            "[audio: {}, about {} bytes]",
            or_unknown(field("mimeType")),
            encoded_len("data")
        ),
        "resource_link" => {
            let name = bounded_redacted(field("name"), 128);
            let uri = bounded_redacted(field("uri"), 512);
            if name.is_empty() {
                format!("[resource link: {uri}]")
            } else {
                format!("[resource link: {uri} {name}]")
            }
        }
        "resource" => {
            let resource = object.get("resource").and_then(Value::as_object);
            let get = |name: &str| {
                resource
                    .and_then(|r| r.get(name))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            };
            let uri = bounded_redacted(get("uri"), 512);
            match resource.and_then(|r| r.get("text")).and_then(Value::as_str) {
                Some(text) => format!("[resource: {uri}]\n{text}"),
                None => format!(
                    "[resource: {uri}, {}, about {} bytes]",
                    or_unknown(get("mimeType")),
                    get("blob").len() / 4 * 3
                ),
            }
        }
        other => format!(
            "[unsupported content: {}]",
            bounded_redacted(or_unknown(other), 32)
        ),
    }
}

fn or_unknown(text: &str) -> &str {
    if text.is_empty() { "unknown" } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_classified() {
        let response = classify(json!({"jsonrpc":"2.0","id":1,"result":{"ok":true}})).unwrap();
        assert!(matches!(
            response,
            Incoming::Response { outcome: Ok(_), .. }
        ));
        let error = classify(
            json!({"jsonrpc":"2.0","id":2,"error":{"code":-32602,"message":"Unknown tool"}}),
        )
        .unwrap();
        let Incoming::Response {
            outcome: Err(error),
            ..
        } = error
        else {
            panic!("expected error");
        };
        assert_eq!(error.kind, McpErrorKind::ServerError);
        assert!(error.message.contains("-32602"));
        assert!(matches!(
            classify(json!({"jsonrpc":"2.0","id":"s1","method":"ping"})).unwrap(),
            Incoming::Request { .. }
        ));
        assert!(matches!(
            classify(json!({"jsonrpc":"2.0","method":"notifications/tools/list_changed"})).unwrap(),
            Incoming::Notification { .. }
        ));
        assert!(classify(json!({"id":1,"result":{}})).is_err());
        assert!(classify(json!([1])).is_err());
    }

    #[test]
    fn versions_are_negotiated_strictly() {
        assert_eq!(
            negotiated_version(&json!({"protocolVersion":"2025-03-26"})).unwrap(),
            "2025-03-26"
        );
        assert!(negotiated_version(&json!({"protocolVersion":"1999-01-01"})).is_err());
        assert!(negotiated_version(&json!({})).is_err());
    }

    #[test]
    fn tool_pages_are_bounded_and_normalized() {
        let page = parse_tools_page(&json!({
            "tools": [
                {"name":"echo","description":"Echo","inputSchema":{"properties":{}}},
                {"name":"","inputSchema":{}},
                {"description":"no name"},
                {"name":"titled","title":"A title"}
            ],
            "nextCursor": "p2"
        }))
        .unwrap();
        assert_eq!(page.tools.len(), 2);
        assert_eq!(page.tools[0].input_schema["type"], "object");
        assert_eq!(page.tools[1].description, "A title");
        assert_eq!(page.next_cursor.as_deref(), Some("p2"));
        assert_eq!(page.notes.len(), 2);
        assert!(parse_tools_page(&json!({})).is_err());
    }

    #[test]
    fn tool_results_are_summarised_and_bounded() {
        let result = tool_result(
            &json!({"content":[
                {"type":"text","text":"hello"},
                {"type":"image","data":"aGVsbG8=","mimeType":"image/png"},
                {"type":"resource","resource":{"uri":"file:///a.txt","text":"body"}},
                {"type":"resource","resource":{"uri":"file:///b.bin","blob":"AAAA","mimeType":"application/octet-stream"}},
                {"type":"resource_link","uri":"file:///c","name":"c"},
                {"type":"mystery"}
            ],"isError":true}),
            MAX_RESULT_TEXT,
        )
        .unwrap();
        assert!(result.is_error && !result.truncated);
        assert!(result.content_text.starts_with("hello\n[image: image/png"));
        assert!(
            result
                .content_text
                .contains("[resource: file:///a.txt]\nbody")
        );
        assert!(result.content_text.contains("application/octet-stream"));
        assert!(result.content_text.contains("[resource link: file:///c c]"));
        assert!(
            result
                .content_text
                .contains("[unsupported content: mystery]")
        );
        let big = tool_result(
            &json!({"content":[{"type":"text","text":"x".repeat(100)}]}),
            10,
        )
        .unwrap();
        assert!(big.truncated && big.content_text.len() == 10);
        let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        let leaked = tool_result(
            &json!({"content":[{"type":"text","text":format!("key {token}")}]}),
            100,
        )
        .unwrap();
        assert!(!leaked.content_text.contains(token));
        let structured = tool_result(&json!({"structuredContent":{"a":1}}), 100).unwrap();
        assert_eq!(structured.content_text, r#"{"a":1}"#);
    }
}
