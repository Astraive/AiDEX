mod http;
mod protocol;
mod stdio;
use crate::{CallScope, ExtensionError, McpServerConfig, McpTool, McpTransport, Result};
use aidex_tools::CancelToken;
pub(crate) use http::endpoint;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicUsize},
};

pub trait ExtensionSecrets: Send + Sync {
    fn available(&self) -> bool {
        true
    }
    fn version(&self, server_id: &str, name: &str) -> Result<aidex_vault::SecretVersion>;
    fn resolve_versioned(
        &self,
        server_id: &str,
        name: &str,
        expected: &aidex_vault::SecretVersion,
    ) -> Result<aidex_vault::SecretBytes>;
    fn resolve(&self, server_id: &str, name: &str) -> Result<aidex_vault::SecretBytes> {
        self.resolve_versioned(server_id, name, &self.version(server_id, name)?)
    }
}
pub struct NoSecrets;
impl ExtensionSecrets for NoSecrets {
    fn version(&self, _: &str, _: &str) -> Result<aidex_vault::SecretVersion> {
        Err(ExtensionError::Unavailable(
            "vault secret versions are unavailable".into(),
        ))
    }
    fn resolve_versioned(
        &self,
        _: &str,
        _: &str,
        _: &aidex_vault::SecretVersion,
    ) -> Result<aidex_vault::SecretBytes> {
        Err(ExtensionError::Unavailable(
            "vault secret resolution is unavailable".into(),
        ))
    }
}
pub struct VaultSecrets(pub Arc<aidex_vault::VaultService>);
impl ExtensionSecrets for VaultSecrets {
    fn available(&self) -> bool {
        self.0.status().state == aidex_vault::VaultState::Unlocked
    }
    fn version(&self, _: &str, name: &str) -> Result<aidex_vault::SecretVersion> {
        let name = aidex_vault::SecretName::new(name)
            .map_err(|_| ExtensionError::Invalid("invalid secret reference".into()))?;
        self.0
            .secret_version(&name)
            .map_err(|_| ExtensionError::Forbidden("vault credential is locked or missing".into()))
    }
    fn resolve_versioned(
        &self,
        server_id: &str,
        name: &str,
        expected: &aidex_vault::SecretVersion,
    ) -> Result<aidex_vault::SecretBytes> {
        let scope = aidex_vault::ConsumerScope::mcp(server_id)
            .map_err(|_| ExtensionError::Invalid("invalid MCP consumer".into()))?;
        let name = aidex_vault::SecretName::new(name)
            .map_err(|_| ExtensionError::Invalid("invalid secret reference".into()))?;
        self.0
            .resolve_versioned(&name, &scope, expected)
            .map_err(|e| {
                if matches!(
                    e,
                    aidex_vault::VaultError::Conflict
                        | aidex_vault::VaultError::HandleDenied(
                            aidex_vault::HandleDenial::RevisionChanged
                        )
                ) {
                    ExtensionError::Conflict(
                        "vault credential changed; reconnect explicitly".into(),
                    )
                } else {
                    ExtensionError::Forbidden(
                        "vault credential is locked, missing or not granted".into(),
                    )
                }
            })
    }
}
pub(crate) struct BoundSecrets {
    pub source: Arc<dyn ExtensionSecrets>,
    pub server_id: String,
    pub versions: std::collections::BTreeMap<String, aidex_vault::SecretVersion>,
}
impl ExtensionSecrets for BoundSecrets {
    fn available(&self) -> bool {
        self.source.available()
    }
    fn version(&self, server_id: &str, name: &str) -> Result<aidex_vault::SecretVersion> {
        let expected = self
            .versions
            .get(name)
            .filter(|_| server_id == self.server_id)
            .ok_or_else(|| {
                ExtensionError::Forbidden("credential was not declared by this server".into())
            })?;
        if self.source.version(server_id, name)? != *expected {
            return Err(ExtensionError::Conflict(
                "vault credential changed; reconnect explicitly".into(),
            ));
        }
        Ok(expected.clone())
    }
    fn resolve_versioned(
        &self,
        server_id: &str,
        name: &str,
        expected: &aidex_vault::SecretVersion,
    ) -> Result<aidex_vault::SecretBytes> {
        if server_id != self.server_id || self.versions.get(name) != Some(expected) {
            return Err(ExtensionError::Conflict(
                "credential binding changed".into(),
            ));
        }
        self.source.resolve_versioned(server_id, name, expected)
    }
}
pub trait McpConnection: Send {
    fn initialize(&mut self, cancel: &CancelToken) -> Result<()>;
    fn list_tools(&mut self, cancel: &CancelToken) -> Result<Vec<McpTool>>;
    fn call_tool(&mut self, name: &str, args: Value, cancel: &CancelToken) -> Result<Value>;
    fn disconnect(&mut self);
}
pub trait McpConnector: Send + Sync {
    fn connect(
        &self,
        config: &McpServerConfig,
        scope: &CallScope,
        secrets: Arc<dyn ExtensionSecrets>,
        cancel: &CancelToken,
    ) -> Result<Box<dyn McpConnection>>;
}
pub struct RealConnector {
    sandbox: aidex_sandbox::Sandbox,
    scratch: PathBuf,
    drive_root: Option<PathBuf>,
    http_budget: Arc<AtomicUsize>,
    network_gate: Arc<dyn Fn() -> bool + Send + Sync>,
}
impl RealConnector {
    pub fn new(sandbox: aidex_sandbox::Sandbox, scratch: PathBuf) -> Self {
        Self {
            sandbox,
            scratch,
            drive_root: None,
            http_budget: Arc::new(AtomicUsize::new(0)),
            network_gate: Arc::new(|| true),
        }
    }
    pub fn for_drive(sandbox: aidex_sandbox::Sandbox, paths: &aidex_drive::DrivePaths) -> Self {
        Self {
            sandbox,
            scratch: paths.tmp(),
            drive_root: Some(paths.root().to_path_buf()),
            http_budget: Arc::new(AtomicUsize::new(0)),
            network_gate: Arc::new(|| true),
        }
    }
    pub fn with_network_gate(mut self, gate: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.network_gate = gate;
        self
    }
}
impl McpConnector for RealConnector {
    fn connect(
        &self,
        config: &McpServerConfig,
        scope: &CallScope,
        secrets: Arc<dyn ExtensionSecrets>,
        cancel: &CancelToken,
    ) -> Result<Box<dyn McpConnection>> {
        if cancel.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        if !scope.network_allowed || !(self.network_gate)() {
            return Err(ExtensionError::Forbidden("network access is denied".into()));
        }
        let wire: Box<dyn protocol::Wire> = match &config.transport {
            McpTransport::Https { url } => Box::new(http::HttpWire::new(
                url,
                config.clone(),
                secrets,
                self.http_budget.clone(),
                self.network_gate.clone(),
            )?),
            McpTransport::Stdio { .. } => Box::new(stdio::StdioWire::start(
                &self.sandbox,
                stdio::LaunchPaths {
                    scratch_root: &self.scratch,
                    drive_root: self.drive_root.as_deref(),
                },
                config,
                scope,
                secrets,
                cancel,
                self.network_gate.clone(),
            )?),
        };
        Ok(Box::new(protocol::Connection::new(wire)))
    }
}
pub(crate) fn reflected(value: &Value, secrets: &[aidex_vault::SecretBytes]) -> bool {
    match value {
        Value::String(s) => secrets.iter().any(|secret| {
            std::str::from_utf8(secret.expose()).is_ok_and(|v| !v.is_empty() && s.contains(v))
        }),
        Value::Array(values) => values.iter().any(|v| reflected(v, secrets)),
        Value::Object(values) => values
            .iter()
            .any(|(k, v)| reflected(&Value::String(k.clone()), secrets) || reflected(v, secrets)),
        Value::Number(_) | Value::Bool(_) | Value::Null => {
            reflected(&Value::String(value.to_string()), secrets)
        }
    }
}
pub(crate) fn protect_frame(
    value: &mut Value,
    secrets: &[aidex_vault::SecretBytes],
    method: Option<&str>,
) -> Result<()> {
    if method != Some("tools/call") && reflected(value, secrets) {
        return Err(ExtensionError::Protocol(
            "server reflected a credential in metadata".into(),
        ));
    }
    redact(value, secrets);
    Ok(())
}
pub(crate) fn redact(value: &mut Value, secrets: &[aidex_vault::SecretBytes]) {
    match value {
        Value::String(text) => {
            for secret in secrets {
                if let Ok(s) = std::str::from_utf8(secret.expose())
                    && !s.is_empty()
                {
                    *text = text.replace(s, "[REDACTED]")
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                redact(value, secrets)
            }
        }
        Value::Object(object) => {
            for (key, mut value) in std::mem::take(object) {
                let mut protected_key = Value::String(key);
                redact(&mut protected_key, secrets);
                redact(&mut value, secrets);
                object.insert(
                    protected_key.as_str().unwrap_or("[REDACTED]").to_owned(),
                    value,
                );
            }
        }
        Value::Number(_) | Value::Bool(_) | Value::Null => {
            if reflected(value, secrets) {
                *value = Value::String("[REDACTED]".into());
            }
        }
    }
}
