//! State shared by a live connection, its reader threads, and the supervisor,
//! and the routing of everything a server sends that is not a response.

use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

use serde_json::Value;

use crate::{
    LogBuffer, LogStream, McpError,
    protocol::{Incoming, classify, reply_to_server_request},
    redact::bounded_redacted,
};

/// One connection's shared flags.
pub(crate) struct Shared {
    pub logs: Arc<LogBuffer>,
    /// A stop was requested; loops end and waits return `cancelled`.
    closed: AtomicBool,
    /// Why the connection ended on its own (process exit, fatal protocol error).
    ended: Mutex<Option<McpError>>,
    /// The server announced `notifications/tools/list_changed`.
    tools_changed: AtomicBool,
}

impl Shared {
    pub fn new(logs: Arc<LogBuffer>) -> Arc<Self> {
        Arc::new(Self {
            logs,
            closed: AtomicBool::new(false),
            ended: Mutex::new(None),
            tools_changed: AtomicBool::new(false),
        })
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Record why the connection ended (the first reason wins).
    pub fn end(&self, error: McpError) {
        let mut ended = self.ended.lock().unwrap_or_else(PoisonError::into_inner);
        if ended.is_none() {
            *ended = Some(error);
        }
    }

    pub fn ended(&self) -> Option<McpError> {
        self.ended
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn is_alive(&self) -> bool {
        !self.is_closed() && self.ended().is_none()
    }

    /// Take the "tools changed" flag.
    pub fn take_tools_changed(&self) -> bool {
        self.tools_changed.swap(false, Ordering::SeqCst)
    }
}

/// What to do with one inbound message.
pub(crate) enum Routed {
    /// A response to one of our requests.
    Response {
        id: Value,
        outcome: Result<Value, McpError>,
    },
    /// A reply the client must send back (the server asked something).
    Reply(Value),
    /// Fully handled (a notification).
    Handled,
}

/// Route one scrubbed inbound message.
pub(crate) fn route(shared: &Shared, frame: Value) -> Result<Routed, McpError> {
    match classify(frame)? {
        Incoming::Response { id, outcome } => Ok(Routed::Response { id, outcome }),
        Incoming::Request { id, method } => {
            if method != "ping" {
                shared.logs.push(
                    LogStream::Client,
                    &format!(
                        "declined the server's `{}` request (not supported by AiDEX)",
                        bounded_redacted(&method, 64)
                    ),
                );
            }
            Ok(Routed::Reply(reply_to_server_request(id, &method)))
        }
        Incoming::Notification { method, params } => {
            match method.as_str() {
                "notifications/tools/list_changed" => {
                    shared.tools_changed.store(true, Ordering::SeqCst);
                }
                "notifications/message" => {
                    let level = params
                        .get("level")
                        .and_then(Value::as_str)
                        .unwrap_or("info");
                    let data = match params.get("data") {
                        Some(Value::String(text)) => text.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    shared.logs.push(
                        LogStream::Server,
                        &format!("{}: {data}", bounded_redacted(level, 16)),
                    );
                }
                _ => {}
            }
            Ok(Routed::Handled)
        }
    }
}

/// Whether a response id is the numeric id we sent.
pub(crate) fn id_matches(id: &Value, expected: i64) -> bool {
    id.as_i64() == Some(expected)
}
