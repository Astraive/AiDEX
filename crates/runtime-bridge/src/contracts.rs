use aidex_core::CancelToken;
use serde::Serialize;
use serde_json::Value;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeScope {
    pub session_id: String,
    pub run_id: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct BridgeTool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    pub annotations: Value,
}
#[derive(Clone, Debug)]
pub struct BridgeError {
    pub code: &'static str,
    pub message: String,
}
impl BridgeError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for BridgeError {}
/// The transport supplies this scope from its hashed lease registry, never from
/// model input. Implementations recheck active run identity before every effect.
pub trait BridgeDispatcher: Send + Sync {
    fn active(&self, scope: &BridgeScope) -> bool;
    fn tools(&self, scope: &BridgeScope) -> Result<Vec<BridgeTool>, BridgeError>;
    fn call(
        &self,
        scope: &BridgeScope,
        request_id: &str,
        name: &str,
        arguments: Value,
        cancel: &CancelToken,
    ) -> Result<Value, BridgeError>;
}
