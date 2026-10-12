//! MCP servers (`aidex-mcp`) composed over the daemon's services.
//!
//! - Stdio servers launch through the broker's sandbox, so they live in the
//!   broker's process registry: emergency stop, daemon shutdown, and drive
//!   removal kill them with every other managed process.
//! - Secrets come from the drive vault, scoped to consumer `mcp:<id>`.
//! - Remote servers pass the offline switch, the emergency stop, and a
//!   `network.request` intent under the drive's policy settings and documents
//!   before every request.
//! - A watchdog stops every server when the daemon shuts down, the vault locks,
//!   or an emergency stop takes effect.
//! - Imported servers resolve their paths at launch: a plugin's bundled server
//!   inside its folder under `DrivePaths::plugins()`, a workspace's server in
//!   that workspace's registered root.

use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError, Weak},
    time::{Duration, Instant},
};

use aidex_broker::Broker;
use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use aidex_events::{EventKind, EventScope, EventSink};
use aidex_mcp::{
    EndpointUrl, McpConfigStore, McpError, McpErrorKind, McpGate, McpSupervisor, SandboxHost,
    ServerConfig, VaultSecrets,
};
use aidex_policy::{Action, Intent, PolicyError, PolicySet, Verdict, classify_network};
use aidex_providers::OfflineMode;
use aidex_vault::{VaultService, VaultState};
use aidex_workspace::WorkspaceRegistry;

use crate::lifecycle::ShutdownSignal;

/// How often the watchdog checks shutdown, the vault, and the emergency stop.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// Open the supervisor over `DrivePaths::mcp()` and start its watchdog.
pub fn open_mcp(
    paths: &DrivePaths,
    broker: &Arc<Broker>,
    vault: &Arc<VaultService>,
    workspaces: &Arc<WorkspaceRegistry>,
    offline: &Arc<OfflineMode>,
    events: Arc<dyn EventSink>,
    shutdown: &ShutdownSignal,
) -> Result<Arc<McpSupervisor>, String> {
    let store = McpConfigStore::open(paths).map_err(|error| error.message)?;
    let scratch = paths
        .ensure_dir(&paths.tmp().join("mcp"))
        .map_err(|error| error.to_string())?;
    let gate = Arc::new(DaemonGate {
        broker: Arc::downgrade(broker),
        offline: Arc::clone(offline),
        policies: broker.config().policies_dir.clone(),
        events: Arc::clone(&events),
        rate: Mutex::new(BTreeMap::new()),
    });
    let registry = Arc::clone(workspaces);
    let host = SandboxHost::new(
        broker.sandbox().clone(),
        scratch,
        Arc::new(VaultSecrets::new(Arc::clone(vault))),
        gate,
    )
    .with_events(events)
    // Imported servers resolve their paths at launch: a plugin's inside its
    // installed folder, a workspace's in the workspace root.
    .with_plugins_dir(paths.plugins())
    .with_workspace_roots(move |id| {
        let id = EntityId::new(id).ok()?;
        let root = registry.confined_root(&id).ok()?;
        Some(root.root().to_path_buf())
    });
    let supervisor = Arc::new(McpSupervisor::new(store, Arc::new(host)));
    spawn_watchdog(&supervisor, broker, vault, shutdown);
    Ok(supervisor)
}

/// The daemon's policy checks for MCP servers.
struct DaemonGate {
    broker: Weak<Broker>,
    offline: Arc<OfflineMode>,
    policies: Option<PathBuf>,
    events: Arc<dyn EventSink>,
    /// Recent request times per rate-limiting rule.
    rate: Mutex<BTreeMap<String, VecDeque<Instant>>>,
}

impl DaemonGate {
    fn broker(&self) -> Result<Arc<Broker>, McpError> {
        self.broker
            .upgrade()
            .ok_or_else(|| McpError::new(McpErrorKind::Unavailable, "the daemon is shutting down"))
    }

    fn documents(&self) -> Result<PolicySet, McpError> {
        let Some(dir) = &self.policies else {
            return Ok(PolicySet::new());
        };
        match PolicySet::load(dir) {
            Ok(set) => Ok(set),
            Err(PolicyError::MissingDirectory(_)) => Ok(PolicySet::new()),
            Err(error) => Err(McpError::new(
                McpErrorKind::PolicyDenied,
                format!(
                    "policy documents failed to load, so remote MCP servers are refused: {error}"
                ),
            )),
        }
    }

    fn within_rate(&self, key: &str, max: u32, per: Duration) -> bool {
        let mut windows = self.rate.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let window = windows.entry(key.to_owned()).or_default();
        while window
            .front()
            .is_some_and(|at| now.duration_since(*at) >= per)
        {
            window.pop_front();
        }
        if window.len() >= max as usize {
            return false;
        }
        window.push_back(now);
        true
    }

    fn record(&self, verdict: &Verdict, rule: String) {
        let Ok(intent_id) = EntityId::generate("int") else {
            return;
        };
        let decision = match verdict {
            Verdict::Allow => aidex_events::Verdict::Allow,
            Verdict::Deny => aidex_events::Verdict::Deny,
            Verdict::Sandbox { .. } => aidex_events::Verdict::Sandbox,
            Verdict::RequireApproval { .. } => aidex_events::Verdict::RequireApproval,
            Verdict::RateLimit { .. } => aidex_events::Verdict::RateLimit,
        };
        let _ = self.events.emit(
            EventScope::global(),
            EventKind::PolicyDecision {
                intent_id,
                decision,
                rule,
            },
        );
    }
}

fn emergency(broker: &Broker) -> Result<(), McpError> {
    match broker.emergency_reason() {
        Some(reason) => Err(McpError::new(
            McpErrorKind::PolicyDenied,
            format!("an emergency stop is in effect ({reason}); resume operations first"),
        )),
        None => Ok(()),
    }
}

impl McpGate for DaemonGate {
    fn check_launch(&self, _config: &ServerConfig) -> Result<(), McpError> {
        let broker = self.broker()?;
        emergency(&broker)
    }

    fn check_egress(&self, server_id: &str, url: &EndpointUrl) -> Result<(), McpError> {
        if !url.is_loopback() && self.offline.is_offline() {
            return Err(McpError::new(
                McpErrorKind::OfflineRefused,
                "AiDEX is offline; remote MCP servers are refused until network use is allowed",
            ));
        }
        let broker = self.broker()?;
        emergency(&broker)?;
        let target = classify_network(url.host(), url.port()).map_err(|error| {
            McpError::new(
                McpErrorKind::PolicyDenied,
                format!("the MCP server host cannot be classified: {error}"),
            )
        })?;
        let session = EntityId::new("ses_mcp_egress")
            .map_err(|_| McpError::new(McpErrorKind::Unavailable, "invalid egress session id"))?;
        let intent = Intent::new(
            Action::NetworkRequest,
            session,
            format!("connect to MCP server `{server_id}` at {}", url.origin()),
        )
        .with_network(target);
        let documents = self.documents()?;
        let decision = aidex_policy::evaluate(&documents, &broker.settings().context(), &intent);
        self.record(&decision.verdict, decision.rule.to_string());
        match &decision.verdict {
            Verdict::Allow | Verdict::Sandbox { .. } => Ok(()),
            Verdict::RateLimit { max, per } => {
                if self.within_rate(&format!("mcp|{}", decision.rule), *max, *per) {
                    Ok(())
                } else {
                    Err(McpError::new(
                        McpErrorKind::PolicyDenied,
                        format!("rate limit reached for MCP server `{server_id}`"),
                    ))
                }
            }
            Verdict::RequireApproval { .. } => Err(McpError::new(
                McpErrorKind::PolicyDenied,
                format!(
                    "connecting to {} needs approval under the current policy ({}); add a policy \
                     rule allowing `network.request` for this host, or choose another profile",
                    url.origin(),
                    decision.rule
                ),
            )),
            Verdict::Deny => Err(McpError::new(McpErrorKind::PolicyDenied, decision.reason)),
        }
    }
}

/// Stop every server on shutdown, on a vault lock, and on an emergency stop.
fn spawn_watchdog(
    supervisor: &Arc<McpSupervisor>,
    broker: &Arc<Broker>,
    vault: &Arc<VaultService>,
    shutdown: &ShutdownSignal,
) {
    let supervisor = Arc::downgrade(supervisor);
    let broker = Arc::downgrade(broker);
    let vault = Arc::downgrade(vault);
    let shutdown = shutdown.clone();
    // Without the watchdog, the broker still kills stdio servers on emergency
    // stop and shutdown; only HTTP sessions would stay open.
    let _ = std::thread::Builder::new()
        .name("aidexd-mcp-watchdog".into())
        .spawn(move || {
            let mut unlocked: Option<bool> = None;
            loop {
                std::thread::sleep(WATCH_INTERVAL);
                let Some(supervisor) = supervisor.upgrade() else {
                    break;
                };
                if shutdown.is_requested() {
                    supervisor.stop_all("the daemon is stopping");
                    break;
                }
                if broker
                    .upgrade()
                    .is_some_and(|broker| broker.emergency_reason().is_some())
                {
                    // The broker already killed stdio servers; this also ends
                    // HTTP sessions and resets every server to `stopped`.
                    supervisor.stop_all("stopped by the emergency stop");
                    continue;
                }
                if !supervisor.has_active() {
                    // The vault is read only while something runs (a locked
                    // vault's status reads its file).
                    unlocked = None;
                    continue;
                }
                let now = vault
                    .upgrade()
                    .is_some_and(|vault| vault.status().state == VaultState::Unlocked);
                if unlocked == Some(true) && !now {
                    supervisor.stop_all("stopped because the vault was locked");
                    unlocked = None;
                    continue;
                }
                unlocked = Some(now);
            }
        });
}
