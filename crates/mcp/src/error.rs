//! Stable MCP error kinds. Messages never contain credential values: secrets are
//! scrubbed from server output before it can reach an error, and configuration
//! carries secret names only.

use std::fmt;

use serde::Serialize;

/// Why an MCP operation failed. `as_str` is the stable `data.reason` on the wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpErrorKind {
    /// A configuration or `.mcp.json` entry is malformed or unsafe.
    InvalidConfig,
    /// Invalid input to a client call (unknown tool name shape, non-object arguments).
    InvalidRequest,
    /// No server with that id.
    NotFound,
    /// A server with that id already exists, or the operation conflicts with state.
    Conflict,
    /// The server is disabled; enable it before starting it or calling its tools.
    Disabled,
    /// Reading or writing the configuration directory failed.
    Io,
    /// The vault is locked (or missing) and the server needs a secret from it.
    VaultLocked,
    /// A required secret is missing, not bound to `mcp:<id>`, or the server
    /// answered HTTP 401/403.
    NeedsAuth,
    /// The offline switch refuses remote MCP servers.
    OfflineRefused,
    /// The network policy, an emergency stop, or the process registry refused it.
    PolicyDenied,
    /// The server process could not be launched under the sandbox.
    LaunchFailed,
    /// The server violated the protocol (malformed frame, wrong version, ...).
    Protocol,
    /// A frame or response exceeded its size limit.
    OutputLimit,
    /// The server answered with a JSON-RPC error.
    ServerError,
    /// The server process exited or the connection closed.
    Disconnected,
    /// An HTTP server forgot the session (HTTP 404); restart the server.
    SessionExpired,
    /// The request did not complete in time.
    Timeout,
    /// The caller cancelled, or the server was stopped during the request.
    Cancelled,
    /// Anything else that makes the server unusable right now.
    Unavailable,
}

impl McpErrorKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidConfig => "invalid_config",
            Self::InvalidRequest => "invalid_request",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Disabled => "disabled",
            Self::Io => "io",
            Self::VaultLocked => "vault_locked",
            Self::NeedsAuth => "needs_auth",
            Self::OfflineRefused => "offline_refused",
            Self::PolicyDenied => "policy_denied",
            Self::LaunchFailed => "launch_failed",
            Self::Protocol => "protocol",
            Self::OutputLimit => "output_limit",
            Self::ServerError => "server_error",
            Self::Disconnected => "disconnected",
            Self::SessionExpired => "session_expired",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unavailable => "unavailable",
        }
    }
}

impl fmt::Display for McpErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An MCP failure: a stable kind plus a human message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpError {
    pub kind: McpErrorKind,
    pub message: String,
}

impl McpError {
    pub fn new(kind: McpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::InvalidConfig, message)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::Protocol, message)
    }

    pub fn not_found(id: &str) -> Self {
        Self::new(
            McpErrorKind::NotFound,
            format!("no MCP server `{id}` is configured"),
        )
    }

    pub fn io(context: &str, error: &std::io::Error) -> Self {
        Self::new(McpErrorKind::Io, format!("{context}: {}", error.kind()))
    }

    pub fn kind(&self) -> McpErrorKind {
        self.kind
    }

    /// Whether the server's status becomes `needs_auth` after this failure.
    pub fn needs_auth(&self) -> bool {
        matches!(
            self.kind,
            McpErrorKind::NeedsAuth | McpErrorKind::VaultLocked
        )
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for McpError {}
