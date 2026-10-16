use crate::{
    profiles::{Accounts, profile_id, profile_record},
    runtime::NetworkGate,
    state::{State, error},
    *,
};
use aidex_auth::AuthService;
use aidex_native_sessions::{
    Mode, PermissionMode, RuntimeExecutor, RuntimeOperation, RuntimeRequest, RuntimeWorkspace,
};
use aidex_sandbox::{ProcessRegistry, Sandbox};
use aidex_vault::VaultService;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use zeroize::Zeroizing;
pub struct RuntimeManager {
    root: PathBuf,
    state: Arc<State>,
    configs: Arc<Configs>,
    auth: Arc<AuthService>,
    sandbox: Sandbox,
    network: NetworkGate,
    executors: Mutex<BTreeMap<String, (String, Arc<ExternalRuntime>)>>,
    /// Every executor this manager made, also those a configuration change
    /// replaced but the engine may still hold: their parked launches are
    /// released with the current ones.
    made: Mutex<Vec<Weak<ExternalRuntime>>>,
    /// Whether the last model-list probe of a runtime succeeded, per
    /// configuration signature: what `health` reports while a launch of the
    /// same managed account is live.
    probes: Mutex<BTreeMap<String, (String, bool)>>,
    /// Stale temporary roots the startup sweep removed.
    swept: usize,
    accounts: Accounts,
}
const CONFIG_RECORD: &str = "runtime-config.sealed";
/// The sealed runtime configuration list. Account logins switch a runtime to
/// its profile through it once they connect, outside any manager call.
pub(crate) struct Configs {
    state: Arc<State>,
    changes: Mutex<()>,
    /// Counts the list's writes in this process, so a holder of executors
    /// can tell that they may be stale (see [`RuntimeManager::generation`]).
    generation: AtomicU64,
}
impl Configs {
    pub(crate) fn new(state: Arc<State>) -> Self {
        Self {
            state,
            changes: Mutex::new(()),
            generation: AtomicU64::new(0),
        }
    }
    fn lock(&self) -> Result<MutexGuard<'_, ()>, RuntimeError> {
        self.changes.lock().map_err(|_| {
            error(
                "state_unavailable",
                "Runtime configuration lock unavailable",
            )
        })
    }
    fn list(&self) -> Result<Vec<RuntimeConfig>, RuntimeError> {
        let Some(bytes) = self.state.read(CONFIG_RECORD)? else {
            return Ok(vec![]);
        };
        serde_json::from_slice(&bytes)
            .map_err(|_| error("state_corrupt", "Encrypted runtime config is invalid"))
    }
    pub(crate) fn write(&self, configs: &[RuntimeConfig]) -> Result<(), RuntimeError> {
        let bytes = Zeroizing::new(
            serde_json::to_vec(configs)
                .map_err(|_| error("invalid_config", "Runtime config cannot be encoded"))?,
        );
        if bytes.len() > 64 * 1024 {
            return Err(error(
                "config_limit",
                "Runtime configuration exceeds 64 KiB",
            ));
        }
        // Counted before the write: a write that fails after it reached the
        // drive must not look like no change.
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.state.write(CONFIG_RECORD, &bytes)
    }
    /// The saved configuration of runtime `id`, if it is still configured.
    pub(crate) fn get(&self, id: &str) -> Result<Option<RuntimeConfig>, RuntimeError> {
        Ok(self.list()?.into_iter().find(|c| c.id == id))
    }
    /// Store a managed account with `store`, then point runtime `id` at its
    /// profile, which then is its only credential (`profile` set, `auth:
    /// none`). The list stays locked throughout, so a runtime removed
    /// meanwhile gets no account (`not_found` before `store` runs) and a
    /// configuration change never lands between the two.
    pub(crate) fn bind_profile(
        &self,
        id: &str,
        profile: &str,
        store: impl FnOnce() -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let _guard = self.lock()?;
        let mut configs = self.list()?;
        let config = configs
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| error("not_found", "Runtime configuration not found"))?;
        config.profile = Some(profile.to_owned());
        config.auth = RuntimeAuth::None;
        validate_config(config)?;
        store()?;
        self.write(&configs)
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentManifest {
    components: Vec<Component>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Component {
    driver: Driver,
    path: String,
    sha256: String,
}
#[derive(Debug, Serialize)]
pub struct InstallResult {
    pub driver: Driver,
    pub executable: String,
    pub verified_sha256: String,
}
impl RuntimeManager {
    pub fn open(
        drive_root: PathBuf,
        vault: Arc<VaultService>,
        auth: Arc<AuthService>,
        sandbox: Sandbox,
        network_gate: NetworkGate,
    ) -> Result<Self, RuntimeError> {
        let root = drive_root
            .canonicalize()
            .map_err(|_| error("state_unavailable", "Runtime drive unavailable"))?;
        // Before any launch or account command: the plaintext homes a crashed,
        // killed or unplugged daemon left in the host temporary folder.
        let swept = crate::state::sweep_stale_temp_roots(
            &crate::state::temp_base(),
            crate::state::TEMP_SWEEP_GRACE,
        );
        let state = Arc::new(State::new(root.join("private"), vault)?);
        let configs = Arc::new(Configs::new(state.clone()));
        let accounts = Accounts::new(
            state.clone(),
            configs.clone(),
            sandbox.clone(),
            network_gate.clone(),
        );
        Ok(Self {
            root,
            state,
            configs,
            auth,
            sandbox,
            network: network_gate,
            executors: Mutex::new(BTreeMap::new()),
            made: Mutex::new(Vec::new()),
            probes: Mutex::new(BTreeMap::new()),
            swept,
            accounts,
        })
    }
    /// How many stale temporary roots (each holding the homes of a crashed
    /// daemon's launches and account commands) the sweep at `open` removed.
    /// A count only: never paths or contents.
    pub fn swept_temporary_roots(&self) -> usize {
        self.swept
    }
    /// How many temporary homes this daemon holds now: running and parked
    /// launches and account commands. Each holds plaintext vendor state.
    pub fn temporary_homes(&self) -> usize {
        self.state.temp_homes()
    }
    pub fn catalog(&self) -> Vec<DriverInfo> {
        catalog()
    }
    /// Whether a runtime configuration was ever saved on this drive. Reads
    /// only file metadata: no vault access, so it never creates the runtime
    /// encryption key and works while the vault is locked.
    pub fn has_saved_configuration(&self) -> bool {
        self.state.root.join(CONFIG_RECORD).is_file()
    }
    pub fn list(&self) -> Result<Vec<RuntimeConfig>, RuntimeError> {
        self.configs.list()
    }
    pub fn get(&self, id: &str) -> Result<RuntimeConfig, RuntimeError> {
        self.list()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| error("not_found", "Runtime configuration not found"))
    }
    /// How many times this manager wrote the configuration list: a save, a
    /// removal, or a login or import that bound an account (a login binds
    /// from its worker thread, outside any call). Executors taken before the
    /// count last changed may be stale; [`Self::executors`] gives current
    /// ones. In memory only, starting at 0 for each manager.
    pub fn generation(&self) -> u64 {
        self.configs.generation.load(Ordering::SeqCst)
    }
    /// Add or replace a configuration as given (the whole of it, `profile`
    /// and `auth` included). Replacing it with one that drops or changes the
    /// managed account profile of a runtime whose account is still signed in
    /// (no other runtime using that profile) is refused with `conflict`:
    /// that would leave the account's credential on the drive with nothing
    /// to sign it out, and it is what a client saving a copy it read before a
    /// sign-in or import bound the account would do. Sign out first.
    pub fn save(&self, mut config: RuntimeConfig) -> Result<RuntimeConfig, RuntimeError> {
        let _guard = self.configs.lock()?;
        if !self.sandbox.registry().is_empty() {
            return Err(error(
                "runtime_busy",
                "Stop active external runtimes before changing configuration",
            ));
        }
        validate_config(&config)?;
        let executable = PathBuf::from(&config.executable);
        if executable.is_absolute()
            && let Ok(relative) = executable.strip_prefix(&self.root)
        {
            config.executable = portable(relative)?;
        }
        let mut configs = self.list()?;
        if let Some(stored) = configs.iter().find(|c| c.id == config.id)
            && let Some(bound) = &stored.profile
            && config.profile.as_ref() != Some(bound)
            && self.state.root.join(profile_record(bound)).is_file()
            && !configs
                .iter()
                .any(|other| other.id != config.id && profile_id(other) == *bound)
        {
            return Err(error(
                "conflict",
                "This runtime's managed account is signed in; sign it out before changing its account profile",
            ));
        }
        if let Some(old) = configs.iter_mut().find(|c| c.id == config.id) {
            *old = config.clone();
        } else {
            if configs.len() >= 64 {
                return Err(error(
                    "config_limit",
                    "At most 64 runtimes can be configured",
                ));
            }
            configs.push(config.clone());
        }
        self.configs.write(&configs)?;
        self.executors
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime registry unavailable"))?
            .remove(&config.id);
        Ok(config)
    }
    /// Remove a runtime with what AiDEX keeps for it: its pending logins are
    /// cancelled, its managed account (the encrypted credential and its
    /// revision, unless another runtime uses the same profile) and its
    /// native session snapshots are removed, and only then its
    /// configuration, so no credential stays on the drive that nothing could
    /// sign out. A removal that fails leaves the runtime configured (at
    /// worst signed out) to be removed again. The vendor's own `logout` is
    /// not run. Removing a signed-in account also removes every snapshot an
    /// older daemon saved under the legacy name, as a sign-out does.
    pub fn remove(&self, id: &str) -> Result<bool, RuntimeError> {
        // A login that finished now would bind an account to the runtime
        // being removed.
        self.accounts.cancel(Some(id))?;
        if !self.sandbox.registry().is_empty() {
            return Err(error(
                "runtime_busy",
                "Stop active external runtimes before removing configuration",
            ));
        }
        // The order a connecting login takes them in.
        let _account_guard = self.state.account_guard()?;
        let _guard = self.configs.lock()?;
        let mut configs = self.list()?;
        let Some(removed) = configs.iter().find(|c| c.id == id).cloned() else {
            return Ok(false);
        };
        configs.retain(|c| c.id != id);
        let profile = profile_id(&removed);
        if !configs.iter().any(|other| profile_id(other) == profile) {
            if self.state.root.join(profile_record(&profile)).is_file() {
                self.state.remove(&profile_record(&profile))?;
                self.state.remove_legacy_snapshots()?;
            }
            self.state.remove_account_revision(&profile)?;
        }
        // Nothing resumes the removed runtime's native sessions any more.
        self.state.remove_runtime_snapshots(id)?;
        self.configs.write(&configs)?;
        self.executors
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime registry unavailable"))?
            .remove(id);
        self.accounts.forget(id);
        Ok(true)
    }
    pub fn executors(&self) -> Result<Vec<Arc<dyn RuntimeExecutor>>, RuntimeError> {
        Ok(self
            .typed_executors()?
            .into_iter()
            .map(|e| e as Arc<dyn RuntimeExecutor>)
            .collect())
    }
    fn typed_executors(&self) -> Result<Vec<Arc<ExternalRuntime>>, RuntimeError> {
        let configs = self.list()?;
        let mut cache = self
            .executors
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime registry unavailable"))?;
        let mut out = vec![];
        cache.retain(|id, _| configs.iter().any(|c| c.enabled && c.id == *id));
        for config in configs.into_iter().filter(|c| c.enabled) {
            validate_config(&config)?;
            let signature = signature(&config);
            let entry = cache
                .entry(config.id.clone())
                .or_insert_with(|| (signature.clone(), self.make(config.clone())));
            if entry.0 != signature {
                *entry = (signature, self.make(config));
            }
            out.push(entry.1.clone());
        }
        Ok(out)
    }
    fn make(&self, config: RuntimeConfig) -> Arc<ExternalRuntime> {
        let executor = Arc::new(ExternalRuntime::new(
            config,
            self.state.clone(),
            self.configs.clone(),
            self.auth.clone(),
            self.sandbox.clone(),
            self.network.clone(),
        ));
        let mut made = self.made.lock().unwrap_or_else(PoisonError::into_inner);
        made.retain(|executor| executor.strong_count() > 0);
        made.push(Arc::downgrade(&executor));
        executor
    }
    /// Every live executor this manager made, current or replaced.
    fn made(&self) -> Vec<Arc<ExternalRuntime>> {
        self.made
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(Weak::upgrade)
            .collect()
    }
    /// Release every launch parked for an approval decision, in every
    /// executor: their trees are stopped, a credential the vendor refreshed
    /// is kept when the vault still allows it, and their temporary homes
    /// (restored native snapshots and managed credentials) are removed before
    /// this returns. For a vault lock, an emergency stop and going offline,
    /// after the runs were cancelled. The runs stay resumable: a later
    /// `Resume` launches from their saved references. Returns how many were
    /// released.
    pub fn release_paused(&self) -> usize {
        self.made().iter().map(|e| e.release_paused()).sum()
    }
    /// Release the launch parked for `run_id`, in whichever executor holds
    /// it (a cancelled, failed or abandoned run; see
    /// [`ExternalRuntime::release_run`]). Returns whether one was parked.
    pub fn release_run(&self, run_id: &str) -> bool {
        self.made().iter().filter(|e| e.release_run(run_id)).count() > 0
    }
    /// How many launches wait for an approval decision.
    pub fn parked_runs(&self) -> usize {
        self.made().iter().map(|e| e.parked_runs()).sum()
    }
    /// Remove the native session snapshot a saved reference names: a thread a
    /// configuration change ended, a refused resume, a deleted conversation.
    /// Only a branch snapshot of the reference's own runtime
    /// (`runtime-<digest(runtime_id)>-<64 hex>.sealed`) is removed, so a
    /// forged name never reaches another record. The runtime need not be
    /// configured any more. Best effort; returns whether one was removed.
    /// The caller makes sure no other saved reference still names it.
    pub fn forget_snapshot(&self, reference: &Value) -> bool {
        let Some(runtime) = reference.get("runtime_id").and_then(Value::as_str) else {
            return false;
        };
        crate::runtime::forget_snapshot(&self.state, runtime, reference)
    }
    /// Remove the snapshot a daemon from before branch-scoped snapshots kept
    /// for `session` of `runtime` (`runtime-<digest(runtime:session)>.sealed`),
    /// for a deleted conversation. Best effort; returns whether one was removed.
    pub fn forget_legacy_snapshot(&self, runtime: &str, session: &str) -> bool {
        let name = State::legacy_snapshot(runtime, session);
        self.state.root.join(&name).is_file() && self.state.remove(&name).is_ok()
    }
    fn executor(&self, id: &str) -> Result<Arc<ExternalRuntime>, RuntimeError> {
        self.typed_executors()?
            .into_iter()
            .find(|e| e.config.id == id)
            .ok_or_else(|| error("not_found", "Enabled runtime not found"))
    }
    pub fn process_registry(&self) -> &ProcessRegistry {
        self.sandbox.registry()
    }
    pub fn health(&self, id: &str) -> Result<RuntimeHealth, RuntimeError> {
        let config = self.get(id)?;
        let executable = self.executable(&config.executable)?;
        let installed = executable.is_file();
        let mut status = if installed {
            "installed_auth_unverified"
        } else {
            "binary_missing"
        }
        .to_owned();
        let mut authenticated = false;
        // A managed account's evidence is the official CLI's cached status,
        // reported in health's own codes (`RuntimeAccountStatus::
        // health_status`). A missing binary has none to give, and an account
        // whose status cannot be read stays unverified rather than failing
        // the health check.
        if installed
            && config.driver == Driver::Codex
            && config.profile.is_some()
            && matches!(config.auth, RuntimeAuth::None)
        {
            match self.account_status(id) {
                Ok(native) => {
                    authenticated = native.authenticated;
                    status = native.health_status().to_owned();
                }
                Err(e) if e.code == "vault_locked" => return Err(e),
                Err(_) => {}
            }
        }
        if let RuntimeAuth::CodexOauth { provider_id } = &config.auth {
            authenticated = self
                .auth
                .status(None, Some(provider_id))
                .is_ok_and(|s| s.state == aidex_auth::AuthState::Connected);
            if !authenticated {
                status = "auth_required".into();
            }
        }
        // Never start a model turn as a health check. Only supported model-list
        // protocols prove protocol readiness, with explicit auth still required.
        let mut protocol_ready = false;
        if installed
            && config.enabled
            && !self.state.login_pending(id)?
            && matches!(
                config.driver,
                Driver::Codex | Driver::Pi | Driver::Omp | Driver::Muse
            )
            && self.protocol_ready(id)
        {
            protocol_ready = true;
            status = if authenticated {
                "ready"
            } else {
                "protocol_ready_auth_unverified"
            }
            .into();
        }
        Ok(RuntimeHealth {
            id: id.into(),
            installed,
            protocol_ready,
            authenticated,
            status,
            limitations: config.driver.info().limitations,
        })
    }
    pub fn models(&self, id: &str) -> Result<Vec<RuntimeModel>, RuntimeError> {
        let r = RuntimeRequest {
            session_id: "runtime-probe".into(),
            run_id: "runtime-probe".into(),
            parent_run_id: None,
            operation: RuntimeOperation::Start,
            mode: Mode::Agent,
            workspace: Some(RuntimeWorkspace {
                workspace_id: "runtime-probe".into(),
                root: self.root.to_string_lossy().into_owned(),
            }),
            permission_mode: PermissionMode::FullAccess,
            instructions: String::new(),
            model: None,
            messages: vec![],
            max_steps: 1,
            max_tokens: 1,
            runtime_ref: None,
            approvals: vec![],
        };
        let executor = self.executor(id)?;
        let models = executor.discover_models(&r);
        self.probes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), (signature(&executor.config), models.is_ok()));
        models
    }
    /// Whether the runtime answers its model-list protocol. A probe restores
    /// a managed account's credential too, and the vendor rotates its refresh
    /// token: while a launch of the account is live, the last probe's result
    /// is reported instead of starting another process that could refresh
    /// the same token.
    fn protocol_ready(&self, id: &str) -> bool {
        let Ok(executor) = self.executor(id) else {
            return false;
        };
        if executor
            .config
            .profile
            .as_deref()
            .is_some_and(|profile| self.state.profile_in_use(profile))
        {
            let signature = signature(&executor.config);
            return self
                .probes
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(id)
                .is_some_and(|(probed, ready)| *probed == signature && *ready);
        }
        self.models(id).is_ok()
    }
    fn executable(&self, path: &str) -> Result<PathBuf, RuntimeError> {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            Ok(p)
        } else {
            portable(&p)?;
            Ok(self.root.join(p))
        }
    }
    pub fn install(
        &self,
        driver: Driver,
        source: Option<&str>,
    ) -> Result<InstallResult, RuntimeError> {
        let manifest_path = self.root.join("runtime-components.json");
        let manifest: ComponentManifest =
            serde_json::from_slice(&std::fs::read(manifest_path).map_err(|_| {
                error(
                    "installer_unavailable",
                    "No verified runtime component manifest is bundled",
                )
            })?)
            .map_err(|_| {
                error(
                    "installer_unavailable",
                    "Runtime component manifest is invalid",
                )
            })?;
        let component = manifest
            .components
            .into_iter()
            .find(|c| c.driver == driver && source.is_none_or(|s| s == c.path))
            .ok_or_else(|| {
                error(
                    "installer_unavailable",
                    "Requested runtime artifact is not declared by the bundle",
                )
            })?;
        let relative = portable(Path::new(&component.path))?;
        // Hashes while it stages the copy, so the verified bytes are the ones placed.
        crate::install::place(&self.root, driver, &relative, &component.sha256)
    }
    pub fn auth(&self) -> &Arc<AuthService> {
        &self.auth
    }
    /// A configuration with its executable resolved, for account commands.
    fn resolved(&self, id: &str) -> Result<RuntimeConfig, RuntimeError> {
        let mut config = self.get(id)?;
        config.executable = self
            .executable(&config.executable)?
            .to_string_lossy()
            .into_owned();
        Ok(config)
    }
    /// Whether an official CLI login is pending for any runtime.
    pub fn has_pending_logins(&self) -> bool {
        self.state.has_pending_logins()
    }
    /// Cancel every pending official login (vault lock, emergency stop, going
    /// offline). The manager stays usable: later logins and launches work.
    pub fn cancel_logins(&self) -> Result<(), RuntimeError> {
        self.accounts.cancel(None)
    }
    /// Daemon shutdown: cancel logins, release parked approvals (their homes
    /// are removed), then terminate and close the runtime process registry.
    /// No runtime process starts afterwards.
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        let cancelled = self.cancel_logins();
        self.release_paused();
        let report = self
            .sandbox
            .registry()
            .shutdown(aidex_sandbox::StopReason::DaemonShutdown);
        self.executors
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime registry unavailable"))?
            .clear();
        if !report.is_clean() {
            return Err(error(
                "process_shutdown_failed",
                "A native runtime tree could not be confirmed terminated",
            ));
        }
        cancelled
    }
    /// The managed account of a Codex runtime; other drivers fail `auth_unsupported`.
    pub fn account_status(&self, id: &str) -> Result<RuntimeAccountStatus, RuntimeError> {
        self.accounts.status(&self.resolved(id)?)
    }
    /// Import an explicitly selected absolute `auth.json` (or its directory) as
    /// the runtime's managed account, once the official CLI confirms it.
    pub fn import_profile(
        &self,
        id: &str,
        source: &Path,
    ) -> Result<RuntimeAccountStatus, RuntimeError> {
        let config = self.resolved(id)?;
        self.accounts.supported(&config)?;
        if !self.sandbox.registry().is_empty() {
            return Err(error(
                "runtime_busy",
                "Stop active external runtimes before importing an account",
            ));
        }
        let profile = profile_id(&config);
        self.accounts.import(&config, &profile, source)
    }
    /// Start the official CLI's browser login. The saved configuration is
    /// unchanged until the login connects; a failed or cancelled login leaves
    /// the runtime's previous authentication as it was.
    pub fn login_start(&self, id: &str) -> Result<RuntimeLoginStatus, RuntimeError> {
        let config = self.resolved(id)?;
        self.accounts.supported(&config)?;
        self.accounts.start(config)
    }
    pub fn login_status(
        &self,
        id: &str,
        flow_id: &str,
    ) -> Result<RuntimeLoginStatus, RuntimeError> {
        self.get(id)?;
        self.accounts.flow(id, flow_id, false)
    }
    pub fn login_cancel(
        &self,
        id: &str,
        flow_id: &str,
    ) -> Result<RuntimeLoginStatus, RuntimeError> {
        self.accounts.flow(id, flow_id, true)
    }
    /// Sign out and remove the managed account profile and every session
    /// snapshot of the runtime. A runtime without a managed account profile
    /// has nothing to sign out: `invalid_request`, before anything runs or
    /// is removed.
    pub fn logout(&self, id: &str) -> Result<RuntimeAccountStatus, RuntimeError> {
        let config = self.resolved(id)?;
        self.accounts.supported(&config)?;
        if config.profile.is_none() {
            return Err(error(
                "invalid_request",
                "This runtime has no managed account to sign out",
            ));
        }
        self.accounts.cancel(Some(id))?;
        if !self.sandbox.registry().is_empty() {
            return Err(error(
                "runtime_busy",
                "Stop active runtimes before signing out",
            ));
        }
        self.accounts.logout(&config)
    }
}
/// What identifies one configuration of a runtime: a change makes a new
/// executor.
fn signature(config: &RuntimeConfig) -> String {
    aidex_native_sessions::digest(serde_json::to_vec(config).unwrap_or_default().as_slice())
}
fn portable(path: &Path) -> Result<String, RuntimeError> {
    if path
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path.to_string_lossy().contains(':')
    {
        return Err(error(
            "invalid_config",
            "Managed runtime paths must be drive relative without traversal",
        ));
    }
    path.components()
        .map(|c| {
            c.as_os_str()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| error("invalid_config", "Runtime path is not UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join("/"))
}
fn validate_config(config: &RuntimeConfig) -> Result<(), RuntimeError> {
    if config.id.is_empty()
        || config.id.len() > 64
        || !config
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(error(
            "invalid_config",
            "Runtime ID must be 1-64 ASCII letters, numbers, dash or underscore",
        ));
    }
    // The native engine refuses an external runtime with this ID; saving one
    // would make every later runtime sync fail.
    if config.id == "native" {
        return Err(error(
            "invalid_config",
            "Runtime ID \"native\" is reserved for the built-in agent loop",
        ));
    }
    if config.executable.is_empty()
        || config.executable.len() > 4096
        || config.executable.contains('\0')
        || config.argv.len() > 64
        || config.argv.iter().any(|a| {
            a.len() > 4096 || a.contains('\0') || aidex_events::redact_text(a) != a.as_str()
        })
    {
        return Err(error(
            "invalid_config",
            "Use a literal executable and bounded credential-free argv; store credentials in the vault",
        ));
    }
    let p = Path::new(&config.executable);
    if !p.is_absolute() {
        portable(p)?;
    }
    if config.profile.as_ref().is_some_and(|p| {
        p.len() > 64
            || p.is_empty()
            || !p
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        return Err(error(
            "invalid_config",
            "Profile is a managed encrypted profile ID, not a host auth directory",
        ));
    }
    match &config.auth {
        RuntimeAuth::CodexOauth { provider_id }
            if config.driver != Driver::Codex || provider_id.is_empty() =>
        {
            return Err(error(
                "invalid_config",
                "Shared Codex OAuth requires Codex and a provider ID",
            ));
        }
        RuntimeAuth::Vault {
            env_name,
            secret_name,
        } => {
            let expected = match config.driver {
                Driver::Codex => "OPENAI_API_KEY",
                Driver::ClaudeCode => "ANTHROPIC_API_KEY",
                Driver::Cursor => "CURSOR_API_KEY",
                Driver::Grok => "XAI_API_KEY",
                Driver::Antigravity => "GEMINI_API_KEY",
                Driver::Muse => "META_API_KEY",
                Driver::Pi | Driver::Omp | Driver::Opencode | Driver::Acp => env_name,
            };
            if env_name != expected
                || !env_name.ends_with("_API_KEY")
                || env_name.len() > 64
                || !env_name
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b == b'_')
                || SecretName::new(secret_name).is_err()
            {
                return Err(error(
                    "invalid_config",
                    "Credential environment must be a documented API key variable",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}
use aidex_vault::SecretName;
