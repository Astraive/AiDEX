//! Streamable HTTP against a loopback fake server: JSON and SSE responses,
//! session and protocol headers, credential headers from secrets, scrubbing,
//! and the egress gate.

mod support;

use std::{sync::Arc, time::Duration};

use aidex_core::CancelToken;
use aidex_mcp::{
    EndpointUrl, HeaderSecret, HttpEndpoint, LogBuffer, McpClient, McpConfigStore, McpErrorKind,
    McpSource, McpStatus, McpSupervisor, OpenGate, SandboxHost, ServerSpec, Timeouts,
    TransportKind,
};
use aidex_sandbox::Sandbox;
use serde_json::{Value, json};
use support::{
    OfflineGate, StaticSecrets,
    http::{FakeServer, RecordedRequest, Reply},
    sentinel,
};
use zeroize::Zeroizing;

const SESSION: &str = "session-7f3a";
const VERSION: &str = "2025-03-26";

fn cancel() -> CancelToken {
    CancelToken::new().with_timeout(Duration::from_secs(30))
}

fn response(id: &Value, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

fn text(value: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": value}], "isError": is_error})
}

/// A scripted MCP server that requires `Authorization: Bearer <token>` and,
/// after `initialize`, the session and protocol-version headers.
fn mcp_server(token: String) -> FakeServer {
    FakeServer::start(move |request: &RecordedRequest| {
        if request.method == "DELETE" {
            return Reply::Empty(200);
        }
        if request.header("authorization") != Some(format!("Bearer {token}").as_str()) {
            return Reply::Json(401, vec![], r#"{"error":"unauthorized"}"#.into());
        }
        let body = request.json();
        let id = body.get("id").cloned().unwrap_or(Value::Null);
        let method = body["method"].as_str().unwrap_or_default().to_owned();
        if method != "initialize"
            && (request.header("mcp-session-id") != Some(SESSION)
                || request.header("mcp-protocol-version") != Some(VERSION))
        {
            return Reply::Json(400, vec![], r#"{"error":"missing session"}"#.into());
        }
        match method.as_str() {
            "initialize" => Reply::Json(
                200,
                vec![("Mcp-Session-Id".into(), SESSION.into())],
                response(
                    &id,
                    json!({
                        "protocolVersion": VERSION,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "remote", "version": "2.0"},
                    }),
                ),
            ),
            "notifications/initialized" | "notifications/cancelled" => Reply::Empty(202),
            "tools/list" if body["params"]["cursor"].is_null() => Reply::Sse(
                vec![],
                vec![
                    json!({"jsonrpc": "2.0", "method": "notifications/message",
                           "params": {"level": "info", "data": "listing tools"}})
                    .to_string(),
                    response(
                        &id,
                        json!({"tools": [
                            {"name": "echo", "description": "Echo", "inputSchema": {"type": "object"}},
                            {"name": "leak", "inputSchema": {"type": "object"}},
                        ], "nextCursor": "page-2"}),
                    ),
                ],
            ),
            "tools/list" => Reply::Json(
                200,
                vec![],
                response(
                    &id,
                    json!({"tools": [
                        {"name": "fail", "inputSchema": {"type": "object"}},
                        {"name": "expire", "inputSchema": {"type": "object"}},
                    ]}),
                ),
            ),
            "tools/call" => match body["params"]["name"].as_str().unwrap_or_default() {
                "echo" => Reply::Sse(
                    vec![],
                    vec![response(
                        &id,
                        text(body["params"]["arguments"]["text"].as_str().unwrap(), false),
                    )],
                ),
                "leak" => Reply::Json(
                    200,
                    vec![],
                    response(&id, text(&format!("the token is {token}"), false)),
                ),
                "fail" => Reply::Json(200, vec![], response(&id, text("remote failure", true))),
                "expire" => Reply::Json(404, vec![], "{}".into()),
                _ => Reply::Json(
                    200,
                    vec![],
                    json!({"jsonrpc": "2.0", "id": id,
                           "error": {"code": -32602, "message": "Unknown tool"}})
                    .to_string(),
                ),
            },
            "ping" => Reply::Json(200, vec![], response(&id, json!({}))),
            _ => Reply::Json(
                200,
                vec![],
                json!({"jsonrpc": "2.0", "id": id,
                       "error": {"code": -32601, "message": "Method not found"}})
                .to_string(),
            ),
        }
    })
}

fn endpoint(server: &FakeServer, token: &str) -> HttpEndpoint {
    HttpEndpoint {
        url: EndpointUrl::parse(&server.url()).unwrap(),
        headers: vec![(
            "Authorization".into(),
            Zeroizing::new(format!("Bearer {token}")),
        )],
        egress: None,
    }
}

#[test]
fn the_client_speaks_streamable_http() {
    let token = sentinel();
    let server = mcp_server(token.clone());
    let logs = Arc::new(LogBuffer::new());
    let mut client = McpClient::http(
        endpoint(&server, &token),
        Arc::clone(&logs),
        Timeouts::default(),
    );
    assert!(!format!("{:?}", endpoint(&server, &token)).contains(&token));

    let identity = client.initialize(&cancel()).unwrap();
    assert_eq!(
        (identity.name.as_str(), identity.protocol_version.as_str()),
        ("remote", VERSION)
    );
    let listing = client.list_tools(&cancel()).unwrap();
    let names: Vec<&str> = listing.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo", "leak", "fail", "expire"]);

    let echo = client
        .call_tool("echo", json!({"text": "over sse"}), &cancel())
        .unwrap();
    assert_eq!(echo.content_text, "over sse");
    let failed = client.call_tool("fail", json!({}), &cancel()).unwrap();
    assert!(failed.is_error);
    let leak = client.call_tool("leak", json!({}), &cancel()).unwrap();
    assert!(!leak.content_text.contains(&token));
    assert!(leak.content_text.contains("[REDACTED]"));
    let unknown = client.call_tool("other", json!({}), &cancel()).unwrap_err();
    assert_eq!(unknown.kind, McpErrorKind::ServerError);
    client.ping(&cancel()).unwrap();

    let expired = client
        .call_tool("expire", json!({}), &cancel())
        .unwrap_err();
    assert_eq!(expired.kind, McpErrorKind::SessionExpired);

    client.close();
    let delete = server.wait_for(|request| request.method == "DELETE");
    assert_eq!(delete.header("mcp-session-id"), Some(SESSION));

    let requests = server.requests();
    let first = &requests[0];
    assert_eq!(first.rpc_method().as_deref(), Some("initialize"));
    assert_eq!(first.json()["params"]["clientInfo"]["name"], "AiDEX");
    assert!(first.header("mcp-session-id").is_none());
    assert!(
        first
            .header("accept")
            .unwrap()
            .contains("text/event-stream")
    );
    assert!(
        first
            .header("content-type")
            .unwrap()
            .starts_with("application/json")
    );
    assert!(requests.iter().all(|r| r.path == "/mcp"));

    let text: Vec<String> = logs.tail(100).lines.into_iter().map(|l| l.text).collect();
    assert!(
        text.iter().any(|line| line == "info: listing tools"),
        "{text:?}"
    );
    assert!(text.iter().all(|line| !line.contains(&token)));
}

#[test]
fn refused_credentials_need_auth() {
    let server = mcp_server(sentinel());
    let mut client = McpClient::http(
        endpoint(&server, "wrong-credential"),
        Arc::new(LogBuffer::new()),
        Timeouts::default(),
    );
    let error = client.initialize(&cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::NeedsAuth);
    assert!(error.needs_auth());
}

fn http_spec(url: String) -> ServerSpec {
    ServerSpec {
        id: "remote".into(),
        name: Some("Remote".into()),
        transport: TransportKind::Http,
        command: None,
        args: vec![],
        env_allowlist: vec![],
        url: Some(url),
        headers_from_secrets: vec![HeaderSecret {
            header: "Authorization".into(),
            secret_name: "remote-token".into(),
        }],
        env_from_secrets: vec![],
        enabled: true,
        workspaces: vec![],
        auto_approve_tools: vec!["echo".into()],
    }
}

fn http_supervisor(
    dir: &std::path::Path,
    secrets: StaticSecrets,
    gate: Arc<dyn aidex_mcp::McpGate>,
) -> McpSupervisor {
    let host = SandboxHost::new(
        Sandbox::new(dir.join("unused-helper")),
        dir.join("scratch"),
        Arc::new(secrets),
        gate,
    );
    McpSupervisor::new(McpConfigStore::in_dir(dir.join("mcp")), Arc::new(host))
}

#[test]
fn the_supervisor_resolves_header_secrets_and_ends_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let token = sentinel();
    let server = mcp_server(token.clone());
    let secrets =
        StaticSecrets::default().with("remote-token", "mcp:remote", &format!("Bearer {token}"));
    let supervisor = http_supervisor(dir.path(), secrets, Arc::new(OpenGate));
    supervisor
        .store()
        .add(&http_spec(server.url()).into_config(McpSource::drive()))
        .unwrap();

    let state = supervisor.start("remote", &cancel()).unwrap();
    assert_eq!(state.status, McpStatus::Running);
    assert_eq!(state.tools.len(), 4);
    assert!(state.process_id.is_none());
    let echo = supervisor
        .call_tool("remote", "echo", json!({"text": "hi"}), &cancel())
        .unwrap();
    assert_eq!(echo.content_text, "hi");
    let toolsets = supervisor.running_toolsets(None).unwrap();
    assert_eq!(toolsets[0].auto_approve_tools, ["echo"]);

    // A forgotten session fails the server until it is restarted.
    let expired = supervisor
        .call_tool("remote", "expire", json!({}), &cancel())
        .unwrap_err();
    assert_eq!(expired.kind, McpErrorKind::SessionExpired);
    assert_eq!(supervisor.state("remote").status, McpStatus::Failed);
    let restarted = supervisor.restart("remote", &cancel()).unwrap();
    assert_eq!(restarted.status, McpStatus::Running);

    supervisor.stop("remote");
    server.wait_for(|request| request.method == "DELETE");
    let logs = supervisor.logs("remote", 1000);
    assert!(logs.lines.iter().all(|line| !line.text.contains(&token)));
}

#[test]
fn the_egress_gate_and_missing_secrets_refuse_before_connecting() {
    let dir = tempfile::tempdir().unwrap();
    let server = mcp_server(sentinel());
    let offline = http_supervisor(
        dir.path(),
        StaticSecrets::default().with("remote-token", "mcp:remote", "Bearer x"),
        Arc::new(OfflineGate),
    );
    offline
        .store()
        .add(&http_spec(server.url()).into_config(McpSource::drive()))
        .unwrap();
    let error = offline.start("remote", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::OfflineRefused);
    assert_eq!(offline.state("remote").status, McpStatus::Failed);

    let unbound = http_supervisor(
        dir.path(),
        StaticSecrets::default().with("remote-token", "mcp:other", "Bearer x"),
        Arc::new(OpenGate),
    );
    let error = unbound.start("remote", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::NeedsAuth);
    assert_eq!(unbound.state("remote").status, McpStatus::NeedsAuth);
    assert_eq!(server.connections(), 0);
}
