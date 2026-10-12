//! MCP server management for AiDEX (issue #24).
//!
//! - [`McpConfigStore`]: one closed-schema TOML file per server under
//!   `DrivePaths::mcp()`, plus a read-only `.mcp.json` importer that produces
//!   DISABLED entries without any secret value.
//! - [`McpClient`]: an MCP JSON-RPC 2.0 client over a sandboxed process's stdio
//!   (newline-delimited, 1 MiB frames) or Streamable HTTP (POST JSON, JSON or SSE
//!   responses), with bounded timeouts and a stderr ring buffer.
//! - [`McpSupervisor`]: starts servers on demand or explicitly, tracks their
//!   health, lists and calls their tools, and stops them (one, or all on shutdown,
//!   vault lock, or emergency stop). Launching and egress go through an
//!   [`McpHost`]; [`SandboxHost`] runs stdio servers through `aidex-sandbox` with a
//!   cleared environment and vault-resolved secrets (consumer `mcp:<id>`).
//!
//! Credentials never appear in configuration, logs, errors, or `Debug` output:
//! configuration names vault secrets, and everything a server sends back is
//! scrubbed of the values resolved for it.

mod client;
mod config;
mod error;
mod logs;
mod protocol;
mod redact;
mod store;
mod supervisor;

pub use client::{
    EgressCheck, HttpEndpoint, MAX_FRAME, McpClient, ServerIdentity, Timeouts, ToolListing,
};
pub use config::{
    CONFIG_VERSION, EndpointUrl, EnvSecret, HeaderSecret, ImportedServer, MAX_ID_LEN,
    MAX_MCP_JSON_BYTES, McpSource, McpSourceKind, ServerConfig, ServerSpec, TransportKind,
    import_mcp_json, slug_id, validate_id,
};
pub use error::{McpError, McpErrorKind};
pub use logs::{LogBuffer, LogLine, LogStream, LogTail, MAX_LOG_LINES};
pub use protocol::{
    CLIENT_NAME, LATEST_PROTOCOL_VERSION, MAX_RESULT_TEXT, MAX_TOOLS, McpToolResult, McpToolSpec,
    SUPPORTED_PROTOCOL_VERSIONS,
};
pub use redact::Scrubber;
pub use store::{InvalidServer, MAX_SERVERS, McpConfigStore, ServerListing};
pub use supervisor::{
    LaunchedServer, McpGate, McpHost, McpStatus, McpSupervisor, McpToolset, OpenGate, SandboxHost,
    SecretSource, ServerState, StatusError, VaultSecrets,
};
