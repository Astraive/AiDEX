//! The daemon's skills, web, and MCP services lent to the native harness
//! (`aidex_harness::Extensions`).
//!
//! - Skills are rediscovered for every call (workspace, drive, and enabled
//!   plugin roots), so enable/disable and edits apply to the next turn.
//! - Web searches and fetches go through [`WebServices`], whose egress gate
//!   checks the offline switch, the emergency stop, and the drive's network
//!   policy again for every connection.
//! - MCP servers start on demand; servers that failed or need credentials are
//!   left for the user to restart. Plugin servers whose plugin stopped
//!   contributing are removed before tools are offered.

use super::{
    WebServices,
    contributions::{has_enabled_plugin_servers, plugin_skill_roots, sync_plugin_servers},
};
use aidex_broker::Broker;
use aidex_core::{CancelToken, EntityId};
use aidex_events::EventSink;
use aidex_harness::{
    ExtensionError, Extensions, McpOutput, McpTool, McpToolset, NetworkState, SkillEntry,
    SkillText, SkillTrust, WebHit, WebPage, WebStatus,
};
use aidex_mcp::{McpStatus, McpSupervisor};
use aidex_plugins::PluginStore;
use aidex_providers::OfflineMode;
use aidex_skills::{SkillCatalog, SkillId, SkillScope, SkillService, SkillWorkspace};
use aidex_workspace::WorkspaceRegistry;
use serde_json::Value;
use std::{sync::Arc, time::Duration};

/// How long one MCP server may take to start and list its tools for a turn.
///
/// At least the supervisor's own `initialize` (60 s) plus one request (30 s),
/// so a slow first start (an `npx` download) still starts on demand, and a
/// server that truly hangs is marked failed by the supervisor's own limit.
const MCP_START_TIMEOUT: Duration = Duration::from_secs(95);

/// Daemon services behind the harness's extension hooks.
pub struct DaemonExtensions {
    pub(super) skills: Arc<SkillService>,
    pub(super) plugins: Arc<PluginStore>,
    pub(super) workspaces: Arc<WorkspaceRegistry>,
    pub(super) web: Arc<WebServices>,
    pub(super) mcp: Option<Arc<McpSupervisor>>,
    pub(super) broker: Arc<Broker>,
    pub(super) offline: Arc<OfflineMode>,
    pub(super) events: Arc<dyn EventSink>,
}

fn unavailable(error: impl ToString) -> ExtensionError {
    ExtensionError::SkillUnavailable(error.to_string())
}

impl DaemonExtensions {
    /// The skills visible in `workspace` (an id), with plugin roots.
    fn catalog(&self, workspace: Option<&str>) -> Result<SkillCatalog, ExtensionError> {
        let workspace = workspace
            .and_then(|id| EntityId::new(id).ok())
            .and_then(|id| self.workspaces.handle(&id).ok())
            .map(|handle| SkillWorkspace {
                id: handle.id().clone(),
                root: handle.root().root().to_path_buf(),
            });
        self.skills
            .catalog(workspace.as_ref(), &plugin_skill_roots(&self.plugins))
            .map_err(unavailable)
    }

    fn text(catalog: &SkillCatalog, id: &SkillId) -> Result<SkillText, ExtensionError> {
        let body = catalog.load_body(id).map_err(unavailable)?;
        Ok(SkillText {
            id: id.to_string(),
            name: body.name,
            description: body.description,
            scope: id.scope().as_str().to_owned(),
            body: body.body,
            trust: if id.scope() == SkillScope::Drive {
                SkillTrust::User
            } else {
                SkillTrust::Project
            },
        })
    }

    fn supervisor(&self) -> Result<&Arc<McpSupervisor>, ExtensionError> {
        self.mcp
            .as_ref()
            .ok_or_else(|| ExtensionError::Unavailable("MCP servers are unavailable".into()))
    }
}

impl Extensions for DaemonExtensions {
    fn invoked_skills(
        &self,
        workspace: Option<&str>,
        ids: &[String],
    ) -> Result<Vec<SkillText>, ExtensionError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let catalog = self.catalog(workspace)?;
        ids.iter()
            .map(|id| {
                let id = SkillId::parse(id).map_err(unavailable)?;
                let summary = catalog.summary(&id).map_err(|_| {
                    unavailable(format!("skill `{id}` is not visible in this conversation"))
                })?;
                if !summary.user_invocable {
                    return Err(unavailable(format!(
                        "skill `{id}` cannot be invoked by the user"
                    )));
                }
                Self::text(&catalog, &id)
            })
            .collect()
    }

    fn model_skills(&self, workspace: Option<&str>) -> Vec<SkillEntry> {
        let Ok(catalog) = self.catalog(workspace) else {
            return Vec::new();
        };
        catalog
            .enabled()
            .filter(|summary| summary.model_invocable)
            .map(|summary| SkillEntry {
                name: summary.name.clone(),
                description: summary.description.clone(),
            })
            .collect()
    }

    fn load_skill(&self, workspace: Option<&str>, name: &str) -> Result<SkillText, ExtensionError> {
        let catalog = self.catalog(workspace)?;
        let summary = catalog
            .enabled()
            .find(|summary| summary.name == name && summary.model_invocable)
            .ok_or_else(|| unavailable(format!("no enabled skill named `{name}` can be loaded")))?;
        let id = SkillId::parse(&summary.id).map_err(unavailable)?;
        Self::text(&catalog, &id)
    }

    fn web(&self) -> WebStatus {
        let settings = match self.web.settings() {
            Ok(settings) => settings,
            Err(error) => {
                return WebStatus {
                    problem: Some(error.to_string()),
                    ..WebStatus::default()
                };
            }
        };
        // The host a search actually contacts: an endpoint left over from an
        // earlier SearXNG choice must not be the host policy is checked against.
        let search_target = settings.search_url().ok().and_then(|url| {
            Some((
                url.host_str()?
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_owned(),
                url.port_or_known_default()?,
            ))
        });
        WebStatus {
            enabled: settings.enabled,
            fetch_enabled: settings.fetch_enabled,
            provider: settings.provider.as_str().to_owned(),
            search_target,
            problem: settings.missing().map(|error| error.to_string()),
        }
    }

    fn web_search(
        &self,
        query: &str,
        max_results: Option<usize>,
        cancel: &CancelToken,
    ) -> Result<(String, Vec<WebHit>), ExtensionError> {
        let client = self
            .web
            .client()
            .map_err(|error| ExtensionError::Unavailable(error.to_string()))?;
        let provider = client.settings().provider.as_str().to_owned();
        let results = client
            .search(query, max_results, cancel)
            .map_err(|error| ExtensionError::Failed(error.to_string()))?;
        Ok((
            provider,
            results
                .into_iter()
                .map(|result| WebHit {
                    title: result.title,
                    url: result.url,
                    snippet: result.snippet,
                    published: result.published,
                })
                .collect(),
        ))
    }

    fn web_fetch(&self, url: &str, cancel: &CancelToken) -> Result<WebPage, ExtensionError> {
        let client = self
            .web
            .client()
            .map_err(|error| ExtensionError::Unavailable(error.to_string()))?;
        if !client.settings().fetch_enabled {
            return Err(ExtensionError::Unavailable(
                "web fetch is turned off in settings".into(),
            ));
        }
        let page = client
            .fetch(url, cancel)
            .map_err(|error| ExtensionError::Failed(error.to_string()))?;
        Ok(WebPage {
            url: page.url,
            final_url: page.final_url,
            status: page.status,
            content_type: page.content_type,
            title: page.title,
            text: page.text,
            truncated: page.truncated,
        })
    }

    fn mcp_toolsets(&self, workspace: Option<&str>, cancel: &CancelToken) -> Vec<McpToolset> {
        let Ok(supervisor) = self.supervisor() else {
            return Vec::new();
        };
        // A plugin that changed or was disabled must not keep offering tools.
        if has_enabled_plugin_servers(supervisor.store()) {
            let _ = sync_plugin_servers(&self.plugins, supervisor, self.events.as_ref());
        }
        let Ok(listing) = supervisor.store().list() else {
            return Vec::new();
        };
        let mut toolsets = Vec::new();
        for config in listing.servers {
            if !config.enabled || !config.applies_to(workspace) || cancel.is_stopped() {
                continue;
            }
            if matches!(
                supervisor.state(&config.id).status,
                McpStatus::Failed | McpStatus::NeedsAuth | McpStatus::Starting
            ) {
                continue;
            }
            let Ok(tools) =
                supervisor.list_tools(&config.id, &cancel.with_timeout(MCP_START_TIMEOUT))
            else {
                continue;
            };
            toolsets.push(McpToolset {
                server_id: config.id,
                server_name: config.name,
                auto_approve_tools: config.auto_approve_tools,
                tools: tools
                    .into_iter()
                    .map(|tool| McpTool {
                        name: tool.name,
                        description: tool.description,
                        input_schema: tool.input_schema,
                    })
                    .collect(),
            });
        }
        toolsets
    }

    fn mcp_call(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
        cancel: &CancelToken,
    ) -> Result<McpOutput, ExtensionError> {
        let result = self
            .supervisor()?
            .call_tool(server, tool, arguments, cancel)
            .map_err(|error| ExtensionError::Failed(error.message))?;
        Ok(McpOutput {
            text: result.content_text,
            is_error: result.is_error,
            truncated: result.truncated,
        })
    }

    fn network(&self) -> NetworkState {
        let blocked = if self.offline.is_offline() {
            Some("offline mode is on".to_owned())
        } else {
            self.broker
                .emergency_reason()
                .map(|reason| format!("an emergency stop is in effect: {reason}"))
        };
        NetworkState {
            blocked,
            context: self.broker.settings().context(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{lifecycle::ShutdownSignal, services::Services};
    use aidex_web::{SearchProvider, WebSettingsPatch};

    #[test]
    fn web_status_and_network_state_follow_the_drive() {
        let drive = crate::services::testing::temp_drive();
        let services = Services::open(drive.path(), ShutdownSignal::new()).unwrap();
        let extensions = services.native().extensions();
        let web = extensions.web();
        assert!(!web.enabled && !web.search_ready());
        assert_eq!(
            web.search_target,
            Some(("html.duckduckgo.com".to_owned(), 443))
        );
        services
            .web()
            .update(&WebSettingsPatch {
                enabled: Some(true),
                provider: Some(SearchProvider::Duckduckgo),
                ..WebSettingsPatch::default()
            })
            .unwrap();
        assert!(extensions.web().search_ready());
        services
            .web()
            .update(&WebSettingsPatch {
                provider: Some(SearchProvider::Brave),
                ..WebSettingsPatch::default()
            })
            .unwrap();
        let brave = extensions.web();
        assert!(brave.enabled && !brave.search_ready() && brave.problem.is_some());
        assert!(extensions.network().blocked.is_none());
        services.models().offline().set_offline(true).unwrap();
        assert_eq!(
            extensions.network().blocked.as_deref(),
            Some("offline mode is on")
        );
        assert!(
            extensions
                .mcp_toolsets(None, &aidex_core::CancelToken::new())
                .is_empty()
        );
        assert!(extensions.model_skills(None).is_empty());
    }

    #[test]
    fn the_search_target_is_the_host_a_search_contacts() {
        let drive = crate::services::testing::temp_drive();
        let services = Services::open(drive.path(), ShutdownSignal::new()).unwrap();
        let extensions = services.native().extensions();
        services
            .web()
            .update(&WebSettingsPatch {
                enabled: Some(true),
                provider: Some(SearchProvider::Searxng),
                endpoint: Some(Some("http://localhost:8888".into())),
                ..WebSettingsPatch::default()
            })
            .unwrap();
        assert_eq!(
            extensions.web().search_target,
            Some(("localhost".to_owned(), 8888))
        );
        // Switching provider keeps the SearXNG endpoint stored, but searches go to
        // the provider's own host, and that is the host policy must check.
        services
            .web()
            .update(&WebSettingsPatch {
                provider: Some(SearchProvider::Brave),
                ..WebSettingsPatch::default()
            })
            .unwrap();
        assert_eq!(
            services.web().settings().unwrap().endpoint.as_deref(),
            Some("http://localhost:8888")
        );
        assert_eq!(
            extensions.web().search_target,
            Some(("api.search.brave.com".to_owned(), 443))
        );
    }
}
