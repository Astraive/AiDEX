//! `mcp.*` methods over the MCP supervisor (`aidex-mcp`).
//!
//! Server configuration is literal: commands, arguments, environment variable
//! *names*, URLs, and vault secret *names*. No method accepts or returns a
//! secret value; values are stored with `vault.secret.set` under consumer
//! `mcp:<id>`. Servers are disabled until the user enables them, start on
//! demand (`mcp.tools`) or explicitly (`mcp.start`), and are stopped when their
//! configuration changes, when they are disabled or removed, on vault lock,
//! emergency stop, and daemon shutdown.

use crate::{
    errors::{ErrorCode, RpcError},
    methods::{
        context::resolve_handle, context::resolve_workspace, native_view::WIRE_BUDGET, record_event,
    },
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_core::{CancelToken, EntityId, Timestamp};
use aidex_events::{EventKind, EventScope, ExtensionKind};
use aidex_mcp::{
    ImportedServer, LogLine, McpConfigStore, McpError, McpErrorKind, McpSource, McpSourceKind,
    McpStatus, McpSupervisor, McpToolSpec, ServerConfig, ServerIdentity, ServerSpec, ServerState,
    StatusError, TransportKind,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub const LIST: &str = "mcp.list";
pub const ADD: &str = "mcp.add";
pub const UPDATE: &str = "mcp.update";
pub const REMOVE: &str = "mcp.remove";
pub const SET_ENABLED: &str = "mcp.set_enabled";
pub const START: &str = "mcp.start";
pub const STOP: &str = "mcp.stop";
pub const RESTART: &str = "mcp.restart";
pub const TOOLS: &str = "mcp.tools";
pub const LOGS: &str = "mcp.logs";
pub const IMPORT: &str = "mcp.import";

/// Log lines returned by `mcp.logs` unless `limit` says otherwise.
const DEFAULT_LOG_LINES: usize = 200;
/// Longest tool description in `mcp.list` (full text is in `mcp.tools`).
const LIST_DESCRIPTION_BYTES: usize = 256;
/// The `.mcp.json` file `mcp.import` reads from a workspace root.
const MCP_JSON: &str = ".mcp.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnabledParams {
    id: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LogsParams {
    id: String,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportParams {
    #[serde(default)]
    workspace_id: Option<String>,
    /// Report what would be imported without writing anything.
    #[serde(default)]
    dry_run: bool,
}

/// A tool as listed by `mcp.list`.
#[derive(Clone, Debug, Serialize)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
}

/// One server as clients see it: configuration (no secret values) plus state.
#[derive(Clone, Debug, Serialize)]
pub struct ServerView {
    pub id: String,
    pub name: String,
    pub transport: TransportKind,
    /// The command line (redacted, bounded) or the URL origin.
    pub endpoint: String,
    pub enabled: bool,
    pub status: McpStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<StatusError>,
    pub tool_count: usize,
    pub tools: Vec<ToolSummary>,
    pub source: McpSourceKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_owner: Option<String>,
    /// Vault secret names the server needs (consumer `mcp:<id>`).
    pub secret_names: Vec<String>,
    pub config: ServerConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
}

impl ServerView {
    fn new(config: ServerConfig, state: ServerState) -> Self {
        let tools = state
            .tools
            .iter()
            .map(|tool| ToolSummary {
                name: tool.name.clone(),
                description: bounded(&tool.description, LIST_DESCRIPTION_BYTES),
            })
            .collect();
        Self {
            id: config.id.clone(),
            name: config.name.clone(),
            transport: config.transport,
            endpoint: config.endpoint_summary(),
            enabled: config.enabled,
            status: state.status,
            error: state.error,
            tool_count: state.tools.len(),
            tools,
            source: config.source.kind,
            source_owner: config.source.owner.clone(),
            secret_names: config.secret_names(),
            server: state.server,
            started_at: state.started_at,
            config,
        }
    }
}

/// A server file that could not be loaded.
#[derive(Clone, Debug, Serialize)]
pub struct InvalidView {
    pub id: String,
    pub error: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ListResult {
    pub servers: Vec<ServerView>,
    pub invalid: Vec<InvalidView>,
    /// Tool lists were left out to fit the message limit (`mcp.tools` has them).
    pub tools_omitted: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ToolsResult {
    pub id: String,
    pub tools: Vec<McpToolSpec>,
    pub total: usize,
    /// Later tools were left out to fit the message limit.
    pub truncated: bool,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LogsResult {
    pub id: String,
    pub lines: Vec<LogLine>,
    pub dropped: u64,
}

/// What `mcp.import` did with one `.mcp.json` entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    /// Saved as a new, disabled server.
    Added,
    /// `dry_run`: would be added.
    New,
    /// A server with this id already exists; it was left unchanged.
    Exists,
    /// The entry cannot become a valid server.
    Invalid,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportEntry {
    pub status: ImportStatus,
    #[serde(flatten)]
    pub entry: ImportedServer,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImportResult {
    pub workspace_id: EntityId,
    pub path: &'static str,
    pub dry_run: bool,
    pub servers: Vec<ImportEntry>,
}

fn bounded(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Map an MCP error to its wire error with `data.reason`.
pub(crate) fn mcp_error(error: McpError) -> RpcError {
    let code = match error.kind {
        McpErrorKind::InvalidConfig | McpErrorKind::InvalidRequest => ErrorCode::InvalidArgument,
        McpErrorKind::NotFound => ErrorCode::NotFound,
        McpErrorKind::Conflict | McpErrorKind::Disabled => ErrorCode::Conflict,
        McpErrorKind::VaultLocked => ErrorCode::VaultLocked,
        McpErrorKind::PolicyDenied => ErrorCode::PolicyDenied,
        McpErrorKind::Cancelled => ErrorCode::Cancelled,
        McpErrorKind::Io
        | McpErrorKind::NeedsAuth
        | McpErrorKind::OfflineRefused
        | McpErrorKind::LaunchFailed
        | McpErrorKind::Protocol
        | McpErrorKind::OutputLimit
        | McpErrorKind::ServerError
        | McpErrorKind::Disconnected
        | McpErrorKind::SessionExpired
        | McpErrorKind::Timeout
        | McpErrorKind::Unavailable => ErrorCode::Unavailable,
    };
    RpcError::new(code, error.message).with_data(json!({"reason": error.kind.as_str()}))
}

/// The supervisor, or `unavailable` when it failed to open at startup.
fn supervisor(services: &Services) -> Result<&Arc<McpSupervisor>, RpcError> {
    services.mcp().map_err(|reason| {
        RpcError::new(
            ErrorCode::Unavailable,
            format!("MCP servers are unavailable: {reason}"),
        )
        .with_data(json!({"reason": "mcp_unavailable"}))
    })
}

fn store(services: &Services) -> Result<&McpConfigStore, RpcError> {
    supervisor(services).map(|supervisor| supervisor.store())
}

fn view(supervisor: &McpSupervisor, config: ServerConfig) -> ServerView {
    let state = supervisor.state(&config.id);
    ServerView::new(config, state)
}

/// `extension.enabled`/`extension.disabled` for `mcp:<id>`.
fn record_enabled(services: &Services, id: &str, enabled: bool) -> Result<(), RpcError> {
    let extension_id = EntityId::new(format!("mcp:{id}"))
        .map_err(|error| RpcError::internal(format!("invalid extension id: {error}")))?;
    let kind = ExtensionKind::Mcp;
    let event = if enabled {
        EventKind::ExtensionEnabled { extension_id, kind }
    } else {
        EventKind::ExtensionDisabled { extension_id, kind }
    };
    record_event(services, EventScope::global(), event)
}

/// A bounded wait for one start, listing, or restart.
fn cancel() -> CancelToken {
    CancelToken::new()
}

/// Bring plugin-contributed servers in line with the enabled, trusted plugins
/// (best effort: a plugin store that cannot be read changes nothing).
pub(crate) fn sync_plugins(services: &Services) {
    if let Ok(supervisor) = services.mcp() {
        let _ = crate::services::contributions::sync_plugin_servers(
            services.plugins(),
            supervisor,
            services.events(),
        );
    }
}

/// Re-verify the plugin behind server `id` before anything may launch it, so a
/// plugin whose files changed since the last check loses its servers (and its
/// bundled code never runs) instead of starting.
fn reverify_plugin(services: &Services, supervisor: &McpSupervisor, id: &str) {
    if supervisor
        .store()
        .get(id)
        .is_ok_and(|config| config.source.kind == McpSourceKind::Plugin)
    {
        sync_plugins(services);
    }
}

fn list(services: &Services, params: ListParams) -> Result<ListResult, RpcError> {
    let supervisor = supervisor(services)?;
    sync_plugins(services);
    let workspace = match params.workspace_id.as_deref().map(str::trim) {
        Some(reference) if !reference.is_empty() => {
            Some(resolve_workspace(services, Some(reference))?.id)
        }
        _ => None,
    };
    let listing = supervisor.store().list().map_err(mcp_error)?;
    let servers = listing
        .servers
        .into_iter()
        .filter(|config| config.applies_to(workspace.as_ref().map(EntityId::as_str)))
        .map(|config| view(supervisor, config))
        .collect();
    let mut result = ListResult {
        servers,
        invalid: listing
            .invalid
            .into_iter()
            .map(|invalid| InvalidView {
                id: invalid.id,
                error: invalid.error,
            })
            .collect(),
        tools_omitted: false,
    };
    let size = serde_json::to_vec(&result).map_or(usize::MAX, |bytes| bytes.len());
    if size > WIRE_BUDGET {
        for server in &mut result.servers {
            server.tools.clear();
        }
        result.tools_omitted = true;
    }
    Ok(result)
}

fn tools(supervisor: &McpSupervisor, id: &str) -> Result<ToolsResult, RpcError> {
    let all = supervisor.list_tools(id, &cancel()).map_err(mcp_error)?;
    let notes = supervisor.state(id).notes;
    let total = all.len();
    let mut used = 1024 + serde_json::to_vec(&notes).map_or(0, |bytes| bytes.len());
    let mut tools = Vec::new();
    for tool in all {
        let size = serde_json::to_vec(&tool).map_or(usize::MAX, |bytes| bytes.len()) + 1;
        if used.saturating_add(size) > WIRE_BUDGET {
            break;
        }
        used += size;
        tools.push(tool);
    }
    Ok(ToolsResult {
        id: id.to_owned(),
        truncated: tools.len() < total,
        tools,
        total,
        notes,
    })
}

fn import(services: &Services, params: ImportParams) -> Result<ImportResult, RpcError> {
    let store = store(services)?;
    let handle = resolve_handle(services, params.workspace_id.as_deref())?;
    let workspace_id = handle.id().clone();
    let path = handle.root().root().join(MCP_JSON);
    let imported =
        McpConfigStore::import_mcp_json(&path, McpSource::workspace(workspace_id.as_str()))
            .map_err(mcp_error)?;
    let mut servers = Vec::new();
    for mut entry in imported {
        let status = match &entry.config {
            None => ImportStatus::Invalid,
            Some(config) if store.exists(&config.id) => ImportStatus::Exists,
            Some(_) if params.dry_run => ImportStatus::New,
            Some(config) => match store.add(config) {
                Ok(_) => ImportStatus::Added,
                Err(error) if error.kind == McpErrorKind::Conflict => ImportStatus::Exists,
                Err(error) => {
                    entry.error = Some(error.message);
                    ImportStatus::Invalid
                }
            },
        };
        servers.push(ImportEntry { status, entry });
    }
    Ok(ImportResult {
        workspace_id,
        path: MCP_JSON,
        dry_run: params.dry_run,
        servers,
    })
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(LIST, move |_context, params| {
        to_result(&list(&shared, parse_params(params)?)?)
    })?;

    let shared = Arc::clone(services);
    router.register(ADD, move |_context, params| {
        let spec: ServerSpec = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        let config = supervisor
            .store()
            .add(&spec.into_config(McpSource::drive()))
            .map_err(mcp_error)?;
        if config.enabled {
            record_enabled(&shared, &config.id, true)?;
        }
        to_result(&view(supervisor, config))
    })?;

    let shared = Arc::clone(services);
    router.register(UPDATE, move |_context, params| {
        let spec: ServerSpec = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        let existing = supervisor.store().get(&spec.id).map_err(mcp_error)?;
        let config = supervisor
            .store()
            .update(&spec.into_config(existing.source.clone()))
            .map_err(mcp_error)?;
        // A running server keeps its old launch parameters; stop it so the next
        // start uses the new ones.
        supervisor.stop(&config.id);
        if config.enabled != existing.enabled {
            record_enabled(&shared, &config.id, config.enabled)?;
        }
        to_result(&view(supervisor, config))
    })?;

    let shared = Arc::clone(services);
    router.register(REMOVE, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        supervisor.forget(&params.id);
        let removed = supervisor.store().remove(&params.id).map_err(mcp_error)?;
        Ok(json!({"removed": removed}))
    })?;

    let shared = Arc::clone(services);
    router.register(SET_ENABLED, move |_context, params| {
        let params: EnabledParams = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        let before = supervisor.store().get(&params.id).map_err(mcp_error)?;
        let config = supervisor
            .store()
            .set_enabled(&params.id, params.enabled)
            .map_err(mcp_error)?;
        if !config.enabled {
            supervisor.stop(&config.id);
        }
        if before.enabled != config.enabled {
            record_enabled(&shared, &config.id, config.enabled)?;
        }
        to_result(&view(supervisor, config))
    })?;

    let shared = Arc::clone(services);
    router.register(START, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        reverify_plugin(&shared, supervisor, &params.id);
        supervisor.start(&params.id, &cancel()).map_err(mcp_error)?;
        let config = supervisor.store().get(&params.id).map_err(mcp_error)?;
        to_result(&view(supervisor, config))
    })?;

    let shared = Arc::clone(services);
    router.register(STOP, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        let config = supervisor.store().get(&params.id).map_err(mcp_error)?;
        supervisor.stop(&params.id);
        to_result(&view(supervisor, config))
    })?;

    let shared = Arc::clone(services);
    router.register(RESTART, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        reverify_plugin(&shared, supervisor, &params.id);
        supervisor
            .restart(&params.id, &cancel())
            .map_err(mcp_error)?;
        let config = supervisor.store().get(&params.id).map_err(mcp_error)?;
        to_result(&view(supervisor, config))
    })?;

    let shared = Arc::clone(services);
    router.register(TOOLS, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let supervisor = supervisor(&shared)?;
        reverify_plugin(&shared, supervisor, &params.id);
        to_result(&tools(supervisor, &params.id)?)
    })?;

    let shared = Arc::clone(services);
    router.register(LOGS, move |_context, params| {
        let params: LogsParams = parse_params(params)?;
        let limit = params.limit.unwrap_or(DEFAULT_LOG_LINES);
        if !(1..=aidex_mcp::MAX_LOG_LINES).contains(&limit) {
            return Err(RpcError::invalid_params(format!(
                "limit must be 1-{}",
                aidex_mcp::MAX_LOG_LINES
            )));
        }
        let supervisor = supervisor(&shared)?;
        supervisor.store().get(&params.id).map_err(mcp_error)?;
        let tail = supervisor.logs(&params.id, limit);
        to_result(&LogsResult {
            id: params.id,
            lines: tail.lines,
            dropped: tail.dropped,
        })
    })?;

    let shared = Arc::clone(services);
    router.register(IMPORT, move |_context, params| {
        to_result(&import(&shared, parse_params(params)?)?)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests;
