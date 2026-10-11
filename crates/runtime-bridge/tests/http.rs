use aidex_core::CancelToken;
use aidex_runtime_bridge::*;
use serde_json::{Value, json};
use std::{
    io::{ErrorKind, Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
struct Dispatch {
    active: AtomicBool,
    effects: AtomicUsize,
}
impl BridgeDispatcher for Dispatch {
    fn active(&self, scope: &BridgeScope) -> bool {
        self.active.load(Ordering::SeqCst)
            && scope.session_id == "session_one"
            && scope.run_id == "run_one"
    }
    fn tools(&self, _: &BridgeScope) -> Result<Vec<BridgeTool>, BridgeError> {
        Ok(vec![BridgeTool {
            name: "web_fetch".into(),
            description: "fixture".into(),
            input_schema: json!({"type":"object","properties":{"url":{"type":"string"}},"required":["url"],"additionalProperties":false}),
            annotations: json!({"readOnlyHint":true}),
        }])
    }
    fn call(
        &self,
        scope: &BridgeScope,
        _: &str,
        name: &str,
        arguments: Value,
        _: &CancelToken,
    ) -> Result<Value, BridgeError> {
        assert!(self.active(scope));
        assert_eq!(name, "web_fetch");
        self.effects.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"content":arguments}))
    }
}
fn request(
    connection: &BridgeConnection,
    body: Value,
    auth: bool,
    origin: Option<&str>,
) -> (u16, Value) {
    let target = connection.endpoint().strip_prefix("http://").unwrap();
    let (address, path) = target.split_once('/').unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let bytes = body.to_string();
    let authorization = if auth {
        format!("Authorization: {}\r\n", connection.authorization_header())
    } else {
        String::new()
    };
    let origin = origin
        .map(|value| format!("Origin: {value}\r\n"))
        .unwrap_or_default();
    write!(stream,"POST /{path} HTTP/1.1\r\nHost: {address}\r\n{authorization}{origin}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-06-18\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bytes}",bytes.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    (
        headers.split_whitespace().nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap_or(Value::Null),
    )
}
fn initialize(connection: &BridgeConnection) {
    let (status, result) = request(
        connection,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
        true,
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(result["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        request(
            connection,
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            true,
            None
        )
        .0,
        202
    );
}
#[test]
fn fragmented_request_waits_for_the_declared_body_before_dispatch() {
    let server = BridgeServer::bind().unwrap();
    let dispatch = Arc::new(Dispatch {
        active: AtomicBool::new(true),
        effects: AtomicUsize::new(0),
    });
    let lease = server
        .lease(
            BridgeScope {
                session_id: "session_one".into(),
                run_id: "run_one".into(),
            },
            dispatch.clone(),
            CancelToken::new(),
            Duration::from_secs(60),
        )
        .unwrap();
    let connection = lease.connection();
    initialize(&connection);
    let target = connection.endpoint().strip_prefix("http://").unwrap();
    let (address, path) = target.split_once('/').unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let body = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{"url":"https://example.com"}}}).to_string();
    write!(
        stream,
        "POST /{path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        connection.authorization_header(),
        body.len()
    )
    .unwrap();
    let mut first_byte = [0; 1];
    let error = stream
        .read(&mut first_byte)
        .expect_err("bridge responded before the declared request body arrived");
    assert!(matches!(
        error.kind(),
        ErrorKind::WouldBlock | ErrorKind::TimedOut
    ));
    assert_eq!(dispatch.effects.load(Ordering::SeqCst), 0);
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(body.as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    assert_eq!(headers.split_whitespace().nth(1), Some("200"));
    let result: Value = serde_json::from_str(body).unwrap();
    assert_eq!(result["result"]["isError"], false);
    assert_eq!(dispatch.effects.load(Ordering::SeqCst), 1);
}
#[test]
fn authenticated_loopback_tools_are_scoped_and_duplicate_effects_are_not_replayed() {
    let server = BridgeServer::bind().unwrap();
    let dispatch = Arc::new(Dispatch {
        active: AtomicBool::new(true),
        effects: AtomicUsize::new(0),
    });
    let lease = server
        .lease(
            BridgeScope {
                session_id: "session_one".into(),
                run_id: "run_one".into(),
            },
            dispatch.clone(),
            CancelToken::new(),
            Duration::from_secs(60),
        )
        .unwrap();
    let connection = lease.connection();
    assert!(!format!("{connection:?}").contains(&connection.authorization_header()));
    assert_eq!(
        request(
            &connection,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
            false,
            None
        )
        .0,
        401
    );
    assert_eq!(
        request(
            &connection,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
            true,
            Some("https://evil.example")
        )
        .0,
        403
    );
    initialize(&connection);
    let (_, catalog) = request(
        &connection,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        true,
        None,
    );
    assert_eq!(catalog["result"]["tools"][0]["name"], "web_fetch");
    let call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{"url":"https://example.com"}}});
    let first = request(&connection, call.clone(), true, None);
    assert_eq!(first.1["result"]["isError"], false);
    assert_eq!(request(&connection, call, true, None), first);
    assert_eq!(dispatch.effects.load(Ordering::SeqCst), 1);
    let changed = request(
        &connection,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{"url":"https://different.example"}}}),
        true,
        None,
    );
    assert!(changed.1.get("error").is_some());
    assert_eq!(dispatch.effects.load(Ordering::SeqCst), 1);
    drop(lease);
    assert_eq!(
        request(
            &connection,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
            true,
            None
        )
        .0,
        401
    );
}
#[test]
fn inactive_scope_and_unadvertised_tools_cannot_execute() {
    let server = BridgeServer::bind().unwrap();
    let dispatch = Arc::new(Dispatch {
        active: AtomicBool::new(true),
        effects: AtomicUsize::new(0),
    });
    let lease = server
        .lease(
            BridgeScope {
                session_id: "session_one".into(),
                run_id: "run_one".into(),
            },
            dispatch.clone(),
            CancelToken::new(),
            Duration::from_secs(60),
        )
        .unwrap();
    let connection = lease.connection();
    initialize(&connection);
    let result = request(
        &connection,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"vault_dump","arguments":{}}}),
        true,
        None,
    );
    assert!(result.1.get("error").is_some());
    assert_eq!(dispatch.effects.load(Ordering::SeqCst), 0);
    dispatch.active.store(false, Ordering::SeqCst);
    assert_eq!(
        request(
            &connection,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}),
            true,
            None
        )
        .0,
        403
    );
}

#[test]
fn bounded_protocol_identity_and_sdk_metadata_are_enforced() {
    let server = BridgeServer::bind().unwrap();
    let dispatch = Arc::new(Dispatch {
        active: AtomicBool::new(true),
        effects: AtomicUsize::new(0),
    });
    let lease = server
        .lease(
            BridgeScope {
                session_id: "session_one".into(),
                run_id: "run_one".into(),
            },
            dispatch.clone(),
            CancelToken::new(),
            Duration::from_secs(60),
        )
        .unwrap();
    let connection = lease.connection();
    let (_, result) = request(
        &connection,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"},"_meta":{"progressToken":"setup"}}}),
        true,
        None,
    );
    assert!(result["result"].is_object());
    request(
        &connection,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        true,
        None,
    );
    let (_, result) = request(
        &connection,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"cursor":null,"_meta":{"progressToken":1}}}),
        true,
        None,
    );
    assert!(result["result"]["tools"].is_array());
    let long = "sensitive-marker".repeat(100);
    let (status, result) = request(
        &connection,
        json!({"jsonrpc":"2.0","id":long,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
        true,
        None,
    );
    assert_eq!(status, 400);
    assert_eq!(result["id"], Value::Null);
    assert!(!result.to_string().contains("sensitive-marker"));
    assert_eq!(dispatch.effects.load(Ordering::SeqCst), 0);
    let (_, result) = request(
        &connection,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{"cursor":"invented-page"}}),
        true,
        None,
    );
    assert!(result.get("error").is_some());
}

struct DeadlineDispatch;
impl BridgeDispatcher for DeadlineDispatch {
    fn active(&self, _: &BridgeScope) -> bool {
        true
    }
    fn tools(&self, _: &BridgeScope) -> Result<Vec<BridgeTool>, BridgeError> {
        Ok(vec![BridgeTool {
            name: "web_fetch".into(),
            description: "deadline fixture".into(),
            input_schema: json!({"type":"object"}),
            annotations: json!({}),
        }])
    }
    fn call(
        &self,
        _: &BridgeScope,
        _: &str,
        _: &str,
        _: Value,
        cancel: &CancelToken,
    ) -> Result<Value, BridgeError> {
        while !cancel.is_stopped() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(BridgeError::new(
            "expired",
            "call expired before any effect",
        ))
    }
}
#[test]
fn admitted_call_is_cancelled_at_lease_deadline() {
    let server = BridgeServer::bind().unwrap();
    let lease = server
        .lease(
            BridgeScope {
                session_id: "session_one".into(),
                run_id: "run_one".into(),
            },
            Arc::new(DeadlineDispatch),
            CancelToken::new(),
            Duration::from_millis(500),
        )
        .unwrap();
    let connection = lease.connection();
    initialize(&connection);
    let started = std::time::Instant::now();
    let (_, result) = request(
        &connection,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
        true,
        None,
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(result["result"]["structuredContent"]["code"], "expired");
}
