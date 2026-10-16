//! Blocking IPC client for the CLI and other local frontends.
//!
//! A background thread reads the connection and hands decoded messages to
//! the caller through a bounded channel, so the client never relies on socket
//! read timeouts. If the caller stops reading, TCP backpressure eventually
//! makes the daemon drop the connection as a slow consumer.

mod error;
mod subscription;

pub use error::ClientError;
pub use subscription::{Notification, Subscription};

use crate::{
    endpoint::{self, Endpoint},
    errors::{ErrorCode, RpcError},
    framing::{Frame, LineReader},
    handshake::HandshakeResult,
    router::ClientInfo,
    rpc::{
        CLOSING_NOTIFICATION, CONTRACT_VERSION, HANDSHAKE_METHOD, Id, MAX_MESSAGE_BYTES, Message,
        UNSUBSCRIBED_NOTIFICATION,
    },
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::{
    collections::{HashSet, VecDeque},
    fmt, io,
    io::Write,
    net::{Shutdown, SocketAddr, TcpStream},
    path::Path,
    sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel},
    thread,
    time::{Duration, Instant},
};

/// Default time a `call` waits for its response.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// Time `update.check`, `update.apply`, and `update.rollback` are given. They re-verify, stage,
/// and hash a whole bundle and run its self-tests (up to a minute per command), which can take
/// minutes on a USB drive; the daemon keeps working when a caller stops waiting, so a shorter
/// limit only reports a failure for an update that still completes. Frontends send these on a
/// connection of their own, so other calls do not queue behind them.
pub const UPDATE_CALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const INCOMING_QUEUE: usize = 256;
const MAX_BUFFERED_NOTIFICATIONS: usize = 4096;

enum Incoming {
    Message(Message),
    Invalid(String),
    TooLarge,
    Closed,
}

/// One authenticated connection to `aidexd`. Calls are sequential.
pub struct Client {
    writer: TcpStream,
    incoming: Receiver<Incoming>,
    next_id: i64,
    buffered: VecDeque<Notification>,
    active: HashSet<String>,
    closing: Option<RpcError>,
    closed: bool,
    server: Option<HandshakeResult>,
    call_timeout: Duration,
}

impl Client {
    /// Read the drive's endpoint, connect, and handshake as `client_name`.
    pub fn connect(drive_root: &Path, client_name: &str) -> Result<Self, ClientError> {
        Self::connect_with(
            drive_root,
            ClientInfo::new(client_name, env!("CARGO_PKG_VERSION")),
        )
    }

    pub fn connect_with(drive_root: &Path, client: ClientInfo) -> Result<Self, ClientError> {
        client.validate().map_err(ClientError::Protocol)?;
        let summary = endpoint::summarize(drive_root)
            .map_err(ClientError::from_endpoint)?
            .ok_or_else(|| {
                ClientError::from_endpoint(endpoint::EndpointError::Missing(endpoint::state_path(
                    drive_root,
                )))
            })?;
        if summary.version != CONTRACT_VERSION {
            return Err(ClientError::from_endpoint(
                endpoint::EndpointError::Incompatible {
                    found: summary.version,
                },
            ));
        }
        let port = summary.port.filter(|port| *port != 0).ok_or_else(|| {
            ClientError::Protocol("daemon endpoint port is missing or invalid".into())
        })?;
        // Removable media carries stale DPAPI blobs between Windows machines.
        // Probe loopback before unlocking: an absent peer is a stopped daemon,
        // not a rejection of the current user's credentials.
        drop(Self::open_stream(port)?);
        let endpoint = endpoint::read(drive_root).map_err(ClientError::from_endpoint)?;
        if endpoint.port != port || Some(endpoint.pid) != summary.pid {
            return Err(ClientError::Unavailable(
                "daemon endpoint changed while connecting; retry".into(),
            ));
        }
        // Open the authenticated connection after DPAPI finishes, so a slow
        // Windows profile does not consume the server's handshake deadline.
        Self::connect_endpoint(&endpoint, client)
    }

    /// Connect to an already-decoded endpoint (in-process daemons and tests).
    pub fn connect_endpoint(endpoint: &Endpoint, client: ClientInfo) -> Result<Self, ClientError> {
        client.validate().map_err(ClientError::Protocol)?;
        Self::connect_stream(endpoint, client, Self::open_stream(endpoint.port)?)
    }
    fn open_stream(port: u16) -> Result<TcpStream, ClientError> {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(|error| {
            ClientError::Unavailable(format!(
                "daemon endpoint {address} is not reachable ({error}); the daemon may have exited"
            ))
        })
    }
    fn connect_stream(
        endpoint: &Endpoint,
        client: ClientInfo,
        stream: TcpStream,
    ) -> Result<Self, ClientError> {
        stream.set_nodelay(true).map_err(ClientError::Io)?;
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .map_err(ClientError::Io)?;
        let reader = stream.try_clone().map_err(ClientError::Io)?;
        let (sender, incoming) = sync_channel(INCOMING_QUEUE);
        thread::Builder::new()
            .name("aidexd-client-read".into())
            .spawn(move || read_incoming(reader, &sender))
            .map_err(ClientError::Io)?;
        let mut connection = Self {
            writer: stream,
            incoming,
            next_id: 1,
            buffered: VecDeque::new(),
            active: HashSet::new(),
            closing: None,
            closed: false,
            server: None,
            call_timeout: DEFAULT_CALL_TIMEOUT,
        };
        let params = json!({
            "contract_version": CONTRACT_VERSION,
            "token": endpoint.token.expose(),
            "client": client,
        });
        let result = connection
            .call_with_timeout(HANDSHAKE_METHOD, params, HANDSHAKE_TIMEOUT)
            .map_err(|error| match error {
                ClientError::Io(error) if error.kind() != io::ErrorKind::TimedOut => {
                    ClientError::Unavailable(format!(
                        "daemon closed the connection during the handshake ({error})"
                    ))
                }
                other => other,
            })?;
        let result: HandshakeResult = serde_json::from_value(result)
            .map_err(|error| ClientError::Protocol(format!("invalid handshake result: {error}")))?;
        if result.contract_version != CONTRACT_VERSION {
            return Err(ClientError::Incompatible {
                message: format!(
                    "daemon negotiated IPC contract version {}, expected {CONTRACT_VERSION}",
                    result.contract_version
                ),
                supported: vec![result.contract_version],
            });
        }
        connection.server = Some(result);
        Ok(connection)
    }

    /// Handshake result: negotiated version, daemon identity, connection id.
    pub fn server(&self) -> Option<&HandshakeResult> {
        self.server.as_ref()
    }

    pub fn set_call_timeout(&mut self, timeout: Duration) {
        self.call_timeout = timeout;
    }

    /// Call a method and wait for its result. Notifications that arrive
    /// meanwhile are buffered for their subscriptions.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        self.call_with_timeout(method, params, self.call_timeout)
    }

    /// Call and decode the result into `T`.
    pub fn call_as<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, ClientError> {
        let value = self.call(method, params)?;
        serde_json::from_value(value).map_err(|error| {
            ClientError::Protocol(format!("unexpected `{method}` result: {error}"))
        })
    }

    pub fn call_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&Message::request(id, method, params))?;
        let deadline = Instant::now() + timeout;
        loop {
            let Some(message) = self.recv(deadline)? else {
                return Err(ClientError::Io(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("no response to `{method}` within {timeout:?}"),
                )));
            };
            match message {
                Message::Response {
                    id: Id::Number(answered),
                    outcome,
                } if answered == id => return outcome.map_err(ClientError::from_rpc),
                Message::Response {
                    id: Id::Null,
                    outcome: Err(error),
                } => return Err(ClientError::from_rpc(error)),
                // A late answer to an earlier timed-out call.
                Message::Response { .. } => {}
                Message::Notification { method, params } => {
                    self.accept_notification(method, params)?;
                }
                Message::Request { .. } => return Err(self.violation("daemon sent a request")),
            }
        }
    }

    /// Send a notification (no response).
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), ClientError> {
        self.send(&Message::notification(method, params))
    }

    /// Call a stream method and return its subscription.
    pub fn subscribe(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Subscription<'_>, ClientError> {
        let mut result = self.call(method, params)?;
        let id = result
            .get("subscription")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                ClientError::Protocol(format!("`{method}` did not return a subscription id"))
            })?;
        let initial = result
            .as_object_mut()
            .and_then(|object| object.remove("data"))
            .unwrap_or(Value::Null);
        self.active.insert(id.clone());
        Ok(Subscription::new(self, id, initial))
    }

    fn send(&mut self, message: &Message) -> Result<(), ClientError> {
        if self.closed {
            return Err(self.closed_error());
        }
        let line = message.encode_line();
        if line.len() - 1 > MAX_MESSAGE_BYTES {
            return Err(ClientError::Protocol(format!(
                "request exceeds the {MAX_MESSAGE_BYTES}-byte message limit"
            )));
        }
        match self.writer.write_all(line.as_bytes()) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.closed = true;
                // Pick up a closing notice the daemon sent before it hung up.
                while let Ok(item) = self.incoming.try_recv() {
                    if let Incoming::Message(Message::Notification { method, params }) = item
                        && method == CLOSING_NOTIFICATION
                    {
                        self.record_closing(params.unwrap_or(Value::Null));
                    }
                }
                let disconnected = matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::NotConnected
                );
                if disconnected || self.closing.is_some() {
                    Err(self.closed_error())
                } else {
                    Err(ClientError::Io(error))
                }
            }
        }
    }

    /// `Ok(None)` when the deadline passes without a message.
    fn recv(&mut self, deadline: Instant) -> Result<Option<Message>, ClientError> {
        if self.closed {
            return Err(self.closed_error());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.incoming.recv_timeout(remaining) {
            Ok(Incoming::Message(message)) => Ok(Some(message)),
            Ok(Incoming::Invalid(reason)) => {
                Err(self.violation(&format!("daemon sent an invalid message: {reason}")))
            }
            Ok(Incoming::TooLarge) => Err(self.violation("daemon sent an oversized message")),
            Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                Err(self.closed_error())
            }
            Err(RecvTimeoutError::Timeout) => Ok(None),
        }
    }

    fn violation(&mut self, message: &str) -> ClientError {
        self.closed = true;
        let _ = self.writer.shutdown(Shutdown::Both);
        ClientError::Protocol(message.to_owned())
    }

    fn closed_error(&self) -> ClientError {
        match &self.closing {
            Some(reason) if reason.kind() == Some(ErrorCode::ShuttingDown) => {
                ClientError::Unavailable(format!(
                    "daemon closed the connection: {}",
                    reason.message
                ))
            }
            Some(reason) => ClientError::from_rpc(reason.clone()),
            None => ClientError::Unavailable("daemon closed the connection".into()),
        }
    }

    fn accept_notification(
        &mut self,
        method: String,
        params: Option<Value>,
    ) -> Result<(), ClientError> {
        let params = params.unwrap_or(Value::Null);
        if method == CLOSING_NOTIFICATION {
            self.record_closing(params);
            return Ok(());
        }
        let Some(subscription) = params
            .get("subscription")
            .and_then(Value::as_str)
            .filter(|id| self.active.contains(*id))
            .map(str::to_owned)
        else {
            // Not for a live subscription of this client (e.g. one just released).
            return Ok(());
        };
        let params = if method == UNSUBSCRIBED_NOTIFICATION {
            params
        } else {
            match params {
                Value::Object(mut object) => object.remove("data").unwrap_or(Value::Null),
                _ => Value::Null,
            }
        };
        if self.buffered.len() >= MAX_BUFFERED_NOTIFICATIONS {
            return Err(self.violation("too many unread notifications buffered"));
        }
        self.buffered.push_back(Notification {
            method,
            subscription: Some(subscription),
            params,
        });
        Ok(())
    }

    fn record_closing(&mut self, params: Value) {
        let code = params
            .get("code")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok())
            .unwrap_or(ErrorCode::ShuttingDown.code());
        let message = params
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("daemon closed the connection")
            .to_owned();
        self.closing = Some(RpcError {
            code,
            message,
            data: None,
        });
    }

    fn take_buffered(&mut self, subscription: &str) -> Option<Notification> {
        let position = self
            .buffered
            .iter()
            .position(|item| item.subscription.as_deref() == Some(subscription))?;
        self.buffered.remove(position)
    }

    fn release(&mut self, subscription: &str) {
        self.active.remove(subscription);
        self.buffered
            .retain(|item| item.subscription.as_deref() != Some(subscription));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("server", &self.server)
            .field("closed", &self.closed)
            .finish_non_exhaustive()
    }
}

fn read_incoming(stream: TcpStream, sender: &SyncSender<Incoming>) {
    let mut reader = LineReader::new(stream, MAX_MESSAGE_BYTES);
    loop {
        let item = match reader.read_frame() {
            Ok(Frame::Line(line)) => match Message::parse(&line) {
                Ok(message) => Incoming::Message(message),
                Err(malformed) => Incoming::Invalid(malformed.error.message),
            },
            Ok(Frame::TooLarge) => Incoming::TooLarge,
            Ok(Frame::Eof) | Err(_) => Incoming::Closed,
        };
        let last = !matches!(item, Incoming::Message(_));
        if sender.send(item).is_err() || last {
            return;
        }
    }
}
