//! What enabled, trusted plugins contribute: `plugin`-scope skill roots and
//! DISABLED MCP server entries imported from their `.mcp.json`.
//!
//! [`PluginStore::enabled_contributions`] re-verifies every enabled plugin's
//! tree hash, so a changed plugin stops contributing (and loses its trust) the
//! next time anything here runs. A plugin whose files cannot be read keeps its
//! trust but contributes nothing until they verify again: its servers are
//! stopped and removed like any other withdrawn contribution, and come back
//! disabled. Plugin server files carry
//! `source = { kind = "plugin", owner = "<plugin>" }`; [`sync_plugin_servers`]
//! adds the missing ones disabled and removes those whose plugin no longer
//! contributes them (stopping them first).

use aidex_core::EntityId;
use aidex_events::{EventKind, EventScope, EventSink, ExtensionKind};
use aidex_mcp::{McpConfigStore, McpError, McpSource, McpSourceKind, McpSupervisor, ServerConfig};
use aidex_plugins::PluginStore;
use aidex_skills::{ExtraSkillRoot, SkillScope};
use serde::Serialize;
use std::collections::BTreeMap;

/// Longest MCP server id (`aidex_mcp::MAX_ID_LEN`).
const MAX_SERVER_ID: usize = aidex_mcp::MAX_ID_LEN;

/// Skill roots of enabled, trusted plugins (scope `plugin`). A plugin store
/// that cannot be read contributes nothing.
pub fn plugin_skill_roots(plugins: &PluginStore) -> Vec<ExtraSkillRoot> {
    plugins
        .enabled_contributions()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|contribution| {
            Some(ExtraSkillRoot {
                scope: SkillScope::Plugin,
                root: contribution.skills_dir?,
                owner: contribution.name,
            })
        })
        .collect()
}

/// The server id of `entry` contributed by `plugin`: `<plugin>-<entry>`,
/// bounded to the id limit.
pub fn plugin_server_id(plugin: &str, entry: &str) -> String {
    let id = format!("{plugin}-{entry}");
    id.chars()
        .take(MAX_SERVER_ID)
        .collect::<String>()
        .trim_end_matches(['-', '_'])
        .to_owned()
}

/// What one synchronization changed.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PluginServerSync {
    /// Server ids added (disabled).
    pub added: Vec<String>,
    /// Server ids removed because their plugin no longer contributes them.
    pub removed: Vec<String>,
    /// Entries that could not be imported (no secret values are included).
    pub issues: Vec<String>,
}

/// Whether any enabled server came from a plugin.
pub fn has_enabled_plugin_servers(store: &McpConfigStore) -> bool {
    store.list().is_ok_and(|listing| {
        listing
            .servers
            .iter()
            .any(|config| config.enabled && config.source.kind == McpSourceKind::Plugin)
    })
}

/// Reconcile plugin-contributed MCP servers with the enabled, trusted
/// plugins. Errors reading the plugin store change nothing.
pub fn sync_plugin_servers(
    plugins: &PluginStore,
    supervisor: &McpSupervisor,
    events: &dyn EventSink,
) -> Result<PluginServerSync, McpError> {
    let contributions = plugins
        .enabled_contributions()
        .map_err(|error| McpError::new(aidex_mcp::McpErrorKind::Unavailable, error.to_string()))?;
    let store = supervisor.store();
    let mut report = PluginServerSync::default();
    let mut desired: BTreeMap<String, ServerConfig> = BTreeMap::new();
    for contribution in contributions {
        let Some(path) = contribution.mcp_json else {
            continue;
        };
        let source = McpSource::plugin(contribution.name.as_str());
        let entries = match McpConfigStore::import_mcp_json(&path, source) {
            Ok(entries) => entries,
            Err(error) => {
                report
                    .issues
                    .push(format!("{}: {}", contribution.name, error.message));
                continue;
            }
        };
        for entry in entries {
            let Some(mut config) = entry.config else {
                report.issues.push(format!(
                    "{}: {}: {}",
                    contribution.name,
                    entry.original_name,
                    entry.error.unwrap_or_else(|| "invalid entry".into())
                ));
                continue;
            };
            config.id = plugin_server_id(&contribution.name, &config.id);
            config.enabled = false;
            if let Err(error) = config.validate() {
                report.issues.push(format!(
                    "{}: {}: {}",
                    contribution.name, entry.original_name, error.message
                ));
                continue;
            }
            desired.entry(config.id.clone()).or_insert(config);
        }
    }
    let listing = store.list()?;
    for config in listing
        .servers
        .iter()
        .filter(|config| config.source.kind == McpSourceKind::Plugin)
    {
        let still = desired
            .get(&config.id)
            .is_some_and(|wanted| wanted.source.owner == config.source.owner);
        if still {
            continue;
        }
        supervisor.forget(&config.id);
        if store.remove(&config.id)? {
            report.removed.push(config.id.clone());
            if config.enabled
                && let Ok(extension_id) = EntityId::new(format!("mcp:{}", config.id))
            {
                // Best effort: the removal stands even if the event is lost.
                let _ = events.emit(
                    EventScope::global(),
                    EventKind::ExtensionDisabled {
                        extension_id,
                        kind: ExtensionKind::Mcp,
                    },
                );
            }
        }
    }
    for (id, config) in desired {
        if store.exists(&id) {
            let existing = store.get(&id)?;
            if existing.source.kind != McpSourceKind::Plugin {
                report.issues.push(format!(
                    "{id}: a server with this id already exists and was left unchanged"
                ));
            }
            continue;
        }
        match store.add(&config) {
            Ok(_) => report.added.push(id),
            Err(error) => report.issues.push(format!("{id}: {}", error.message)),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_server_ids_are_prefixed_and_bounded() {
        assert_eq!(plugin_server_id("review", "github"), "review-github");
        let long = plugin_server_id(&"p".repeat(60), "server-name");
        assert_eq!(long.len(), MAX_SERVER_ID);
        assert!(aidex_mcp::validate_id(&long).is_ok());
        assert_eq!(plugin_server_id(&"p".repeat(63), "x"), "p".repeat(63));
    }
}
