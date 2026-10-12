//! The MCP client: one live connection to one server over a [`Channel`]
//! (stdio or Streamable HTTP). Requests are sequential (`&mut self`); every
//! request has a bounded timeout, and a timed-out or cancelled request is
//! followed by a best-effort `notifications/cancelled`.

mod http;
mod shared;
mod sse;
mod stdio;

pub use http::{EgressCheck, HttpEndpoint};
pub use stdio::MAX_FRAME;

use std::{collections::HashSet, fmt, sync::Arc, time::Duration};

use aidex_core::{CancelReason, CancelToken, EntityId};
use aidex_sandbox::ManagedProcess;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    LogBuffer, LogStream, McpError, McpErrorKind,
    protocol::{
        MAX_RESULT_TEXT, MAX_TOOL_PAGES, MAX_TOOLS, McpToolResult, McpToolSpec, initialize_params,
        negotiated_version, notification, parse_tools_page, request, tool_result,
    },
    redact::{Scrubber, bounded_redacted},
};

use self::{http::HttpChannel, shared::Shared, stdio::StdioChannel};

/// One transport carrying JSON-RPC messages to and from a server.
pub(crate) trait Channel: Send {
    /// Send one message. A request's answer is then read with [`Channel::response`].
    fn send(&mut self, message: &Value, cancel: &CancelToken) -> Result<(), McpError>;
    /// Wait for the response to request `id`, handling everything else the server
    /// sends meanwhile.
    fn response(&mut self, id: i64, cancel: &CancelToken) -> Result<Value, McpError>;
    fn is_alive(&self) -> bool;
    /// End the connection (gracefully where the transport allows).
    fn close(&mut self);
    /// Remember the negotiated protocol revision (HTTP sends it as a header).
    fn set_protocol_version(&mut self, _version: &str) {}
}

/// Why a wait must end now: the connection was stopped, or `cancel` fired.
pub(crate) fn cancelled_error(cancel: &CancelToken, shared: &Shared) -> Option<McpError> {
    if shared.is_closed() {
        return Some(McpError::new(
            McpErrorKind::Cancelled,
            "the MCP server was stopped",
        ));
    }
    match cancel.state() {
        Some(CancelReason::Cancelled) => Some(McpError::new(
            McpErrorKind::Cancelled,
            "the MCP request was cancelled",
        )),
        Some(CancelReason::DeadlineExceeded) => Some(McpError::new(
            McpErrorKind::Timeout,
            "the MCP server did not answer in time",
        )),
        None => None,
    }
}

/// Per-request time limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timeouts {
    /// `initialize` (includes the server's own start-up work).
    pub initialize: Duration,
    /// `tools/list`, `ping`, and notifications.
    pub request: Duration,
    /// One `tools/call`.
    pub call: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            initialize: Duration::from_secs(60),
            request: Duration::from_secs(30),
            call: Duration::from_secs(300),
        }
    }
}

/// What the server reported in `initialize` (bounded, redacted display text).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServerIdentity {
    pub name: String,
    pub version: String,
    pub protocol_version: String,
    /// The server declared the `tools` capability.
    pub tools: bool,
}

/// Every tool of one listing plus notes about skipped or reduced entries.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ToolListing {
    pub tools: Vec<McpToolSpec>,
    pub notes: Vec<String>,
}

/// Stops a connection from another thread: in-flight waits end with `cancelled`.
#[derive(Clone)]
pub(crate) struct Interrupter {
    shared: Arc<Shared>,
    process: Option<Arc<ManagedProcess>>,
}

impl Interrupter {
    pub fn interrupt(&self) {
        self.shared.close();
    }

    pub fn is_alive(&self) -> bool {
        self.shared.is_alive() && self.process.as_ref().is_none_or(|p| p.is_running())
    }

    /// Why the connection ended on its own, if it did.
    pub fn failure(&self) -> Option<McpError> {
        if let Some(error) = self.shared.ended() {
            return Some(error);
        }
        if self.process.as_ref().is_some_and(|p| !p.is_running()) {
            return Some(McpError::new(
                McpErrorKind::Disconnected,
                "the MCP server process exited",
            ));
        }
        None
    }

    pub fn process(&self) -> Option<&Arc<ManagedProcess>> {
        self.process.as_ref()
    }
}

/// A live MCP connection.
pub struct McpClient {
    channel: Box<dyn Channel>,
    shared: Arc<Shared>,
    process: Option<Arc<ManagedProcess>>,
    next_id: i64,
    identity: Option<ServerIdentity>,
    timeouts: Timeouts,
}

impl fmt::Debug for McpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpClient")
            .field("identity", &self.identity)
            .field("process", &self.process.as_ref().map(|p| p.id().clone()))
            .field("alive", &self.is_alive())
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// A connection over a running interactive process. Every protocol frame and
    /// stderr line is scrubbed of `scrubber`'s values before it is used or logged.
    pub fn stdio(
        process: ManagedProcess,
        logs: Arc<LogBuffer>,
        scrubber: Scrubber,
        timeouts: Timeouts,
    ) -> Result<Self, McpError> {
        logs.set_scrubber(scrubber.clone());
        let shared = Shared::new(logs);
        let channel = StdioChannel::start(process, Arc::clone(&shared), scrubber)?;
        let process = Some(channel.process());
        Ok(Self {
            channel: Box::new(channel),
            shared,
            process,
            next_id: 1,
            identity: None,
            timeouts,
        })
    }

    /// A connection to a Streamable HTTP endpoint. Nothing is sent until
    /// [`McpClient::initialize`].
    pub fn http(endpoint: HttpEndpoint, logs: Arc<LogBuffer>, timeouts: Timeouts) -> Self {
        logs.set_scrubber(Scrubber::new(
            endpoint
                .headers
                .iter()
                .map(|(_, value)| value.clone())
                .collect(),
        ));
        let shared = Shared::new(logs);
        let channel = HttpChannel::new(endpoint, Arc::clone(&shared));
        Self {
            channel: Box::new(channel),
            shared,
            process: None,
            next_id: 1,
            identity: None,
            timeouts,
        }
    }

    pub fn identity(&self) -> Option<&ServerIdentity> {
        self.identity.as_ref()
    }

    pub fn is_alive(&self) -> bool {
        self.channel.is_alive()
    }

    /// The registry id of the server process (stdio only).
    pub fn process_id(&self) -> Option<EntityId> {
        self.process.as_ref().map(|process| process.id().clone())
    }

    /// Whether the server announced `notifications/tools/list_changed` since the
    /// last call (the flag is cleared).
    pub fn take_tools_changed(&self) -> bool {
        self.shared.take_tools_changed()
    }

    pub(crate) fn interrupter(&self) -> Interrupter {
        Interrupter {
            shared: Arc::clone(&self.shared),
            process: self.process.clone(),
        }
    }

    /// `initialize` (version negotiation, `clientInfo` AiDEX) followed by
    /// `notifications/initialized`.
    pub fn initialize(&mut self, cancel: &CancelToken) -> Result<ServerIdentity, McpError> {
        let result = self.request(
            "initialize",
            Some(initialize_params()),
            self.timeouts.initialize,
            cancel,
        )?;
        let protocol_version = negotiated_version(&result)?;
        self.channel.set_protocol_version(&protocol_version);
        let info = result.get("serverInfo");
        let text = |field: &str, max: usize| {
            info.and_then(|info| info.get(field))
                .and_then(Value::as_str)
                .map(|text| bounded_redacted(text, max))
                .unwrap_or_default()
        };
        let identity = ServerIdentity {
            name: text("name", 128),
            version: text("version", 64),
            protocol_version,
            tools: result
                .get("capabilities")
                .and_then(|capabilities| capabilities.get("tools"))
                .is_some(),
        };
        let initialized = notification("notifications/initialized", None);
        self.channel
            .send(&initialized, &cancel.with_timeout(self.timeouts.request))?;
        self.shared.logs.push(
            LogStream::Client,
            &format!(
                "connected to {} {} (MCP {})",
                or_unnamed(&identity.name),
                identity.version,
                identity.protocol_version
            ),
        );
        self.identity = Some(identity.clone());
        Ok(identity)
    }

    /// Every tool, following `nextCursor` (at most 64 pages and 512 tools;
    /// duplicate names keep the first).
    pub fn list_tools(&mut self, cancel: &CancelToken) -> Result<ToolListing, McpError> {
        let mut listing = ToolListing::default();
        if self
            .identity
            .as_ref()
            .is_some_and(|identity| !identity.tools)
        {
            listing
                .notes
                .push("the server does not offer tools".to_owned());
            return Ok(listing);
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut cursor: Option<String> = None;
        'pages: for page_number in 1..=MAX_TOOL_PAGES {
            let params = cursor.as_ref().map(|cursor| json!({"cursor": cursor}));
            let result = self.request("tools/list", params, self.timeouts.request, cancel)?;
            let page = parse_tools_page(&result)?;
            listing.notes.extend(page.notes);
            for tool in page.tools {
                if !seen.insert(tool.name.clone()) {
                    listing.notes.push(format!(
                        "skipped a second tool named `{}`",
                        bounded_redacted(&tool.name, 64)
                    ));
                    continue;
                }
                if listing.tools.len() >= MAX_TOOLS {
                    listing
                        .notes
                        .push(format!("kept the first {MAX_TOOLS} tools"));
                    break 'pages;
                }
                listing.tools.push(tool);
            }
            match page.next_cursor {
                None => break,
                Some(next) if cursor.as_deref() == Some(next.as_str()) => {
                    listing
                        .notes
                        .push("the server repeated a tools/list cursor".to_owned());
                    break;
                }
                Some(next) => cursor = Some(next),
            }
            if page_number == MAX_TOOL_PAGES {
                listing
                    .notes
                    .push(format!("stopped after {MAX_TOOL_PAGES} tools/list pages"));
            }
        }
        Ok(listing)
    }

    /// Call one tool. `arguments` must be a JSON object (`null` means none).
    /// The result is reduced to bounded, redacted text.
    pub fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
        cancel: &CancelToken,
    ) -> Result<McpToolResult, McpError> {
        if name.is_empty() || name.chars().count() > 128 || name.chars().any(char::is_control) {
            return Err(McpError::new(
                McpErrorKind::InvalidRequest,
                "a tool name must be 1-128 characters without control characters",
            ));
        }
        let arguments = match arguments {
            Value::Null => json!({}),
            object @ Value::Object(_) => object,
            _ => {
                return Err(McpError::new(
                    McpErrorKind::InvalidRequest,
                    "tool arguments must be a JSON object",
                ));
            }
        };
        let result = self.request(
            "tools/call",
            Some(json!({"name": name, "arguments": arguments})),
            self.timeouts.call,
            cancel,
        )?;
        tool_result(&result, MAX_RESULT_TEXT)
    }

    /// Liveness check.
    pub fn ping(&mut self, cancel: &CancelToken) -> Result<(), McpError> {
        self.request("ping", None, self.timeouts.request, cancel)
            .map(drop)
    }

    /// End the connection: stdio servers get end-of-input and up to two seconds
    /// to exit before the tree is killed; HTTP sessions are ended with DELETE.
    pub fn close(&mut self) {
        self.channel.close();
    }

    fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let bounded = cancel.with_timeout(timeout);
        let message = request(id, method, params);
        let outcome = self
            .channel
            .send(&message, &bounded)
            .and_then(|()| self.channel.response(id, &bounded));
        outcome.map_err(|error| {
            if matches!(error.kind, McpErrorKind::Timeout | McpErrorKind::Cancelled)
                && !self.shared.is_closed()
                && self.channel.is_alive()
            {
                let notice = notification(
                    "notifications/cancelled",
                    Some(json!({"requestId": id, "reason": error.message})),
                );
                let _ = self.channel.send(
                    &notice,
                    &CancelToken::new().with_timeout(Duration::from_secs(2)),
                );
            }
            if error.kind != McpErrorKind::Timeout {
                return error;
            }
            // Name this request's own limit only when it, not the caller's
            // earlier deadline, ran out.
            let message = if cancel.is_stopped() {
                format!("the MCP server did not answer `{method}` before the caller's time limit")
            } else {
                format!(
                    "the MCP server did not answer `{method}` within {} s",
                    timeout.as_secs().max(1)
                )
            };
            McpError::new(McpErrorKind::Timeout, message)
        })
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if !self.shared.is_closed() {
            self.channel.close();
        }
    }
}

fn or_unnamed(name: &str) -> &str {
    if name.is_empty() {
        "an unnamed server"
    } else {
        name
    }
}
