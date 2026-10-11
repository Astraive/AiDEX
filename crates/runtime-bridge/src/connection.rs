//! Backend-only ephemeral credential; neither serializable nor printable.
use std::{fmt, sync::Arc};
use zeroize::Zeroizing;
#[derive(Clone)]
pub struct BridgeConnection {
    pub(crate) endpoint: String,
    pub(crate) token: Arc<Zeroizing<String>>,
}
impl BridgeConnection {
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    /// Only pass this to the provider's native MCP client. Never persist it in
    /// snapshots, command diagnostics, frontend DTOs or opaque resume references.
    pub fn authorization_header(&self) -> String {
        format!("Bearer {}", self.token.as_str())
    }
}
impl fmt::Debug for BridgeConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeConnection")
            .field("endpoint", &self.endpoint)
            .field("authorization", &"[REDACTED]")
            .finish()
    }
}
