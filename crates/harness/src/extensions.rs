//! Skills, web search and fetch, and MCP servers that the embedder (the daemon)
//! lends the native harness. Everything here is optional: [`NoExtensions`]
//! offers none of them, and the harness then behaves as before.
//!
//! Text crossing this boundary (skill bodies, search results, pages, MCP
//! output) is untrusted; the harness wraps it before it reaches a model.
use aidex_core::CancelToken;
use aidex_policy::{EvaluationContext, HostTrust, NetworkMode, PrivacyMode, Profile};
use serde::Serialize;
use serde_json::Value;
use std::fmt;

/// How the text of a skill is trusted in a prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillTrust {
    /// Written by the user (drive skills).
    User,
    /// Guidance from a workspace, another agent's skill folder, or a plugin.
    Project,
}

impl SkillTrust {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// An enabled skill's instructions (credential-redacted, still untrusted).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillText {
    /// `<scope>:<name>`.
    pub id: String,
    pub name: String,
    pub description: String,
    /// `project`, `repo`, `drive`, or `plugin`.
    pub scope: String,
    pub body: String,
    pub trust: SkillTrust,
}

/// A skill the model may load with `skill_load`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
}

/// Whether web tools can be offered, from the drive's web settings.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebStatus {
    /// The user's master switch (`web.toml` `enabled`).
    pub enabled: bool,
    /// Whether `web_fetch` is offered alongside search.
    pub fetch_enabled: bool,
    /// The configured provider id (`brave`, `searxng`, ...).
    pub provider: String,
    /// Host and port searches go to, for the policy check.
    pub search_target: Option<(String, u16)>,
    /// Why searches cannot run yet (missing key or endpoint), if anything.
    pub problem: Option<String>,
}

impl WebStatus {
    /// Search is offered: switched on, configured, and with a known host.
    pub fn search_ready(&self) -> bool {
        self.enabled && self.problem.is_none() && self.search_target.is_some()
    }
}

/// One search result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WebHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

/// A fetched page reduced to readable text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebPage {
    pub url: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub title: Option<String>,
    pub text: String,
    /// The body or the text reached the client's size limit.
    pub truncated: bool,
}

/// A tool of an MCP server.
#[derive(Clone, Debug, PartialEq)]
pub struct McpTool {
    pub name: String,
    /// The server's description (untrusted text).
    pub description: String,
    /// JSON schema of the arguments object.
    pub input_schema: Value,
}

/// The tools of one enabled, running MCP server.
#[derive(Clone, Debug, PartialEq)]
pub struct McpToolset {
    pub server_id: String,
    pub server_name: String,
    /// Tools the user lets run without approval in Supervised mode.
    pub auto_approve_tools: Vec<String>,
    pub tools: Vec<McpTool>,
}

/// An MCP tool result reduced to bounded text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct McpOutput {
    pub text: String,
    /// The server reported a tool-level failure.
    pub is_error: bool,
    pub truncated: bool,
}

/// What the network gate needs: a refusal reason (offline switch, emergency
/// stop) and the drive's policy settings.
#[derive(Clone, Debug)]
pub struct NetworkState {
    pub blocked: Option<String>,
    pub context: EvaluationContext,
}

impl Default for NetworkState {
    /// The drive defaults: balanced profile, hybrid privacy, known host,
    /// normal network.
    fn default() -> Self {
        Self {
            blocked: None,
            context: EvaluationContext::new(
                Profile::Balanced,
                PrivacyMode::Hybrid,
                HostTrust::Known,
                NetworkMode::Normal,
            ),
        }
    }
}

/// Why an extension call failed. Messages must never carry credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtensionError {
    /// An invoked or loaded skill is unknown, disabled, or invalid.
    SkillUnavailable(String),
    /// The service is not configured or not running.
    Unavailable(String),
    /// The call ran and failed.
    Failed(String),
}

impl fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SkillUnavailable(message) => write!(f, "skill_unavailable: {message}"),
            Self::Unavailable(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ExtensionError {}

/// Services the daemon lends the harness. Every method has a "nothing
/// available" default.
pub trait Extensions: Send + Sync {
    /// The bodies of the skills a user invoked, in order. Unknown, disabled,
    /// or not user-invocable ids are [`ExtensionError::SkillUnavailable`].
    fn invoked_skills(
        &self,
        _workspace: Option<&str>,
        ids: &[String],
    ) -> Result<Vec<SkillText>, ExtensionError> {
        match ids.first() {
            None => Ok(Vec::new()),
            Some(id) => Err(ExtensionError::SkillUnavailable(format!(
                "skills are unavailable here ({id})"
            ))),
        }
    }

    /// Enabled skills the model may load, for the system prompt catalog.
    fn model_skills(&self, _workspace: Option<&str>) -> Vec<SkillEntry> {
        Vec::new()
    }

    /// An enabled, model-invocable skill by name.
    fn load_skill(
        &self,
        _workspace: Option<&str>,
        name: &str,
    ) -> Result<SkillText, ExtensionError> {
        Err(ExtensionError::SkillUnavailable(format!(
            "no skill named `{name}` can be loaded"
        )))
    }

    /// The web settings as far as tool offering is concerned.
    fn web(&self) -> WebStatus {
        WebStatus::default()
    }

    /// Search with the configured provider; returns the provider id and hits.
    fn web_search(
        &self,
        _query: &str,
        _max_results: Option<usize>,
        _cancel: &CancelToken,
    ) -> Result<(String, Vec<WebHit>), ExtensionError> {
        Err(ExtensionError::Unavailable(
            "web search is not configured".into(),
        ))
    }

    /// Fetch a page as readable text.
    fn web_fetch(&self, _url: &str, _cancel: &CancelToken) -> Result<WebPage, ExtensionError> {
        Err(ExtensionError::Unavailable(
            "web fetch is not configured".into(),
        ))
    }

    /// The tools of enabled MCP servers offered in `workspace`, starting
    /// servers on demand. Servers that fail to start are left out.
    fn mcp_toolsets(&self, _workspace: Option<&str>, _cancel: &CancelToken) -> Vec<McpToolset> {
        Vec::new()
    }

    /// Call `tool` on MCP server `server`.
    fn mcp_call(
        &self,
        _server: &str,
        _tool: &str,
        _arguments: Value,
        _cancel: &CancelToken,
    ) -> Result<McpOutput, ExtensionError> {
        Err(ExtensionError::Unavailable(
            "MCP servers are unavailable".into(),
        ))
    }

    /// The offline switch, emergency stop, and the drive's policy settings
    /// that network tools are evaluated under.
    fn network(&self) -> NetworkState {
        NetworkState::default()
    }
}

/// No skills, web, or MCP servers.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoExtensions;

impl Extensions for NoExtensions {}
