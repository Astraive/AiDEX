//! MCP over Streamable HTTP: every message is a POST of one JSON-RPC object;
//! a request is answered with `application/json` or a `text/event-stream` that
//! may carry server notifications and requests before the response.
//! Notifications and replies are answered with 202. `Mcp-Session-Id` and
//! `MCP-Protocol-Version` are sent once known; the session is ended with a
//! best-effort DELETE. Redirects are never followed (they could carry the
//! credential headers elsewhere). The server-initiated GET stream is not opened.

use std::{
    fmt,
    io::{BufReader, Read},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel},
    },
    thread,
    time::Duration,
};

use aidex_core::CancelToken;
use serde_json::Value;
use zeroize::Zeroizing;

use super::{
    Channel, cancelled_error,
    shared::{Routed, Shared, id_matches, route},
    sse::read_events,
};
use crate::{
    EndpointUrl, McpError, McpErrorKind,
    protocol::is_request,
    redact::{Scrubber, bounded_redacted},
};

const POLL: Duration = Duration::from_millis(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest JSON response body or SSE event.
const MAX_MESSAGE: usize = 1024 * 1024;
/// Error bodies are read only this far.
const MAX_ERROR_BODY: u64 = 64 * 1024;
const MAX_SESSION_ID: usize = 256;

/// Checked before every POST (the offline switch and egress policy).
pub type EgressCheck = Arc<dyn Fn() -> Result<(), McpError> + Send + Sync>;

/// A remote server and the credential headers resolved for it.
pub struct HttpEndpoint {
    pub url: EndpointUrl,
    /// Header name and resolved value; values are zeroized on drop.
    pub headers: Vec<(String, Zeroizing<String>)>,
    pub egress: Option<EgressCheck>,
}

impl fmt::Debug for HttpEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("HttpEndpoint")
            .field("url", &self.url.origin())
            .field("headers", &format_args!("{names:?} (values redacted)"))
            .finish()
    }
}

/// What a POST worker reports.
enum Event {
    Head {
        status: u16,
        session: Option<String>,
    },
    Message(Value),
    End,
    Failed(McpError),
}

#[derive(Clone)]
struct Connection {
    agent: ureq::Agent,
    url: String,
    headers: Arc<Vec<(String, Zeroizing<String>)>>,
    session: Arc<Mutex<Option<String>>>,
    version: Arc<Mutex<Option<String>>>,
    scrubber: Scrubber,
}

pub(crate) struct HttpChannel {
    connection: Connection,
    shared: Arc<Shared>,
    egress: Option<EgressCheck>,
    exchange: Option<Receiver<Event>>,
}

impl HttpChannel {
    pub fn new(endpoint: HttpEndpoint, shared: Arc<Shared>) -> Self {
        let proxy = if endpoint.url.is_loopback() {
            None
        } else {
            ureq::Proxy::try_from_env()
        };
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(proxy)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(RESPONSE_TIMEOUT))
            .timeout_recv_body(Some(RESPONSE_TIMEOUT))
            .user_agent(concat!("aidex/", env!("CARGO_PKG_VERSION")))
            .build();
        let scrubber = Scrubber::new(
            endpoint
                .headers
                .iter()
                .map(|(_, value)| value.clone())
                .collect(),
        );
        Self {
            connection: Connection {
                agent: config.into(),
                url: endpoint.url.as_url(),
                headers: Arc::new(endpoint.headers),
                session: Arc::new(Mutex::new(None)),
                version: Arc::new(Mutex::new(None)),
                scrubber,
            },
            shared,
            egress: endpoint.egress,
            exchange: None,
        }
    }

    fn post(&self, message: &Value) -> Result<Receiver<Event>, McpError> {
        if let Some(egress) = &self.egress {
            egress()?;
        }
        let body = serde_json::to_vec(message)
            .map_err(|_| McpError::protocol("cannot encode the MCP message"))?;
        let (sender, receiver) = sync_channel(64);
        let connection = self.connection.clone();
        let shared = Arc::clone(&self.shared);
        thread::Builder::new()
            .name("aidex-mcp-http".into())
            .spawn(move || run_post(&connection, &shared, &body, &sender))
            .map_err(|_| {
                McpError::new(
                    McpErrorKind::Unavailable,
                    "cannot start an MCP HTTP worker thread",
                )
            })?;
        Ok(receiver)
    }

    fn record_session(&self, session: Option<String>) -> Result<(), McpError> {
        let Some(session) = session else {
            return Ok(());
        };
        if session.is_empty()
            || session.len() > MAX_SESSION_ID
            || !session.bytes().all(|b| (0x21..=0x7e).contains(&b))
        {
            return Err(McpError::protocol(
                "the server sent an invalid Mcp-Session-Id",
            ));
        }
        *lock(&self.connection.session) = Some(session);
        Ok(())
    }

    fn next_event(
        &self,
        receiver: &Receiver<Event>,
        cancel: &CancelToken,
    ) -> Result<Event, McpError> {
        loop {
            if let Some(error) = cancelled_error(cancel, &self.shared) {
                return Err(error);
            }
            let wait = cancel.remaining().map_or(POLL, |left| left.min(POLL));
            match receiver.recv_timeout(wait.max(Duration::from_millis(1))) {
                Ok(event) => return Ok(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(McpError::new(
                        McpErrorKind::Disconnected,
                        "the HTTP exchange ended unexpectedly",
                    ));
                }
            }
        }
    }

    /// Send a reply to a server request without waiting for it.
    fn reply(&self, reply: &Value) {
        let _ = self.post(reply);
    }

    fn status_error(&self, status: u16) -> McpError {
        match status {
            401 | 403 => McpError::new(
                McpErrorKind::NeedsAuth,
                format!("the MCP server refused the credentials (HTTP {status})"),
            ),
            404 if lock(&self.connection.session).is_some() => McpError::new(
                McpErrorKind::SessionExpired,
                "the MCP server ended the session (HTTP 404); restart the server",
            ),
            _ => McpError::new(
                McpErrorKind::Unavailable,
                format!("the MCP server answered HTTP {status}"),
            ),
        }
    }
}

impl Channel for HttpChannel {
    fn send(&mut self, message: &Value, cancel: &CancelToken) -> Result<(), McpError> {
        if self.shared.is_closed() {
            return Err(McpError::new(
                McpErrorKind::Cancelled,
                "the MCP server was stopped",
            ));
        }
        if is_request(message) {
            self.exchange = Some(self.post(message)?);
            return Ok(());
        }
        let receiver = self.post(message)?;
        loop {
            match self.next_event(&receiver, cancel)? {
                Event::Head { status, .. } if (200..300).contains(&status) => return Ok(()),
                Event::Head { status, .. } => return Err(self.status_error(status)),
                Event::Failed(error) => return Err(error),
                Event::Message(_) => {}
                Event::End => return Ok(()),
            }
        }
    }

    fn response(&mut self, id: i64, cancel: &CancelToken) -> Result<Value, McpError> {
        let receiver = self
            .exchange
            .take()
            .ok_or_else(|| McpError::protocol("no MCP request is waiting for a response"))?;
        let mut failed_status = None;
        loop {
            match self.next_event(&receiver, cancel)? {
                Event::Head { status, session } => {
                    if (200..300).contains(&status) {
                        if status == 202 {
                            return Err(McpError::protocol(
                                "the MCP server accepted a request without answering it",
                            ));
                        }
                        self.record_session(session)?;
                    } else if matches!(status, 401 | 403 | 404) {
                        return Err(self.status_error(status));
                    } else {
                        failed_status = Some(status);
                    }
                }
                Event::Message(message) => match route(&self.shared, message)? {
                    Routed::Response { id: got, outcome } if id_matches(&got, id) => {
                        return outcome;
                    }
                    Routed::Response { .. } | Routed::Handled => {}
                    Routed::Reply(reply) => self.reply(&reply),
                },
                Event::End => {
                    return Err(match failed_status {
                        Some(status) => self.status_error(status),
                        None => McpError::protocol(
                            "the MCP server ended its response without answering",
                        ),
                    });
                }
                Event::Failed(error) => return Err(error),
            }
        }
    }

    fn is_alive(&self) -> bool {
        self.shared.is_alive()
    }

    fn close(&mut self) {
        self.shared.close();
        self.exchange = None;
        let session = lock(&self.connection.session).take();
        let Some(session) = session else {
            return;
        };
        // Ending the session is a request too: offline or under an emergency
        // stop the session is simply abandoned.
        if self.egress.as_ref().is_some_and(|egress| egress().is_err()) {
            return;
        }
        let connection = self.connection.clone();
        let (sender, receiver) = sync_channel::<()>(1);
        let started = thread::Builder::new()
            .name("aidex-mcp-http-close".into())
            .spawn(move || {
                let mut request = connection
                    .agent
                    .delete(&connection.url)
                    .header("Mcp-Session-Id", session.as_str());
                for (name, value) in connection.headers.iter() {
                    request = request.header(name.as_str(), value.as_str());
                }
                if let Some(version) = lock(&connection.version).clone() {
                    request = request.header("MCP-Protocol-Version", version.as_str());
                }
                let _ = request.call();
                let _ = sender.send(());
            });
        if started.is_ok() {
            let _ = receiver.recv_timeout(Duration::from_secs(2));
        }
    }

    fn set_protocol_version(&mut self, version: &str) {
        *lock(&self.connection.version) = Some(version.to_owned());
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn run_post(connection: &Connection, shared: &Shared, body: &[u8], sender: &SyncSender<Event>) {
    let mut request = connection
        .agent
        .post(&connection.url)
        .header("Accept", "application/json, text/event-stream")
        .content_type("application/json");
    for (name, value) in connection.headers.iter() {
        request = request.header(name.as_str(), value.as_str());
    }
    if let Some(session) = lock(&connection.session).clone() {
        request = request.header("Mcp-Session-Id", session.as_str());
    }
    if let Some(version) = lock(&connection.version).clone() {
        request = request.header("MCP-Protocol-Version", version.as_str());
    }
    let mut response = match request.send(body) {
        Ok(response) => response,
        Err(error) => {
            let _ = sender.send(Event::Failed(map_ureq_error(error)));
            return;
        }
    };
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let session = header("mcp-session-id");
    let event_stream = header("content-type").is_some_and(|value| {
        value
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("text/event-stream")
    });
    if sender.send(Event::Head { status, session }).is_err() {
        return;
    }
    let deliver = |text: &str| -> bool {
        if text.trim().is_empty() {
            return true;
        }
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            // A malformed message ends the exchange.
            let _ = sender.send(Event::Failed(McpError::protocol(
                "the MCP server sent a malformed JSON message",
            )));
            return false;
        };
        let values = match value {
            Value::Array(items) => items,
            other => vec![other],
        };
        values.into_iter().all(|value| {
            sender
                .send(Event::Message(connection.scrubber.value(value)))
                .is_ok()
        })
    };
    let success = (200..300).contains(&status);
    let mut reader = BufReader::new(response.body_mut().as_reader());
    if success && event_stream {
        let outcome = read_events(&mut reader, MAX_MESSAGE, |data| deliver(&data));
        let _ = sender.send(match outcome {
            Ok(()) => Event::End,
            Err(error) => Event::Failed(body_error(&error)),
        });
        return;
    }
    let limit = if success {
        MAX_MESSAGE as u64
    } else {
        MAX_ERROR_BODY
    };
    let mut bytes = Vec::new();
    match reader.take(limit + 1).read_to_end(&mut bytes) {
        Ok(_) if success && bytes.len() as u64 > limit => {
            let _ = sender.send(Event::Failed(McpError::new(
                McpErrorKind::OutputLimit,
                "the MCP response exceeds 1 MiB",
            )));
            return;
        }
        Ok(_) => {
            let text = String::from_utf8_lossy(&bytes);
            if success || looks_like_jsonrpc(&text) {
                if !deliver(&text) {
                    return;
                }
            } else if !text.trim().is_empty() {
                shared.logs.push(
                    crate::LogStream::Client,
                    &format!(
                        "HTTP {status}: {}",
                        bounded_redacted(&connection.scrubber.text(&text), 300)
                    ),
                );
            }
        }
        Err(error) if success => {
            let _ = sender.send(Event::Failed(body_error(&error)));
            return;
        }
        Err(_) => {}
    }
    let _ = sender.send(Event::End);
}

fn looks_like_jsonrpc(text: &str) -> bool {
    serde_json::from_str::<Value>(text).is_ok_and(|value| value.get("jsonrpc").is_some())
}

fn body_error(error: &std::io::Error) -> McpError {
    match error.kind() {
        std::io::ErrorKind::InvalidData => {
            McpError::new(McpErrorKind::OutputLimit, error.to_string())
        }
        std::io::ErrorKind::TimedOut => {
            McpError::new(McpErrorKind::Timeout, "the MCP server stopped responding")
        }
        _ => McpError::new(
            McpErrorKind::Disconnected,
            "the connection was lost while reading the MCP response",
        ),
    }
}

fn map_ureq_error(error: ureq::Error) -> McpError {
    let unavailable = |message: &str| McpError::new(McpErrorKind::Unavailable, message);
    match error {
        ureq::Error::Timeout(_) => McpError::new(
            McpErrorKind::Timeout,
            "the MCP server did not respond in time",
        ),
        ureq::Error::HostNotFound => unavailable("the MCP server host was not found"),
        ureq::Error::ConnectionFailed => unavailable("the connection to the MCP server failed"),
        ureq::Error::Io(error) => match error.kind() {
            std::io::ErrorKind::TimedOut => McpError::new(
                McpErrorKind::Timeout,
                "the MCP server did not respond in time",
            ),
            std::io::ErrorKind::ConnectionRefused => {
                unavailable("the MCP server refused the connection")
            }
            kind => McpError::new(
                McpErrorKind::Unavailable,
                format!("connection error ({kind})"),
            ),
        },
        ureq::Error::Protocol(_) => McpError::protocol("the server sent a malformed HTTP response"),
        _ => unavailable("the request to the MCP server failed"),
    }
}
