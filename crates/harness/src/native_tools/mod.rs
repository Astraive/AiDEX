//! Harness-level tools that need no workspace: web search and fetch, skill
//! loading, artifacts, plans, and the dynamic tools of MCP servers.
//!
//! Model-facing names are stable (the desktop renders them specially):
//! `web_search`, `web_fetch`, `skill_load`, `artifact_create`,
//! `artifact_update`, `plan_propose`, and `mcp__<server>__<tool>`. They are the
//! model-safe forms of `web.search`, `web.fetch`, `skill.load`,
//! `artifact.create`, `artifact.update`, and `plan.propose` (dots become
//! underscores, as for the built-in `fs_write`).
mod exec;
mod mcp;
mod plan;
mod specs;

pub(crate) use mcp::{McpBinding, McpTarget};
pub(crate) use plan::ExtensionPlan;

use crate::{NativeEngine, engine::relock, extensions::SkillEntry};
use aidex_core::CancelToken;
use aidex_events::RiskLevel;
use aidex_native_sessions::{Mode, Run, SessionDetail};

pub(crate) const WEB_SEARCH: &str = "web_search";
pub(crate) const WEB_FETCH: &str = "web_fetch";
pub(crate) const SKILL_LOAD: &str = "skill_load";
pub(crate) const ARTIFACT_CREATE: &str = "artifact_create";
pub(crate) const ARTIFACT_UPDATE: &str = "artifact_update";
pub(crate) const PLAN_PROPOSE: &str = "plan_propose";
pub(crate) const MCP_PREFIX: &str = "mcp__";
/// Child-run tools of Agent mode (handled by the engine itself).
pub(crate) const AGENT_SPAWN: &str = "agent_spawn";
pub(crate) const AGENT_RESULT: &str = "agent_result";

/// Most model-invocable skills listed in the system prompt.
const MAX_CATALOG_SKILLS: usize = 64;

/// Whether results of the tool named `name` are untrusted outside data that
/// must be wrapped before the model reads them, and how.
pub(crate) fn untrusted_trust(name: &str) -> Option<&'static str> {
    match name {
        WEB_SEARCH | WEB_FETCH => Some("untrusted"),
        SKILL_LOAD => Some("project"),
        name if name.starts_with(MCP_PREFIX) => Some("untrusted"),
        _ => None,
    }
}

/// A harness-level tool resolved from a model call name.
#[derive(Clone, Debug)]
pub(crate) enum ExtTool {
    WebSearch,
    WebFetch,
    SkillLoad,
    ArtifactCreate,
    ArtifactUpdate,
    PlanPropose,
    Mcp(McpBinding),
}

impl ExtTool {
    /// Event tool id (dotted form).
    pub(crate) fn tool_id(&self) -> &'static str {
        match self {
            Self::WebSearch => "web.search",
            Self::WebFetch => "web.fetch",
            Self::SkillLoad => "skill.load",
            Self::ArtifactCreate => "artifact.create",
            Self::ArtifactUpdate => "artifact.update",
            Self::PlanPropose => "plan.propose",
            Self::Mcp(_) => "mcp.call",
        }
    }

    pub(crate) fn risk(&self) -> RiskLevel {
        match self {
            Self::WebFetch | Self::Mcp(_) => RiskLevel::Medium,
            _ => RiskLevel::Low,
        }
    }
}

/// The tools one request offers.
#[derive(Clone, Debug, Default)]
pub(crate) struct Offer {
    /// Workspace tools plus `agent_spawn`/`agent_result` (Agent mode).
    pub builtin: bool,
    pub web_search: bool,
    pub web_fetch: bool,
    /// Model-invocable skills; `skill_load` is offered when non-empty.
    pub skills: Vec<SkillEntry>,
    pub artifacts: bool,
    pub plan: bool,
    pub mcp: Vec<McpBinding>,
}

impl Offer {
    pub(crate) fn any(&self) -> bool {
        self.builtin
            || self.web_search
            || self.web_fetch
            || !self.skills.is_empty()
            || self.artifacts
            || self.plan
            || !self.mcp.is_empty()
    }

    /// Whether any offered tool returns untrusted data.
    pub(crate) fn has_untrusted(&self) -> bool {
        self.web_search || self.web_fetch || !self.skills.is_empty() || !self.mcp.is_empty()
    }
}

impl NativeEngine {
    /// Whether tools are switched on for this run at all: Agent and Plan
    /// always, Chat only when the request turned web tools on.
    fn tools_on(s: &SessionDetail, run: &Run) -> bool {
        match s.configuration.mode {
            Mode::Agent | Mode::Plan => true,
            Mode::Chat => run.web_search,
        }
    }

    /// The tools offered to the model for the next turn of `run`.
    /// `tool_calls` says whether the model calls tools natively.
    pub(crate) fn offer(
        &self,
        s: &SessionDetail,
        run: &Run,
        tool_calls: bool,
        cancel: &CancelToken,
    ) -> Offer {
        if !tool_calls || !Self::tools_on(s, run) {
            return Offer::default();
        }
        let mode = s.configuration.mode;
        let workspace = s.configuration.workspace_id.as_deref();
        let web = self.extensions.web();
        let web_search = web.search_ready();
        let mut skills = self.extensions.model_skills(workspace);
        skills.truncate(MAX_CATALOG_SKILLS);
        Offer {
            builtin: mode == Mode::Agent,
            web_search,
            web_fetch: web_search && web.fetch_enabled,
            skills,
            artifacts: true,
            plan: mode == Mode::Plan,
            mcp: if mode == Mode::Agent {
                mcp::bind(self.extensions.mcp_toolsets(workspace, cancel))
            } else {
                Vec::new()
            },
        }
    }

    /// Resolve a pending call to a harness-level tool that is still offered.
    /// `None` when `name` is not a harness-level tool name; `Some(Err)` when it
    /// is one that is not (or no longer) offered.
    pub(crate) fn resolve_tool(
        &self,
        s: &SessionDetail,
        run: &Run,
        name: &str,
        cancel: &CancelToken,
    ) -> Option<Result<ExtTool, String>> {
        let known = matches!(
            name,
            WEB_SEARCH | WEB_FETCH | SKILL_LOAD | ARTIFACT_CREATE | ARTIFACT_UPDATE | PLAN_PROPOSE
        ) || name.starts_with(MCP_PREFIX);
        if !known {
            return None;
        }
        let unavailable = || {
            Err(format!(
                "The tool `{name}` is not available in this conversation."
            ))
        };
        if !Self::tools_on(s, run) {
            return Some(unavailable());
        }
        let mode = s.configuration.mode;
        let workspace = s.configuration.workspace_id.as_deref();
        let resolved = match name {
            WEB_SEARCH => self
                .extensions
                .web()
                .search_ready()
                .then_some(ExtTool::WebSearch),
            WEB_FETCH => {
                let web = self.extensions.web();
                (web.search_ready() && web.fetch_enabled).then_some(ExtTool::WebFetch)
            }
            SKILL_LOAD => Some(ExtTool::SkillLoad),
            ARTIFACT_CREATE => Some(ExtTool::ArtifactCreate),
            ARTIFACT_UPDATE => Some(ExtTool::ArtifactUpdate),
            PLAN_PROPOSE => (mode == Mode::Plan).then_some(ExtTool::PlanPropose),
            // The server tool offered under this name, if it is still listed:
            // a fresh binding may give the name to another server's tool.
            _ if mode == Mode::Agent => self.offered_target(&run.id, name).and_then(|target| {
                mcp::bind(self.extensions.mcp_toolsets(workspace, cancel))
                    .into_iter()
                    .find(|b| b.server_id == target.server_id && b.tool == target.tool)
                    .map(|binding| {
                        ExtTool::Mcp(McpBinding {
                            name: target.name,
                            ..binding
                        })
                    })
            }),
            _ => None,
        };
        Some(resolved.ok_or(()).or_else(|()| unavailable()))
    }

    /// Record the MCP tools `offer` gives the next turn of `run`.
    pub(crate) fn remember_offer(&self, run: &str, offer: &Offer) {
        let mut offered = relock(&self.offered_mcp);
        if offer.mcp.is_empty() {
            offered.remove(run);
        } else {
            offered.insert(run.into(), offer.mcp.iter().map(McpTarget::from).collect());
        }
    }

    /// Forget what `run` was offered once it settles.
    pub(crate) fn forget_offer(&self, run: &str) {
        relock(&self.offered_mcp).remove(run);
    }

    fn offered_target(&self, run: &str, name: &str) -> Option<McpTarget> {
        relock(&self.offered_mcp)
            .get(run)?
            .iter()
            .find(|target| target.name == name)
            .cloned()
    }
}
