//! Per-server MCP configuration: `DrivePaths::mcp()/<id>.toml`, closed schema v1.
//!
//! A configuration holds literal launch parameters and vault secret *names*.
//! Secret values never appear here: `env_from_secrets` and
//! `headers_from_secrets` name vault secrets (bound to consumer `mcp:<id>`) that
//! are resolved only when the server starts.

mod import;
mod url;

pub use import::{ImportedServer, MAX_MCP_JSON_BYTES, import_mcp_json, slug_id};
pub use url::EndpointUrl;

use aidex_vault::{ConsumerScope, SecretName};
use serde::{Deserialize, Serialize};

use crate::{McpError, redact::bounded_redacted};

/// Schema version of a server file.
pub const CONFIG_VERSION: u32 = 1;
/// Longest server id (also the file stem).
pub const MAX_ID_LEN: usize = 64;
const MAX_NAME_CHARS: usize = 128;
const MAX_COMMAND_BYTES: usize = 4096;
const MAX_ARGS: usize = 256;
const MAX_ARG_BYTES: usize = 8192;
const MAX_ENV: usize = 64;
const MAX_SECRET_MAPPINGS: usize = 32;
const MAX_WORKSPACES: usize = 64;
const MAX_AUTO_APPROVE: usize = 256;
const MAX_TOOL_NAME_CHARS: usize = 128;
const MAX_HEADER_NAME: usize = 128;

/// Headers the client sets itself; a secret mapping may not replace them.
const RESERVED_HEADERS: &[&str] = &[
    "accept",
    "connection",
    "content-length",
    "content-type",
    "host",
    "mcp-protocol-version",
    "mcp-session-id",
    "transfer-encoding",
    "upgrade",
    "user-agent",
];

/// How the client talks to the server.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// A local process speaking newline-delimited JSON-RPC on stdin/stdout.
    Stdio,
    /// A Streamable HTTP endpoint (POST JSON, JSON or SSE responses).
    Http,
}

impl TransportKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdio => "stdio",
            Self::Http => "http",
        }
    }
}

/// An HTTP header whose value is the named vault secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderSecret {
    pub header: String,
    pub secret_name: String,
}

/// An environment variable whose value is the named vault secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvSecret {
    pub env: String,
    pub secret_name: String,
}

/// Where a server configuration came from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpSourceKind {
    /// Created by the user in AiDEX.
    #[default]
    Drive,
    /// Imported from a workspace `.mcp.json`.
    Workspace,
    /// Contributed by an installed plugin's `.mcp.json`.
    Plugin,
}

impl McpSourceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Drive => "drive",
            Self::Workspace => "workspace",
            Self::Plugin => "plugin",
        }
    }
}

/// Provenance of a server configuration: the kind plus the owning workspace id
/// or plugin name.
#[derive(Clone, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpSource {
    pub kind: McpSourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
}

impl McpSource {
    pub fn drive() -> Self {
        Self::default()
    }

    pub fn workspace(workspace_id: impl Into<String>) -> Self {
        Self {
            kind: McpSourceKind::Workspace,
            owner: Some(workspace_id.into()),
        }
    }

    pub fn plugin(name: impl Into<String>) -> Self {
        Self {
            kind: McpSourceKind::Plugin,
            owner: Some(name.into()),
        }
    }

    pub fn is_drive(&self) -> bool {
        self.kind == McpSourceKind::Drive
    }
}

/// One server file. Field order keeps TOML tables last.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub transport: TransportKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    /// Parent environment variables copied into the server (on top of the
    /// sandbox's PATH/SystemRoot basics). Credential-like names are refused.
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Disabled until the user enables it.
    #[serde(default)]
    pub enabled: bool,
    /// Workspace ids the server is offered in; empty means every workspace.
    #[serde(default)]
    pub workspaces: Vec<String>,
    /// Tools the harness may call without approval in Supervised mode.
    #[serde(default)]
    pub auto_approve_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers_from_secrets: Vec<HeaderSecret>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_from_secrets: Vec<EnvSecret>,
    #[serde(default, skip_serializing_if = "McpSource::is_drive")]
    pub source: McpSource,
}

/// A server as a client submits it (`mcp.add`/`mcp.update`): no version, no
/// provenance, and no field that could carry a secret value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpec {
    pub id: String,
    /// Display name; defaults to the id.
    #[serde(default)]
    pub name: Option<String>,
    pub transport: TransportKind,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env_allowlist: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub headers_from_secrets: Vec<HeaderSecret>,
    #[serde(default)]
    pub env_from_secrets: Vec<EnvSecret>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub auto_approve_tools: Vec<String>,
}

impl ServerSpec {
    /// The configuration this spec describes, with `source` provenance. Not yet
    /// validated.
    pub fn into_config(self, source: McpSource) -> ServerConfig {
        ServerConfig {
            version: CONFIG_VERSION,
            name: self.name.unwrap_or_else(|| self.id.clone()),
            id: self.id,
            transport: self.transport,
            command: self.command,
            args: self.args,
            env_allowlist: self.env_allowlist,
            url: self.url,
            enabled: self.enabled,
            workspaces: self.workspaces,
            auto_approve_tools: self.auto_approve_tools,
            headers_from_secrets: self.headers_from_secrets,
            env_from_secrets: self.env_from_secrets,
            source,
        }
    }
}

impl ServerConfig {
    /// Check every field; the first problem is reported.
    pub fn validate(&self) -> Result<(), McpError> {
        let bad = |message: String| Err(McpError::invalid_config(message));
        if self.version != CONFIG_VERSION {
            return bad(format!(
                "unsupported MCP server file version {} (expected {CONFIG_VERSION})",
                self.version
            ));
        }
        validate_id(&self.id)?;
        let name = self.name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return bad(format!(
                "name must be 1-{MAX_NAME_CHARS} characters without control characters"
            ));
        }
        match self.transport {
            TransportKind::Stdio => self.validate_stdio()?,
            TransportKind::Http => self.validate_http()?,
        }
        if self.workspaces.len() > MAX_WORKSPACES {
            return bad(format!("at most {MAX_WORKSPACES} workspaces"));
        }
        for workspace in &self.workspaces {
            if aidex_core::EntityId::new(workspace.as_str()).is_err() {
                return bad(format!("workspace id `{workspace}` is invalid"));
            }
        }
        if self.auto_approve_tools.len() > MAX_AUTO_APPROVE {
            return bad(format!("at most {MAX_AUTO_APPROVE} auto-approved tools"));
        }
        for tool in &self.auto_approve_tools {
            if !valid_tool_name(tool) {
                return bad(format!(
                    "auto-approved tool `{}` must be 1-{MAX_TOOL_NAME_CHARS} characters without control characters",
                    bounded_redacted(tool, 64)
                ));
            }
        }
        if let Some(owner) = &self.source.owner
            && (owner.is_empty() || owner.len() > 128 || owner.chars().any(char::is_control))
        {
            return bad("source owner must be 1-128 characters".into());
        }
        if self.source.kind != McpSourceKind::Drive && self.source.owner.is_none() {
            return bad("an imported server must name its workspace or plugin".into());
        }
        Ok(())
    }

    fn validate_stdio(&self) -> Result<(), McpError> {
        let bad = |message: &str| Err(McpError::invalid_config(message.to_owned()));
        let Some(command) = self.command.as_deref() else {
            return bad("a stdio server needs a command");
        };
        if command.trim().is_empty()
            || command.len() > MAX_COMMAND_BYTES
            || command.chars().any(char::is_control)
        {
            return bad("command must be 1-4096 bytes without control characters");
        }
        if self.url.is_some() || !self.headers_from_secrets.is_empty() {
            return bad("a stdio server takes no url or headers");
        }
        if self.args.len() > MAX_ARGS {
            return bad("at most 256 arguments");
        }
        if self
            .args
            .iter()
            .any(|arg| arg.len() > MAX_ARG_BYTES || arg.contains('\0'))
        {
            return bad("each argument must be at most 8192 bytes without NUL");
        }
        if looks_like_credential(command) || self.args.iter().any(|arg| looks_like_credential(arg))
        {
            return bad(
                "the command line looks like it contains a credential; store it in the vault and map it with env_from_secrets",
            );
        }
        if self.env_allowlist.len() > MAX_ENV {
            return bad("at most 64 allowlisted environment variables");
        }
        for name in &self.env_allowlist {
            validate_env_name(name)?;
            if aidex_sandbox::is_secret_like(name) {
                return Err(McpError::invalid_config(format!(
                    "`{name}` looks like a credential and cannot be inherited; store it in the vault and map it with env_from_secrets"
                )));
            }
        }
        if self.env_from_secrets.len() > MAX_SECRET_MAPPINGS {
            return bad("at most 32 secret environment variables");
        }
        let mut seen: Vec<String> = Vec::new();
        for mapping in &self.env_from_secrets {
            validate_env_name(&mapping.env)?;
            validate_secret_name(&mapping.secret_name)?;
            let key = mapping.env.to_ascii_uppercase();
            if seen.contains(&key) {
                return Err(McpError::invalid_config(format!(
                    "environment variable `{}` is mapped twice",
                    mapping.env
                )));
            }
            seen.push(key);
        }
        Ok(())
    }

    fn validate_http(&self) -> Result<(), McpError> {
        let bad = |message: &str| Err(McpError::invalid_config(message.to_owned()));
        let Some(url) = self.url.as_deref() else {
            return bad("an http server needs a url");
        };
        EndpointUrl::parse(url).map_err(McpError::invalid_config)?;
        if looks_like_credential(url) {
            return bad(
                "the URL looks like it contains a credential; map it to a header from the vault",
            );
        }
        if self.command.is_some()
            || !self.args.is_empty()
            || !self.env_allowlist.is_empty()
            || !self.env_from_secrets.is_empty()
        {
            return bad("an http server takes no command, args, or environment");
        }
        if self.headers_from_secrets.len() > MAX_SECRET_MAPPINGS {
            return bad("at most 32 secret headers");
        }
        let mut seen: Vec<String> = Vec::new();
        for mapping in &self.headers_from_secrets {
            let header = mapping.header.as_str();
            if header.is_empty()
                || header.len() > MAX_HEADER_NAME
                || !header.bytes().all(is_token_byte)
            {
                return Err(McpError::invalid_config(format!(
                    "`{}` is not a valid HTTP header name",
                    bounded_redacted(header, 64)
                )));
            }
            let lower = header.to_ascii_lowercase();
            if RESERVED_HEADERS.contains(&lower.as_str()) {
                return Err(McpError::invalid_config(format!(
                    "header `{header}` is set by AiDEX and cannot come from a secret"
                )));
            }
            if seen.contains(&lower) {
                return Err(McpError::invalid_config(format!(
                    "header `{header}` is mapped twice"
                )));
            }
            seen.push(lower);
            validate_secret_name(&mapping.secret_name)?;
        }
        Ok(())
    }

    /// The parsed endpoint of an http server.
    pub fn endpoint_url(&self) -> Option<EndpointUrl> {
        self.url
            .as_deref()
            .and_then(|url| EndpointUrl::parse(url).ok())
    }

    /// A display summary: the command line (redacted, bounded) or the URL origin.
    pub fn endpoint_summary(&self) -> String {
        match self.transport {
            TransportKind::Stdio => {
                let mut line = self.command.clone().unwrap_or_default();
                for arg in &self.args {
                    line.push(' ');
                    line.push_str(arg);
                }
                // Arguments may hold control characters (a plugin's are
                // imported as written); the summary is shown on terminals.
                bounded_redacted(&line, 512)
                    .chars()
                    .map(|ch| if ch.is_control() { '\u{fffd}' } else { ch })
                    .collect()
            }
            TransportKind::Http => self
                .endpoint_url()
                .map(|url| url.origin())
                .unwrap_or_default(),
        }
    }

    /// Whether the server is offered in `workspace` (`None`: no workspace filter).
    pub fn applies_to(&self, workspace: Option<&str>) -> bool {
        match workspace {
            None => true,
            Some(id) => self.workspaces.is_empty() || self.workspaces.iter().any(|w| w == id),
        }
    }

    /// The vault secret names this server needs.
    pub fn secret_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .env_from_secrets
            .iter()
            .map(|mapping| mapping.secret_name.clone())
            .chain(
                self.headers_from_secrets
                    .iter()
                    .map(|mapping| mapping.secret_name.clone()),
            )
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

/// A server id: `[a-z0-9][a-z0-9_-]{0,63}`, usable as a file stem and as the
/// vault consumer `mcp:<id>`.
pub fn validate_id(id: &str) -> Result<(), McpError> {
    let valid = !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
        && ConsumerScope::mcp(id).is_ok();
    if valid {
        Ok(())
    } else {
        Err(McpError::invalid_config(format!(
            "server id `{}` must be 1-{MAX_ID_LEN} characters of a-z, 0-9, `-`, `_`, starting with a letter or digit",
            bounded_redacted(id, 80)
        )))
    }
}

fn validate_env_name(name: &str) -> Result<(), McpError> {
    let valid = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !valid {
        return Err(McpError::invalid_config(format!(
            "`{}` is not a valid environment variable name",
            bounded_redacted(name, 64)
        )));
    }
    if aidex_sandbox::MANAGED_ENV_VARS
        .iter()
        .any(|managed| managed.eq_ignore_ascii_case(name))
    {
        return Err(McpError::invalid_config(format!(
            "`{name}` is managed by the sandbox"
        )));
    }
    Ok(())
}

fn validate_secret_name(name: &str) -> Result<(), McpError> {
    SecretName::new(name).map(drop).map_err(|_| {
        McpError::invalid_config(format!(
            "`{}` is not a valid vault secret name",
            bounded_redacted(name, 64)
        ))
    })
}

/// Whether `text` holds something the credential redactor would hide.
fn looks_like_credential(text: &str) -> bool {
    aidex_events::redact_text(text).as_ref() != text
}

fn valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_TOOL_NAME_CHARS
        && !name.chars().any(char::is_control)
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stdio() -> ServerConfig {
        ServerSpec {
            id: "github".into(),
            name: Some("GitHub".into()),
            transport: TransportKind::Stdio,
            command: Some("npx".into()),
            args: vec!["-y".into(), "@modelcontextprotocol/server-github".into()],
            env_allowlist: vec!["NODE_OPTIONS".into()],
            url: None,
            headers_from_secrets: vec![],
            env_from_secrets: vec![EnvSecret {
                env: "GITHUB_PERSONAL_ACCESS_TOKEN".into(),
                secret_name: "github-token".into(),
            }],
            enabled: false,
            workspaces: vec![],
            auto_approve_tools: vec!["search_repositories".into()],
        }
        .into_config(McpSource::drive())
    }

    #[test]
    fn a_complete_stdio_config_validates_and_round_trips_through_toml() {
        let config = stdio();
        config.validate().unwrap();
        let text = toml::to_string(&config).unwrap();
        assert!(!text.contains("source"));
        let back: ServerConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, config);
        assert_eq!(
            config.endpoint_summary(),
            "npx -y @modelcontextprotocol/server-github"
        );
        assert_eq!(config.secret_names(), ["github-token"]);
        assert!(config.applies_to(Some("ws_1")));
    }

    #[test]
    fn the_endpoint_summary_hides_control_characters_in_arguments() {
        let mut config = stdio();
        config.args = vec!["--name".into(), "a\rb\u{1b}[2J".into()];
        config.validate().unwrap();
        assert_eq!(
            config.endpoint_summary(),
            "npx --name a\u{fffd}b\u{fffd}[2J"
        );
        assert_eq!(config.args[1], "a\rb\u{1b}[2J");
    }

    #[test]
    fn http_configs_keep_tables_last_and_validate_headers() {
        let mut config = ServerSpec {
            id: "remote".into(),
            name: None,
            transport: TransportKind::Http,
            command: None,
            args: vec![],
            env_allowlist: vec![],
            url: Some("https://mcp.example.com/mcp".into()),
            headers_from_secrets: vec![HeaderSecret {
                header: "Authorization".into(),
                secret_name: "remote-token".into(),
            }],
            env_from_secrets: vec![],
            enabled: true,
            workspaces: vec!["ws_1".into()],
            auto_approve_tools: vec![],
        }
        .into_config(McpSource::workspace("ws_1"));
        config.validate().unwrap();
        assert_eq!(config.name, "remote");
        let text = toml::to_string(&config).unwrap();
        let back: ServerConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, config);
        assert_eq!(config.endpoint_summary(), "https://mcp.example.com");
        assert!(!config.applies_to(Some("ws_2")));
        config.headers_from_secrets[0].header = "Host".into();
        assert!(config.validate().is_err());
        config.headers_from_secrets[0].header = "Bad Header".into();
        assert!(config.validate().is_err());
    }

    type Mutation = Box<dyn Fn(&mut ServerConfig)>;

    #[test]
    fn unsafe_or_inconsistent_configs_are_refused() {
        let cases: Vec<(&str, Mutation)> = vec![
            ("version", Box::new(|c| c.version = 2)),
            ("id upper", Box::new(|c| c.id = "GitHub".into())),
            ("id dot", Box::new(|c| c.id = "a.b".into())),
            ("id long", Box::new(|c| c.id = "a".repeat(65))),
            ("name", Box::new(|c| c.name = " ".into())),
            ("no command", Box::new(|c| c.command = None)),
            (
                "url on stdio",
                Box::new(|c| c.url = Some("https://x.test".into())),
            ),
            (
                "secret-like allowlist",
                Box::new(|c| c.env_allowlist = vec!["GITHUB_TOKEN".into()]),
            ),
            (
                "managed env",
                Box::new(|c| c.env_allowlist = vec!["TEMP".into()]),
            ),
            (
                "bad env",
                Box::new(|c| c.env_from_secrets[0].env = "1BAD".into()),
            ),
            (
                "bad secret",
                Box::new(|c| c.env_from_secrets[0].secret_name = "has space".into()),
            ),
            (
                "duplicate env",
                Box::new(|c| {
                    let first = c.env_from_secrets[0].clone();
                    c.env_from_secrets.push(first);
                }),
            ),
            (
                "workspace",
                Box::new(|c| c.workspaces = vec!["bad id".into()]),
            ),
            (
                "unowned import",
                Box::new(|c| {
                    c.source = McpSource {
                        kind: McpSourceKind::Plugin,
                        owner: None,
                    }
                }),
            ),
            ("nul arg", Box::new(|c| c.args.push("a\0b".into()))),
            (
                "credential arg",
                Box::new(|c| {
                    c.args
                        .push("--api-key=sk-proj-abcdefghijklmnopqrstuvwxyz0123".into())
                }),
            ),
        ];
        for (label, mutate) in cases {
            let mut config = stdio();
            mutate(&mut config);
            assert_eq!(
                config.validate().unwrap_err().kind,
                crate::McpErrorKind::InvalidConfig,
                "{label}"
            );
        }
    }

    #[test]
    fn unknown_fields_and_secret_values_are_rejected_by_the_schema() {
        let text = "version = 1\nid = \"x\"\nname = \"x\"\ntransport = \"stdio\"\ncommand = \"x\"\n[env]\nTOKEN = \"value\"\n";
        assert!(toml::from_str::<ServerConfig>(text).is_err());
        let spec =
            serde_json::json!({"id":"x","transport":"stdio","command":"x","env":{"TOKEN":"v"}});
        assert!(serde_json::from_value::<ServerSpec>(spec).is_err());
    }
}
