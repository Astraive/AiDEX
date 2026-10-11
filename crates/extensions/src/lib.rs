//! Portable extension metadata and permit-gated native MCP access.
mod connect;
mod error;
mod marketplace;
mod mcp;
mod model;
mod plugin_store;
mod portable;
mod schema;
mod service;
mod skill_store;
mod skills;
mod storage;
mod tools;
mod transport;
pub use error::{ExtensionError, Result};
pub use model::*;
pub use service::ExtensionService;
pub use transport::{
    ExtensionSecrets, McpConnection, McpConnector, NoSecrets, RealConnector, VaultSecrets,
};
