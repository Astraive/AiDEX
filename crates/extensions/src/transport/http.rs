use super::{ExtensionSecrets, protect_frame, protocol::Wire};
use crate::{ExtensionError, McpServerConfig, Result};
use aidex_tools::CancelToken;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Read},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

pub(crate) fn endpoint(url: &str) -> Result<(String, u16)> {
    if url.len() > 4096 || url.contains('#') {
        return Err(ExtensionError::Invalid("invalid MCP URL".into()));
    }
    let uri: ureq::http::Uri = url
        .parse()
        .map_err(|_| ExtensionError::Invalid("invalid MCP URL".into()))?;
    if uri.scheme_str() != Some("https") {
        return Err(ExtensionError::Invalid(
            "MCP HTTP endpoint must use HTTPS".into(),
        ));
    }
    let authority = uri
        .authority()
        .ok_or_else(|| ExtensionError::Invalid("MCP endpoint requires host".into()))?;
    if authority.as_str().contains('@') {
        return Err(ExtensionError::Invalid(
            "MCP URL must not contain credentials".into(),
        ));
    }
    let host = authority.host();
    if host.is_empty() {
        return Err(ExtensionError::Invalid("missing MCP host".into()));
    }
    Ok((
        host.to_ascii_lowercase(),
        authority.port_u16().unwrap_or(443),
    ))
}
pub(crate) struct HttpWire {
    url: String,
    config: McpServerConfig,
    secrets: Arc<dyn ExtensionSecrets>,
    session: Option<String>,
    protocol: Option<String>,
    agent: ureq::Agent,
    active: Arc<AtomicBool>,
    budget: Arc<AtomicUsize>,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl HttpWire {
    pub fn new(
        url: &str,
        config: McpServerConfig,
        secrets: Arc<dyn ExtensionSecrets>,
        budget: Arc<AtomicUsize>,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self> {
        endpoint(url)?;
        let timeout = Duration::from_millis(config.timeout_ms);
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(None)
            .timeout_global(Some(timeout))
            .timeout_connect(Some(timeout.min(Duration::from_secs(10))))
            .build()
            .new_agent();
        Ok(Self {
            url: url.into(),
            config,
            secrets,
            session: None,
            protocol: None,
            agent,
            active: Arc::new(AtomicBool::new(false)),
            budget,
            gate,
        })
    }
}
impl Wire for HttpWire {
    fn exchange(&mut self, message: Value, cancel: &CancelToken) -> Result<Option<Value>> {
        admit(&self.gate, cancel)?;
        if self.active.swap(true, Ordering::SeqCst) {
            return Err(ExtensionError::Unavailable(
                "previous HTTP request is still closing".into(),
            ));
        }
        if !reserve(&self.budget) {
            self.active.store(false, Ordering::SeqCst);
            return Err(ExtensionError::Unavailable(
                "MCP HTTP worker limit reached".into(),
            ));
        }
        struct Lease {
            active: Arc<AtomicBool>,
            budget: Arc<AtomicUsize>,
        }
        impl Drop for Lease {
            fn drop(&mut self) {
                self.active.store(false, Ordering::SeqCst);
                self.budget.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let lease = Lease {
            active: self.active.clone(),
            budget: self.budget.clone(),
        };
        let mut protected = Vec::new();
        let mut headers = Vec::new();
        for (name, reference) in &self.config.secret_refs {
            admit(&self.gate, cancel)?;
            match self.secrets.resolve(&self.config.id, reference) {
                Ok(value) => {
                    let text = match std::str::from_utf8(value.expose()) {
                        Ok(text)
                            if !text.is_empty() && text.bytes().all(|b| (32..127).contains(&b)) =>
                        {
                            text
                        }
                        _ => {
                            self.active.store(false, Ordering::SeqCst);
                            return Err(ExtensionError::Invalid(
                                "credential is not a valid HTTP header".into(),
                            ));
                        }
                    };
                    headers.push((name.clone(), zeroize::Zeroizing::new(text.to_owned())));
                    protected.push(value);
                }
                Err(e) => {
                    self.active.store(false, Ordering::SeqCst);
                    return Err(e);
                }
            }
        }
        let body = serde_json::to_vec(&message)
            .map_err(|_| ExtensionError::Protocol("cannot encode request".into()))?;
        if body.len() > self.config.max_output_bytes {
            self.active.store(false, Ordering::SeqCst);
            return Err(ExtensionError::TooLarge);
        }
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let url = self.url.clone();
        let session = self.session.clone();
        let protocol = self.protocol.clone();
        let max = self.config.max_output_bytes;
        let agent = self.agent.clone();
        let admission = CancelToken::new();
        let worker_cancel = admission.clone();
        let gate = self.gate.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        if std::thread::Builder::new()
            .name("aidex-mcp-http".into())
            .spawn(move || {
                let _lease = lease;
                let result = (|| {
                    admit(&gate, &worker_cancel)?;
                    let mut request = agent
                        .post(&url)
                        .header("Accept", "application/json, text/event-stream")
                        .header("Content-Type", "application/json");
                    if let Some(session) = session {
                        request = request.header("Mcp-Session-Id", session)
                    }
                    if let Some(protocol) = protocol {
                        request = request.header("MCP-Protocol-Version", protocol)
                    }
                    for (name, value) in &headers {
                        request = request.header(name, value.as_str())
                    }
                    admit(&gate, &worker_cancel)?;
                    let mut response = request.send(&body[..]).map_err(|_| {
                        ExtensionError::Unavailable("MCP HTTPS exchange failed".into())
                    })?;
                    admit(&gate, &worker_cancel)?;
                    if !(200..300).contains(&response.status().as_u16()) {
                        return Err(ExtensionError::Unavailable(
                            "MCP endpoint rejected the HTTP request".into(),
                        ));
                    }
                    let session = response
                        .headers()
                        .get("Mcp-Session-Id")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    if session.as_ref().is_some_and(|s| {
                        s.is_empty() || s.len() > 1024 || !s.bytes().all(|b| b.is_ascii_graphic())
                    }) {
                        return Err(ExtensionError::Protocol("invalid MCP session id".into()));
                    }
                    let Some(id) = id else {
                        if response.status().as_u16() != 202 {
                            return Err(ExtensionError::Protocol(
                                "notification did not receive HTTP 202".into(),
                            ));
                        }
                        return Ok((None, session));
                    };
                    let content = response
                        .headers()
                        .get("Content-Type")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    let mut frame = if content.starts_with("application/json") {
                        let mut bytes = Vec::new();
                        let mut reader = response.body_mut().as_reader();
                        let mut chunk = [0u8; 8192];
                        loop {
                            admit(&gate, &worker_cancel)?;
                            let count = reader.read(&mut chunk)?;
                            if count == 0 {
                                break;
                            }
                            if bytes.len() + count > max {
                                return Err(ExtensionError::TooLarge);
                            }
                            bytes.extend_from_slice(&chunk[..count]);
                        }
                        serde_json::from_slice::<Value>(&bytes)
                            .map_err(|_| ExtensionError::Protocol("invalid JSON response".into()))?
                    } else if content.starts_with("text/event-stream") {
                        parse_sse_checked(response.body_mut().as_reader(), &id, max, || {
                            admit(&gate, &worker_cancel)
                        })?
                    } else {
                        return Err(ExtensionError::Protocol(
                            "unsupported MCP response content type".into(),
                        ));
                    };
                    admit(&gate, &worker_cancel)?;
                    protect_frame(&mut frame, &protected, method.as_deref())?;
                    Ok((Some(frame), session))
                })();
                let _ = tx.send(result);
            })
            .is_err()
        {
            self.active.store(false, Ordering::SeqCst);
            return Err(ExtensionError::Unavailable(
                "cannot start bounded HTTP worker".into(),
            ));
        }
        let start = Instant::now();
        loop {
            if cancel.is_cancelled() {
                admission.cancel();
                return Err(ExtensionError::Cancelled);
            }
            if !(self.gate)() {
                admission.cancel();
                return Err(ExtensionError::Forbidden("network access is denied".into()));
            }
            if start.elapsed() > Duration::from_millis(self.config.timeout_ms) {
                admission.cancel();
                return Err(ExtensionError::Timeout);
            }
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => {
                    admit(&self.gate, cancel)?;
                    let (frame, session) = result?;
                    if session.is_some() {
                        self.session = session;
                    }
                    return Ok(frame);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err(ExtensionError::Unavailable("MCP HTTP worker closed".into())),
            }
        }
    }
    fn set_protocol(&mut self, version: &str) {
        self.protocol = Some(version.into())
    }
    fn disconnect(&mut self) {
        self.session = None;
        self.protocol = None;
    }
}
fn admit(gate: &Arc<dyn Fn() -> bool + Send + Sync>, cancel: &CancelToken) -> Result<()> {
    if cancel.is_cancelled() {
        return Err(ExtensionError::Cancelled);
    }
    if !gate() {
        return Err(ExtensionError::Forbidden("network access is denied".into()));
    }
    Ok(())
}
fn reserve(budget: &AtomicUsize) -> bool {
    let mut count = budget.load(Ordering::SeqCst);
    loop {
        if count >= 16 {
            return false;
        }
        match budget.compare_exchange(count, count + 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(current) => count = current,
        }
    }
}
#[cfg(test)]
fn parse_sse(reader: impl Read, id: &Value, max: usize) -> Result<Value> {
    parse_sse_checked(reader, id, max, || Ok(()))
}
fn parse_sse_checked(
    reader: impl Read,
    id: &Value,
    max: usize,
    check: impl Fn() -> Result<()>,
) -> Result<Value> {
    let mut reader = BufReader::new(reader.take(max as u64 + 1));
    let mut total = 0;
    let mut data = String::new();
    let mut lines = 0;
    loop {
        check()?;
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        total += n;
        lines += 1;
        if total > max || lines > 4096 {
            return Err(ExtensionError::TooLarge);
        }
        if n == 0 {
            return Err(ExtensionError::Protocol(
                "SSE ended without matching response".into(),
            ));
        }
        if line.trim_end().is_empty() {
            if data.is_empty() {
                continue;
            }
            let frame: Value = serde_json::from_str(&data)
                .map_err(|_| ExtensionError::Protocol("invalid SSE JSON-RPC frame".into()))?;
            data.clear();
            if frame.get("id") == Some(id) && frame.get("method").is_none() {
                return Ok(frame);
            }
            if frame.get("method").and_then(Value::as_str)
                == Some("notifications/tools/list_changed")
            {
                return Err(ExtensionError::Conflict("MCP tool catalog changed".into()));
            }
            if frame.get("id").is_some() {
                return Err(ExtensionError::Protocol(
                    "unsolicited server capability request".into(),
                ));
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n')
            }
            data.push_str(value.trim_start().trim_end());
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
        sync::{atomic::AtomicBool, mpsc},
    };
    fn fixture_wire(listener: &TcpListener, online: Arc<AtomicBool>) -> HttpWire {
        let gate: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || online.load(Ordering::SeqCst));
        let config = McpServerConfig {
            id: "fixture".into(),
            name: "Fixture".into(),
            enabled: true,
            trusted: true,
            workspace_ids: vec![],
            profiles: vec![],
            transport: crate::McpTransport::Https {
                url: "https://example.com/mcp".into(),
            },
            secret_refs: BTreeMap::new(),
            timeout_ms: 1500,
            max_output_bytes: 4096,
        };
        let mut wire = HttpWire::new(
            "https://example.com/mcp",
            config,
            Arc::new(crate::NoSecrets),
            Arc::new(AtomicUsize::new(0)),
            gate,
        )
        .unwrap();
        // Loopback plaintext is a unit-fixture seam only; production new() rejects it.
        wire.url = format!("http://{}/mcp", listener.local_addr().unwrap());
        wire
    }
    fn request_read(stream: &mut std::net::TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut header = Vec::new();
        let mut one = [0];
        while !header.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut one).unwrap();
            header.push(one[0]);
            assert!(header.len() < 8192);
        }
        let text = String::from_utf8(header).unwrap();
        let length = text
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        stream.read_exact(&mut vec![0; length]).unwrap();
    }
    fn reply(stream: &mut std::net::TcpStream) {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
    }
    #[test]
    fn live_gate_blocks_subsequent_http_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let online = Arc::new(AtomicBool::new(true));
        let mut wire = fixture_wire(&listener, online.clone());
        let server = listener.try_clone().unwrap();
        let fixture = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            request_read(&mut stream);
            reply(&mut stream);
        });
        wire.exchange(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
            &CancelToken::new(),
        )
        .unwrap();
        fixture.join().unwrap();
        online.store(false, Ordering::SeqCst);
        assert!(matches!(
            wire.exchange(
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                &CancelToken::new()
            ),
            Err(ExtensionError::Forbidden(_))
        ));
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    #[test]
    fn offline_change_stops_waiting_for_a_blocked_http_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let online = Arc::new(AtomicBool::new(true));
        let mut wire = fixture_wire(&listener, online.clone());
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let fixture = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            request_read(&mut stream);
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            reply(&mut stream);
        });
        let caller = std::thread::spawn(move || {
            result_tx
                .send(wire.exchange(
                    json!({"jsonrpc":"2.0","id":1,"method":"initialize"}),
                    &CancelToken::new(),
                ))
                .unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        online.store(false, Ordering::SeqCst);
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err(ExtensionError::Forbidden(_))
        ));
        release_tx.send(()).unwrap();
        caller.join().unwrap();
        fixture.join().unwrap();
    }
    #[test]
    fn sse_handles_notifications_and_multiline_events() {
        let body=b"data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n\ndata: {\"jsonrpc\":\"2.0\",\ndata: \"id\":7,\"result\":{}}\n\n";
        assert_eq!(parse_sse(&body[..], &json!(7), 1024).unwrap()["id"], 7);
        let body=b"data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}\n\ndata: {\"jsonrpc\":\"2.0\",\ndata: invalid\n\n";
        assert!(parse_sse(&body[..], &json!(7), 1024).is_err());
        assert!(endpoint("http://localhost/mcp").is_err());
        assert!(endpoint("https://user:password@example.com/mcp").is_err());
    }
}
