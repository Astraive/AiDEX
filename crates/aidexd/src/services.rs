//! Daemon-wide service composition.
//!
//! [`Services`] is built once at startup and shared with handlers as
//! `Arc<Services>`. Domain services (events, workspaces, sessions, vault, ...)
//! are added here as fields constructed in [`Services::open`]; their JSON-RPC
//! adapters live in `methods/<domain>.rs`.

pub mod contributions;
mod extensions;
mod inference;
mod mcp;
mod models;
mod runtime;
mod web;

pub use extensions::DaemonExtensions;
pub use models::{EventsModelObserver, ModelServices, VaultCredentials, model_event_kind};
pub use web::WebServices;

use crate::{endpoint, lifecycle::ShutdownSignal};
use aidex_broker::{Broker, BrokerConfig};
use aidex_context::ContextService;
use aidex_core::ServiceErrorCode;
use aidex_drive::DrivePaths;
use aidex_events::{EventLog, EventSink, RetentionPolicy};
use aidex_sessions::{RecoveryReport, SessionStore};
use aidex_skills::SkillService;
use aidex_updater::{RecoveryReport as UpdateRecovery, Updater, UpdaterConfig};
use aidex_vault::{VaultService, VaultServiceConfig};
use aidex_workspace::WorkspaceRegistry;
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Event database inside the drive's session directory (`DrivePaths::sessions()`).
pub const EVENT_FILE: &str = "events.sqlite3";
/// PR #40's event database inside `.aidex/runtime/`. Superseded: imported once into
/// [`EVENT_FILE`] when that log is empty, then left untouched and never written.
pub const LEGACY_EVENT_FILE: &str = "events.sqlite";
/// Durable events retained before the oldest expire.
pub const EVENT_RETENTION_EVENTS: u64 = 100_000;
/// Durable events older than this expire (90 days).
pub const EVENT_RETENTION_AGE: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// The daemon's event retention policy.
pub fn event_retention() -> RetentionPolicy {
    RetentionPolicy {
        max_events: Some(EVENT_RETENTION_EVENTS),
        max_age: Some(EVENT_RETENTION_AGE),
    }
}

/// Where the daemon keeps the event log on a drive.
pub fn event_log_path(paths: &DrivePaths) -> PathBuf {
    paths.sessions().join(EVENT_FILE)
}

pub struct Services {
    drive_root: PathBuf,
    paths: DrivePaths,
    pid: u32,
    started_at: Instant,
    shutdown: ShutdownSignal,
    events: EventLog,
    vault: Arc<VaultService>,
    workspaces: Arc<WorkspaceRegistry>,
    broker: Arc<Broker>,
    models: ModelServices,
    web: Arc<WebServices>,
    sessions: Arc<SessionStore>,
    session_recovery: RecoveryReport,
    context: Arc<ContextService>,
    skills: Arc<SkillService>,
    native: Arc<aidex_harness::NativeEngine>,
    inference: Arc<aidex_inference::InferenceService>,
    plugins: Arc<aidex_plugins::PluginStore>,
    runtime: Result<Arc<aidex_adapters::RuntimeManager>, String>,
    mcp: Result<Arc<aidex_mcp::McpSupervisor>, String>,
    updates: Result<Arc<Updater>, String>,
    update_recovery: Mutex<Option<Result<UpdateRecovery, String>>>,
}

impl Services {
    /// Open every domain service rooted at `drive_root`.
    ///
    /// Add each domain service here: open it, map its error with
    /// `ServicesError::new("<domain>", error.code(), error.to_string())`, and
    /// store it in a field with an accessor.
    pub fn open(drive_root: &Path, shutdown: ShutdownSignal) -> Result<Self, ServicesError> {
        Self::open_with(drive_root, shutdown, None)
    }

    /// [`open`](Self::open) with an explicit updater configuration (tests inject a
    /// trust store and self-test); `None` uses the binary's built-in trust store and
    /// the process self-test.
    pub fn open_with(
        drive_root: &Path,
        shutdown: ShutdownSignal,
        updater: Option<UpdaterConfig>,
    ) -> Result<Self, ServicesError> {
        if !drive_root.is_dir() {
            return Err(ServicesError::new(
                "daemon",
                ServiceErrorCode::Unavailable,
                format!("selected drive is unavailable: {}", drive_root.display()),
            ));
        }
        // Services run only on an initialized schema-v2 AiDEX drive; their state lives in the
        // directories its manifest declares. Opening never creates or repairs the manifest.
        let paths = DrivePaths::open(drive_root)
            .map_err(|error| ServicesError::new("drive", error.code(), error.to_string()))?;
        let state_error = |error: endpoint::EndpointError| {
            ServicesError::new("daemon", error.code(), error.to_string())
        };
        let runtime = endpoint::ensure_state_dir(drive_root).map_err(state_error)?;
        let events = open_events(&paths, &runtime.join(LEGACY_EVENT_FILE))?;
        let vault_dir = paths
            .ensure_dir(&paths.vault())
            .map_err(|error| ServicesError::new("vault", error.code(), error.to_string()))?;
        let vault = VaultService::open(vault_dir, VaultServiceConfig::default())
            .map_err(|error| ServicesError::new("vault", error.code(), error.to_string()))?;
        let workspaces =
            Arc::new(WorkspaceRegistry::open(&paths).map_err(|error| {
                ServicesError::new("workspace", error.code(), error.to_string())
            })?);
        let vault = Arc::new(vault);
        let broker = open_broker(&paths, &events, &workspaces, &vault)?;
        spawn_approval_expiry(&broker, &shutdown);
        // Model notifications are recorded as typed `model.*` events; remote provider
        // calls and their credentials pass the broker's policy.
        let models = ModelServices::open(
            &paths,
            Arc::clone(&vault),
            Arc::clone(&broker),
            Arc::new(EventsModelObserver::new(Arc::new(events.clone()))),
        )?;
        // Web search and fetch pass the offline switch and the broker's network
        // policy; provider keys come from the vault for `process:web`.
        let web = Arc::new(WebServices::open(
            &paths,
            Arc::clone(&broker),
            Arc::clone(models.offline()),
            Arc::clone(&vault),
            Arc::new(events.clone()),
        )?);
        let (sessions, session_recovery) = open_sessions(&paths, &events)?;
        let context = ContextService::open(&paths, Arc::new(events.clone()))
            .map_err(|error| ServicesError::new("context", error.code(), error.to_string()))?;
        // Skills are rescanned on every request; enablement lives in
        // `private_config/skills.toml` and changes emit `extension.*` events.
        let skills = Arc::new(SkillService::new(paths.clone(), Arc::new(events.clone())));
        let private_sessions = Arc::new(
            aidex_native_sessions::SessionStore::open(
                paths.sessions().join("native.sqlite3"),
                vault.clone(),
            )
            .map_err(|e| ServicesError::new("native", ServiceErrorCode::Unavailable, e))?,
        );
        // Installed plugin bundles; `extension.*` events go to the drive log.
        let plugins = Arc::new(
            aidex_plugins::PluginStore::open(&paths)
                .map_err(|error| ServicesError::new("plugins", error.code(), error.to_string()))?
                .with_events(Arc::new(events.clone())),
        );
        // MCP servers are optional: `mcp.*` reports why they are unavailable.
        let mcp = mcp::open_mcp(
            &paths,
            &broker,
            &vault,
            &workspaces,
            models.offline(),
            Arc::new(events.clone()),
            &shutdown,
        );
        // Native runs reach skills, web search and fetch, and MCP tools through
        // these hooks; network tools see the drive's policy and offline switch.
        let extensions = Arc::new(DaemonExtensions {
            skills: Arc::clone(&skills),
            plugins: Arc::clone(&plugins),
            workspaces: Arc::clone(&workspaces),
            web: Arc::clone(&web),
            mcp: mcp.as_ref().ok().cloned(),
            broker: Arc::clone(&broker),
            offline: Arc::clone(models.offline()),
            events: Arc::new(events.clone()),
        });
        let native = aidex_harness::NativeEngine::with_extensions(
            private_sessions,
            models.registry().clone(),
            workspaces.clone(),
            Arc::new(events.clone()),
            runtime.join("native-processes"),
            extensions,
        );
        let inference = inference::open_inference(&paths, &models)?;
        // External runtimes are optional: the daemon starts without them and
        // `runtime.*` reports why they are unavailable.
        let runtime = runtime::open_runtime(&paths, Arc::clone(&vault), &models)
            .map_err(|error| error.to_string());
        let updates = open_updates(&paths, &events, updater);
        Ok(Self {
            drive_root: drive_root.to_path_buf(),
            paths,
            pid: std::process::id(),
            started_at: Instant::now(),
            shutdown,
            events,
            vault,
            workspaces,
            broker,
            models,
            web,
            sessions: Arc::new(sessions),
            session_recovery,
            context: Arc::new(context),
            skills,
            native,
            inference,
            plugins,
            runtime,
            mcp,
            updates,
            update_recovery: Mutex::new(None),
        })
    }

    pub fn drive_root(&self) -> &Path {
        &self.drive_root
    }

    /// Absolute service directories declared by the drive manifest (`aidex-drive`).
    /// Build service paths from these accessors and create them with `ensure_dir`.
    pub fn paths(&self) -> &DrivePaths {
        &self.paths
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Ask the daemon to stop; the listener notices within one poll interval.
    pub fn shutdown(&self) -> &ShutdownSignal {
        &self.shutdown
    }

    /// The drive's durable event log (`aidex-events`); clone it to get an `EventSink`.
    pub fn events(&self) -> &EventLog {
        &self.events
    }

    /// The drive's encrypted vault (`aidex-vault`). Handle issuance/resolution for the
    /// broker are Rust methods on [`VaultService`]; they are not RPC methods.
    pub fn vault(&self) -> &Arc<VaultService> {
        &self.vault
    }

    /// Workspace registry and confinement (`aidex-workspace`).
    pub fn workspaces(&self) -> &WorkspaceRegistry {
        &self.workspaces
    }

    /// The execution broker (`aidex-broker`): tool execution for `tool.*`,
    /// broker approvals, policy settings, model egress, and emergency stop.
    pub fn broker(&self) -> &Arc<Broker> {
        &self.broker
    }

    /// Model providers, the router, and the offline switch
    /// (`aidex-providers`, `aidex-router`).
    pub fn models(&self) -> &ModelServices {
        &self.models
    }

    /// Web search and fetch settings and clients (`aidex-web`).
    pub fn web(&self) -> &Arc<WebServices> {
        &self.web
    }
    /// Durable sessions, checkpoints, and resume plans (`aidex-sessions`).
    pub fn sessions(&self) -> &Arc<SessionStore> {
        &self.sessions
    }

    /// What startup recovery found (interrupted sessions, lost tool calls, event gaps).
    pub fn session_recovery(&self) -> &RecoveryReport {
        &self.session_recovery
    }

    /// Local index, search, project rules, memory, and context packs
    /// (`aidex-context`). Dropping the services cancels running index jobs.
    pub fn context(&self) -> &Arc<ContextService> {
        &self.context
    }
    /// Skill discovery, enablement, and drive skill authoring (`aidex-skills`).
    pub fn skills(&self) -> &Arc<SkillService> {
        &self.skills
    }
    pub fn native(&self) -> &Arc<aidex_harness::NativeEngine> {
        &self.native
    }
    pub fn inference(&self) -> &Arc<aidex_inference::InferenceService> {
        &self.inference
    }
    /// Installed plugin bundles, their provenance, and trust (`aidex-plugins`).
    pub fn plugins(&self) -> &Arc<aidex_plugins::PluginStore> {
        &self.plugins
    }
    /// External agent runtimes (`aidex-adapters`), or why they could not be
    /// opened at startup.
    pub fn runtime(&self) -> Result<&Arc<aidex_adapters::RuntimeManager>, &str> {
        self.runtime.as_ref().map_err(String::as_str)
    }
    /// MCP server supervision (`aidex-mcp`), or why it could not be opened at startup.
    pub fn mcp(&self) -> Result<&Arc<aidex_mcp::McpSupervisor>, &str> {
        self.mcp.as_ref().map_err(String::as_str)
    }
    /// The drive's updater (`aidex-updater`), or why it is unavailable (a host without
    /// a bundle target, or an invalid built-in trust configuration).
    pub fn updates(&self) -> Result<&Arc<Updater>, &str> {
        self.updates.as_ref().map_err(String::as_str)
    }

    /// Recover an interrupted update or rollback. The daemon calls this once it owns
    /// the drive's endpoint and before it serves requests (the daemon is the drive's
    /// only updater, so no other process can be mid-update).
    pub fn recover_updates(&self) {
        let outcome = match &self.updates {
            Ok(updater) => updater.recover().map_err(|error| error.to_string()),
            Err(reason) => Err(reason.clone()),
        };
        if let Ok(mut slot) = self.update_recovery.lock() {
            *slot = Some(outcome);
        }
    }

    /// What update recovery found at startup (`None` before it ran).
    pub fn update_recovery(&self) -> Option<Result<UpdateRecovery, String>> {
        self.update_recovery
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }
}

/// The updater for this host's bundle target, emitting `update.*` into the drive log.
fn open_updates(
    paths: &DrivePaths,
    events: &EventLog,
    config: Option<UpdaterConfig>,
) -> Result<Arc<Updater>, String> {
    let config = match config {
        Some(config) => config,
        None => UpdaterConfig::builtin().map_err(|error| error.to_string())?,
    };
    let sink: Arc<dyn EventSink> = Arc::new(events.clone());
    Updater::for_host(paths.clone(), config)
        .map(|updater| Arc::new(updater.with_events(sink)))
        .map_err(|error| error.to_string())
}

/// How often pending approvals are checked for expiry.
const APPROVAL_EXPIRY_INTERVAL: Duration = Duration::from_millis(500);

/// Compose the execution broker over the drive: approvals in `private_sessions/`,
/// settings in `private_config/policy.toml`, policy documents in `policies/`, the
/// sandbox helper next to the daemon executable, and the drive vault.
fn open_broker(
    paths: &DrivePaths,
    events: &EventLog,
    workspaces: &Arc<WorkspaceRegistry>,
    vault: &Arc<VaultService>,
) -> Result<Arc<Broker>, ServicesError> {
    let broker_error = |error: aidex_broker::BrokerError| {
        ServicesError::new("broker", error.code(), error.to_string())
    };
    let config = BrokerConfig::for_drive(paths).map_err(broker_error)?;
    let sink: Arc<dyn EventSink> = Arc::new(events.clone());
    let broker = Broker::builder(config, Arc::clone(workspaces) as _, sink)
        .vault(Arc::clone(vault))
        .open()
        .map_err(broker_error)?;
    Ok(Arc::new(broker))
}

/// Expire due approvals in the background until shutdown (or the broker is gone).
fn spawn_approval_expiry(broker: &Arc<Broker>, shutdown: &ShutdownSignal) {
    let broker = Arc::downgrade(broker);
    let shutdown = shutdown.clone();
    // Expiry is also enforced when an approval is resolved, so a failed spawn only
    // delays `approval.expired` events.
    let _ = std::thread::Builder::new()
        .name("aidexd-approval-expiry".into())
        .spawn(move || {
            while !shutdown.is_requested() {
                std::thread::sleep(APPROVAL_EXPIRY_INTERVAL);
                let Some(broker) = broker.upgrade() else {
                    break;
                };
                let _ = broker.expire_due();
            }
        });
}

/// Open the session store with the drive event log as its sink, then recover from an
/// unclean stop before anything is served: active sessions become `interrupted`,
/// running tool calls `lost`, and unprovable events explicit gaps.
fn open_sessions(
    paths: &DrivePaths,
    events: &EventLog,
) -> Result<(SessionStore, RecoveryReport), ServicesError> {
    let session_error = |error: aidex_sessions::SessionError| {
        ServicesError::new("sessions", error.code(), error.to_string())
    };
    let sink: Arc<dyn EventSink> = Arc::new(events.clone());
    let store = SessionStore::open(paths, sink).map_err(session_error)?;
    let latest = events
        .latest_cursor()
        .map_err(|error| ServicesError::new("events", error.code(), error.to_string()))?;
    let report = store
        .recover_on_startup(store.host(), Some(latest))
        .map_err(session_error)?;
    Ok((store, report))
}

/// Open the drive's event log and import PR #40's superseded log once, if present.
fn open_events(paths: &DrivePaths, legacy: &Path) -> Result<EventLog, ServicesError> {
    let events_error = |error: aidex_events::EventError| {
        ServicesError::new("events", error.code(), error.to_string())
    };
    paths
        .ensure_dir(&paths.sessions())
        .map_err(|error| ServicesError::new("drive", error.code(), error.to_string()))?;
    let events = EventLog::open(event_log_path(paths), event_retention()).map_err(events_error)?;
    endpoint::refuse_symlink(legacy)
        .map_err(|error| ServicesError::new("events", error.code(), error.to_string()))?;
    if legacy.is_file() {
        events.import_legacy(legacy).map_err(events_error)?;
    }
    Ok(events)
}

/// Domain services may hold secrets or handles; never derive `Debug` here.
impl fmt::Debug for Services {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Services")
            .field("drive_root", &self.drive_root)
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

/// A domain service failed to open at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicesError {
    service: &'static str,
    code: ServiceErrorCode,
    message: String,
}

impl ServicesError {
    pub fn new(service: &'static str, code: ServiceErrorCode, message: impl Into<String>) -> Self {
        Self {
            service,
            code,
            message: message.into(),
        }
    }

    pub fn service(&self) -> &'static str {
        self.service
    }

    pub fn code(&self) -> ServiceErrorCode {
        self.code
    }
}

impl fmt::Display for ServicesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} service failed to start: {}",
            self.service, self.message
        )
    }
}

impl std::error::Error for ServicesError {}

/// Unit-test fixtures for daemon modules.
#[cfg(test)]
pub(crate) mod testing {
    /// A temporary, initialized schema-v2 AiDEX drive (no removable-media check).
    pub(crate) fn temp_drive() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("temp dir");
        aidex_drive::init_with(
            root.path(),
            aidex_drive::InitOptions {
                require_removable: false,
            },
        )
        .expect("initialize test drive");
        root
    }
}
