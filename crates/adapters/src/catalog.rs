use aidex_native_sessions::{Mode, PermissionMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Driver {
    Codex,
    ClaudeCode,
    Cursor,
    Grok,
    Opencode,
    Antigravity,
    Pi,
    Muse,
    Acp,
    Omp,
}
impl Driver {
    pub const ALL: [Self; 10] = [
        Self::Codex,
        Self::ClaudeCode,
        Self::Cursor,
        Self::Grok,
        Self::Opencode,
        Self::Antigravity,
        Self::Pi,
        Self::Muse,
        Self::Acp,
        Self::Omp,
    ];
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(Default)]
pub enum RuntimeAuth {
    #[default]
    None,
    Vault {
        secret_name: String,
        env_name: String,
    },
    CodexOauth {
        provider_id: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub id: String,
    pub driver: Driver,
    pub executable: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub auth: RuntimeAuth,
}
fn enabled() -> bool {
    true
}
#[derive(Clone, Debug, Serialize)]
pub struct DriverInfo {
    pub driver: Driver,
    pub name: String,
    pub protocol: String,
    pub command: Vec<String>,
    pub package: Option<String>,
    pub modes: Vec<Mode>,
    pub permission_modes: Vec<PermissionMode>,
    pub limitations: Vec<String>,
    pub auth_methods: Vec<String>,
    pub supports_mcp_bridge: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct RuntimeHealth {
    pub id: String,
    pub installed: bool,
    pub protocol_ready: bool,
    pub authenticated: bool,
    pub status: String,
    pub limitations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeModel {
    pub id: String,
    pub name: String,
}
impl Driver {
    pub(crate) fn info(self) -> DriverInfo {
        let (name, protocol, args, package) = match self {
            Self::Codex => (
                "Codex",
                "codex_app_server",
                vec!["app-server"],
                Some("@openai/codex"),
            ),
            Self::ClaudeCode => (
                "Claude Code",
                "claude_stream_json",
                vec![
                    "--print",
                    "--verbose",
                    "--output-format",
                    "stream-json",
                    "--include-partial-messages",
                ],
                Some("@anthropic-ai/claude-code"),
            ),
            Self::Cursor => (
                "Cursor",
                "cursor_stream_json",
                vec!["--print", "--output-format", "stream-json", "--force"],
                None,
            ),
            Self::Grok => (
                "Grok",
                "acp",
                vec!["agent", "stdio"],
                Some("@xai-official/grok"),
            ),
            Self::Opencode => ("OpenCode", "acp", vec!["acp"], Some("opencode-ai")),
            Self::Antigravity => ("Antigravity", "acp", vec![], None),
            Self::Pi => (
                "Pi",
                "pi_rpc",
                vec!["--mode", "rpc"],
                Some("@earendil-works/pi-coding-agent"),
            ),
            Self::Muse => (
                "Muse",
                "msp",
                vec!["serve", "--trust-workspace", "--disable-sandbox"],
                None,
            ),
            Self::Acp => ("ACP local command", "acp", vec![], None),
            Self::Omp => (
                "Oh My Pi",
                "omp_rpc",
                vec!["--mode", "rpc"],
                Some("@oh-my-pi/pi-coding-agent"),
            ),
        };
        let mut limitations=vec!["External runtime owns its tools; AiDEX does not intercept every tool. Full access runs as the current user with a terminable process tree, without filesystem or network confinement.".into(),"CLI auth readiness requires a supported account/status response; binary presence alone is not authentication.".into(),"Only agent/full_access is currently selectable; vendor-specific confined modes require a verified enforcement probe.".into()];
        if self == Self::Antigravity {
            limitations.push("Configure the official ACP executable and literal launcher argv for this platform. No invented Windows launcher or host-keychain portability is claimed.".into());
        }
        if matches!(self, Self::Pi | Self::Omp) {
            limitations.push("RPC tool execution has no universal approval callback; supervised modes are unavailable.".into());
        }
        if matches!(self, Self::ClaudeCode | Self::Cursor) {
            limitations.push("Print protocol does not expose AiDEX approval mediation in this adapter; only explicit full access is supported.".into());
        }
        if !crate::bridge::supported(self) {
            limitations.push("AiDEX's scoped MCP tool bridge is unsupported by this adapter; vendor tools remain vendor owned.".into());
        }
        DriverInfo {
            driver: self,
            name: name.into(),
            protocol: protocol.into(),
            command: args.into_iter().map(str::to_owned).collect(),
            package: package.map(str::to_owned),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::FullAccess],
            limitations,
            supports_mcp_bridge: crate::bridge::supported(self),
            auth_methods: if self == Self::Codex {
                vec![
                    "native_cli".into(),
                    "profile_import".into(),
                    "codex_oauth".into(),
                    "vault".into(),
                ]
            } else {
                vec!["vault".into()]
            },
        }
    }
}
pub fn catalog() -> Vec<DriverInfo> {
    Driver::ALL.into_iter().map(Driver::info).collect()
}
