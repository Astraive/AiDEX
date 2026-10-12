//! Shared fixtures: the Python MCP server, the sandbox helper, static secrets, and
//! a loopback HTTP server.
#![allow(dead_code)]

pub mod http;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use aidex_mcp::{
    EndpointUrl, McpError, McpErrorKind, McpGate, McpSource, OpenGate, SandboxHost, SecretSource,
    ServerConfig, ServerSpec, TransportKind,
};
use aidex_sandbox::Sandbox;
use aidex_vault::{ConsumerScope, SecretName};
use zeroize::Zeroizing;

/// The interpreter for the fixture server: `AIDEX_TEST_PYTHON` (set by CI), or a
/// conventional location.
pub fn python() -> String {
    std::env::var("AIDEX_TEST_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "C:/Users/gskne/AppData/Local/Programs/Python/Python312/python.exe".into()
        } else {
            "/usr/bin/python3".into()
        }
    })
}

pub fn fixture_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mcp_server.py")
}

/// The sandbox helper built next to the test binaries (`cargo build --bins`).
pub fn sandbox() -> Sandbox {
    let mut helper = std::env::current_exe().unwrap();
    helper.pop();
    helper.pop();
    helper.push(format!(
        "aidex-sandbox-exec{}",
        std::env::consts::EXE_SUFFIX
    ));
    assert!(
        helper.is_file(),
        "build the sandbox helper first (cargo build --workspace --bins): {}",
        helper.display()
    );
    Sandbox::new(helper)
}

/// A stdio server running the fixture in `scenario`.
pub fn stdio_spec(id: &str, scenario: &str) -> ServerSpec {
    ServerSpec {
        id: id.into(),
        name: Some(format!("Fixture {id}")),
        transport: TransportKind::Stdio,
        command: Some(python()),
        args: vec![
            "-u".into(),
            fixture_script().to_string_lossy().into_owned(),
            scenario.into(),
        ],
        env_allowlist: Vec::new(),
        url: None,
        headers_from_secrets: Vec::new(),
        env_from_secrets: Vec::new(),
        enabled: true,
        workspaces: Vec::new(),
        auto_approve_tools: Vec::new(),
    }
}

pub fn stdio_config(id: &str, scenario: &str) -> ServerConfig {
    stdio_spec(id, scenario).into_config(McpSource::drive())
}

/// Secret values by name, available to every consumer listed with them.
#[derive(Default)]
pub struct StaticSecrets {
    values: BTreeMap<String, (String, String)>,
}

impl StaticSecrets {
    pub fn with(mut self, name: &str, consumer: &str, value: &str) -> Self {
        self.values
            .insert(name.into(), (consumer.into(), value.into()));
        self
    }
}

impl SecretSource for StaticSecrets {
    fn resolve(
        &self,
        consumer: &ConsumerScope,
        name: &SecretName,
    ) -> Result<Zeroizing<String>, McpError> {
        match self.values.get(name.as_str()) {
            Some((allowed, value)) if *allowed == consumer.to_string() => {
                Ok(Zeroizing::new(value.clone()))
            }
            Some(_) => Err(McpError::new(
                McpErrorKind::NeedsAuth,
                format!("secret `{name}` is not available to consumer {consumer}"),
            )),
            None => Err(McpError::new(
                McpErrorKind::NeedsAuth,
                format!("store secret `{name}` in the vault with consumer {consumer}"),
            )),
        }
    }
}

/// A gate that refuses every remote request, like the offline switch.
pub struct OfflineGate;

impl McpGate for OfflineGate {
    fn check_launch(&self, _config: &ServerConfig) -> Result<(), McpError> {
        Ok(())
    }

    fn check_egress(&self, _server_id: &str, _url: &EndpointUrl) -> Result<(), McpError> {
        Err(McpError::new(
            McpErrorKind::OfflineRefused,
            "AiDEX is offline",
        ))
    }
}

pub fn host(scratch: &Path, secrets: StaticSecrets) -> SandboxHost {
    SandboxHost::new(sandbox(), scratch, Arc::new(secrets), Arc::new(OpenGate))
}

/// A secret value generated per run, so no credential lives in a fixture.
pub fn sentinel() -> String {
    format!(
        "opaque-{}",
        aidex_core::EntityId::generate("secret").unwrap().as_str()
    )
}
