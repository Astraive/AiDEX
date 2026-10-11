//! Run-scoped, authenticated loopback MCP transport for provider SDKs.
mod connection;
mod contracts;
mod http;
mod protocol;
mod server;
pub use connection::BridgeConnection;
pub use contracts::{BridgeDispatcher, BridgeError, BridgeScope, BridgeTool};
pub use server::{BridgeLease, BridgeServer};
