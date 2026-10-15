//! `runtime.*` methods over external agent runtimes (`aidex-adapters`).
//!
//! Runtime configuration is literal: executables, argv, and vault secret
//! *names*. Credentials never enter these DTOs. The configuration is sealed
//! with a vault key, so everything except `runtime.catalog` needs the vault
//! unlocked; the native engine only runs runtimes [`sync_runtimes`] loaded.
//!
//! Managed accounts (`runtime.account.status`, `runtime.profile.import`,
//! `runtime.login.*`) run the runtime's official CLI in an app-owned temporary
//! home and keep only an encrypted profile. Their results are status codes and
//! the login's authorization URL: never credential bytes, file contents, or CLI
//! output. While the runtime gate is closed (offline, a locked vault, or an
//! emergency stop) none of them starts the official CLI: `runtime.account.status`
//! and `runtime.health` answer from what the CLI last confirmed, and an import,
//! login or sign-out is refused with `offline_refused`.
use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_adapters::{
    Driver, RuntimeAccountStatus, RuntimeAuth, RuntimeConfig, RuntimeError, RuntimeHealth,
    RuntimeManager,
};
use aidex_native_sessions::{RuntimeExecutor, SessionConfig};
use aidex_vault::VaultState;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const RUNTIME_CATALOG: &str = "runtime.catalog";
pub const RUNTIME_LIST: &str = "runtime.list";
pub const RUNTIME_ADD: &str = "runtime.add";
pub const RUNTIME_UPDATE: &str = "runtime.update";
pub const RUNTIME_CONFIG_SAVE: &str = "runtime.config.save";
pub const RUNTIME_REMOVE: &str = "runtime.remove";
pub const RUNTIME_CONFIG_GET: &str = "runtime.config.get";
pub const RUNTIME_HEALTH: &str = "runtime.health";
pub const RUNTIME_MODELS: &str = "runtime.models";
pub const RUNTIME_INSTALL: &str = "runtime.install";
pub const RUNTIME_AUTH_START: &str = "runtime.auth.start";
pub const RUNTIME_AUTH_STATUS: &str = "runtime.auth.status";
pub const RUNTIME_AUTH_CANCEL: &str = "runtime.auth.cancel";
pub const RUNTIME_ACCOUNT_STATUS: &str = "runtime.account.status";
pub const RUNTIME_PROFILE_IMPORT: &str = "runtime.profile.import";
pub const RUNTIME_LOGIN_START: &str = "runtime.login.start";
pub const RUNTIME_LOGIN_STATUS: &str = "runtime.login.status";
pub const RUNTIME_LOGIN_CANCEL: &str = "runtime.login.cancel";
pub const RUNTIME_LOGIN_LOGOUT: &str = "runtime.login.logout";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    driver: Driver,
    #[serde(default)]
    source: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Auth {
    id: String,
    #[serde(default)]
    flow_id: Option<String>,
}

/// Params of `runtime.profile.import`: an absolute `auth.json` or its directory.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    id: String,
    source: PathBuf,
}
/// Params of `runtime.login.status` and `runtime.login.cancel`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Flow {
    id: String,
    flow_id: String,
}

/// Map a runtime error: `data.reason` is always the adapter's stable code, and
/// the message is the adapter's fixed text (never credential or file content).
fn error(e: RuntimeError) -> RpcError {
    RpcError::new(
        match e.code.as_str() {
            "vault_locked" => ErrorCode::VaultLocked,
            "invalid_config" | "invalid_request" | "profile_invalid" => ErrorCode::InvalidArgument,
            "not_found" => ErrorCode::NotFound,
            "conflict" | "runtime_busy" | "auth_pending" | "account_changed" => ErrorCode::Conflict,
            "cancelled" => ErrorCode::Cancelled,
            // `auth_unsupported`, `auth_required`, `offline_refused`,
            // `login_failed`, `logout_failed`, `auth_probe_failed`,
            // `process_shutdown_failed`, `resume_unavailable`, `state_limit`,
            // `protocol_parameters`, `state_unavailable`, ...
            _ => ErrorCode::Unavailable,
        },
        e.message,
    )
    .with_data(json!({"reason":e.code}))
}

/// A refused engine swap that no runtime error explains: `vault_locked` while
/// the vault is not unlocked (the engine could not read the runs it checks, as
/// after an idle lock that left a run active), otherwise `conflict` with
/// `data.reason = "runtime_busy"` (an external run or approval is active). The
/// engine's own text is not returned: it can be a store error.
fn swap_refused(services: &Services) -> RpcError {
    if services.vault().status().state != VaultState::Unlocked {
        return error(RuntimeError {
            code: "vault_locked".into(),
            message: "Unlock the vault before changing external runtimes".into(),
        });
    }
    error(RuntimeError {
        code: "runtime_busy".into(),
        message: "Cancel active external runs and approvals before changing external runtimes"
            .into(),
    })
}

/// The refusal of a turn or account command of a runtime whose managed account
/// is being changed (an import or a sign-out while the official CLI runs).
fn account_changing() -> RpcError {
    error(RuntimeError {
        code: "runtime_busy".into(),
        message: "This runtime's account is being changed; try again once that has finished".into(),
    })
}

/// The manager's current executors, without those of runtimes whose account
/// is being changed, and the configuration generation they reflect (read
/// first, so a change that lands meanwhile makes the next turn load again).
fn current_executors(
    services: &Services,
    manager: &RuntimeManager,
) -> Result<(u64, Vec<Arc<dyn RuntimeExecutor>>), RuntimeError> {
    let generation = manager.generation();
    let sync = services.runtime_sync();
    let executors = manager
        .executors()?
        .into_iter()
        .filter(|executor| !sync.changing(&executor.descriptor().id))
        .collect();
    Ok((generation, executors))
}

/// Run a runtime change and reload the engine's runtimes with the result, as
/// one step the engine refuses while an external run is active. Refusals are
/// the change's own runtime error, or [`swap_refused`].
fn change_runtimes<T>(
    services: &Services,
    change: impl FnOnce(&RuntimeManager) -> Result<T, RuntimeError>,
) -> Result<T, RpcError> {
    let manager = manager(services)?;
    let mut domain_error = None;
    let changed = services.native().mutate_runtimes(|| {
        let value = change(manager).map_err(|e| {
            domain_error = Some(error(e));
            "Runtime change refused".to_owned()
        })?;
        let (generation, executors) = current_executors(services, manager).map_err(|e| {
            domain_error = Some(error(e));
            "Runtime registry unavailable".to_owned()
        })?;
        Ok(((value, generation), executors))
    });
    match changed {
        Ok((value, generation)) => {
            services.runtime_sync().set_loaded(generation);
            Ok(value)
        }
        Err(_) => Err(domain_error.unwrap_or_else(|| swap_refused(services))),
    }
}

/// Change runtime `id`'s managed account with its official CLI (an import or
/// a sign-out, which can take about 25 s) without holding the engine's
/// operations for that long. The runtime, and every other runtime configured
/// with the same account profile, first leaves the engine, in a step the
/// engine refuses while an external run is active, as for any account change;
/// until the change has ended no turn of them starts (`runtime_busy`) and the
/// engine does not load them again. Afterwards the engine loads them with the
/// account they have now (best effort: a refused load leaves that to their
/// next turn, which loads them before it starts).
fn change_account<T>(
    services: &Services,
    id: &str,
    change: impl FnOnce(&RuntimeManager) -> Result<T, RuntimeError>,
) -> Result<T, RpcError> {
    let manager = manager(services)?;
    let profile = manager.get(id).map_err(error)?.profile;
    let mut ids = manager
        .list()
        .map_err(error)?
        .into_iter()
        .filter(|config| profile.is_some() && config.profile == profile)
        .map(|config| config.id)
        .collect::<BTreeSet<_>>();
    ids.insert(id.to_owned());
    let Some(changing) = services.runtime_sync().begin_change(ids) else {
        return Err(account_changing());
    };
    let result =
        change_runtimes(services, |_| Ok(())).and_then(|()| change(manager).map_err(error));
    drop(changing);
    let _ = sync_runtimes(services);
    result
}

/// The runtime manager, or `unavailable` when it failed to open at startup.
fn manager(services: &Services) -> Result<&Arc<RuntimeManager>, RpcError> {
    services.runtime().map_err(|reason| {
        RpcError::new(
            ErrorCode::Unavailable,
            format!("External runtimes are unavailable: {reason}"),
        )
        .with_data(json!({"reason":"runtime_unavailable"}))
    })
}

/// Whether a session runs on an external runtime rather than the native loop.
pub(crate) fn uses_external_runtime(config: &SessionConfig) -> bool {
    config
        .runtime_id
        .as_deref()
        .is_some_and(|id| id != "native")
}

/// Cancel the active runs of conversations on an external runtime (going
/// offline, an emergency stop). Called before the runtime gate closes as well:
/// a run settles as `cancelled` only when its own cancel came before the gate
/// ended its vendor process, which the run thread would otherwise report as a
/// failure. Best effort: a run that cannot be read or cancelled (a locked
/// vault) is also stopped by the gate.
pub(crate) fn cancel_external_runs(services: &Services) {
    let native = services.native();
    let Ok(runs) = native.store().run_summaries() else {
        return;
    };
    let mut external = std::collections::HashMap::new();
    for run in runs.into_iter().filter(|run| run.status.active()) {
        let on_runtime = *external.entry(run.session_id.clone()).or_insert_with(|| {
            native
                .store()
                .get(&run.session_id)
                .is_ok_and(|detail| uses_external_runtime(&detail.configuration))
        });
        if on_runtime {
            let _ = native.cancel(&run.id);
        }
    }
}

/// End the external runtime work a closed gate must stop at once (going
/// offline, an emergency stop): pending official logins are cancelled, every
/// vendor process and official CLI command still running is terminated (with
/// `reason`, waiting up to the registry's grace period), and every launch
/// parked for an approval is released, so its temporary home is gone when
/// this returns. The runtime registry's report, or `None` without runtimes.
pub(crate) fn stop_runtime_processes(
    services: &Services,
    reason: aidex_sandbox::StopReason,
) -> Option<aidex_sandbox::TerminationReport> {
    let runtime = services.runtime().ok()?;
    // Best effort: a login whose process cannot be confirmed ended reports
    // `process_shutdown_failed` through its own `runtime.login.status`.
    let _ = runtime.cancel_logins();
    let report = runtime.process_registry().terminate_all(reason);
    runtime.release_paused();
    Some(report)
}

/// Load every enabled configured runtime into the native engine.
///
/// A no-op (and not an error) while the vault is locked, because the runtime
/// configuration is sealed with a vault key; when no runtime configuration
/// was ever saved (reading one would create that key, so a plain unlock must
/// not); and when the runtime manager is unavailable. The engine refuses the
/// swap while an external run is active. A runtime whose account is being
/// changed is left out until that change has ended.
pub(crate) fn sync_runtimes(services: &Services) -> Result<(), String> {
    let Ok(manager) = services.runtime() else {
        return Ok(());
    };
    if services.vault().status().state != VaultState::Unlocked || !manager.has_saved_configuration()
    {
        return Ok(());
    }
    let generation = services.native().mutate_runtimes(|| {
        let (generation, executors) =
            current_executors(services, manager).map_err(|e| e.to_string())?;
        Ok((generation, executors))
    })?;
    services.runtime_sync().set_loaded(generation);
    Ok(())
}

/// Load runtimes before a session that needs `config`'s external runtime is
/// created, started or continued: when the engine lacks the runtime, and when
/// a runtime configuration changed since the engine last loaded them (a login
/// that connected while the load was refused, for one), so a turn does not
/// launch an executor made for the previous account. When an active external
/// run refuses the full load, just this runtime is loaded: added when the
/// engine lacks it, or its executor replaced when none of its own runs is
/// active (an executor a run of it still uses stays, and an outdated one
/// refuses to launch, `account_changed`). Best effort: the engine reports a
/// runtime it still lacks. Refused (`runtime_busy`) only while the runtime's
/// account is being changed.
pub(crate) fn ensure_session_runtime(
    services: &Services,
    config: &SessionConfig,
) -> Result<(), RpcError> {
    if !uses_external_runtime(config) {
        return Ok(());
    }
    let Some(id) = config.runtime_id.as_deref() else {
        return Ok(());
    };
    let Ok(manager) = services.runtime() else {
        return Ok(());
    };
    if services.runtime_sync().changing(id) {
        return Err(account_changing());
    }
    let current = || services.runtime_sync().loaded() == Some(manager.generation());
    if engine_has_runtime(services, id) && current() {
        return Ok(());
    }
    let _ = sync_runtimes(services);
    if !engine_has_runtime(services, id) || !current() {
        load_runtime(services, id);
    }
    Ok(())
}

fn engine_has_runtime(services: &Services, id: &str) -> bool {
    services
        .native()
        .runtime_descriptors()
        .iter()
        .any(|descriptor| descriptor.id == id)
}

/// Load just runtime `id`'s current executor when a full swap was refused
/// because an external run is active (for example the run an
/// `approval.respond` answers, or another runtime's). A missing runtime is
/// added, which leaves every executor a run holds in place; one the engine has
/// is replaced only while none of its own runs is active
/// (`NativeEngine::replace_runtime`), so a paused launch keeps its executor.
fn load_runtime(services: &Services, id: &str) {
    let Ok(manager) = services.runtime() else {
        return;
    };
    if services.vault().status().state != VaultState::Unlocked
        || !manager.has_saved_configuration()
        || services.runtime_sync().changing(id)
    {
        return;
    }
    let Some(executor) = manager.executors().ok().and_then(|executors| {
        executors
            .into_iter()
            .find(|executor| executor.descriptor().id == id)
    }) else {
        return;
    };
    let native = services.native();
    let _ = if engine_has_runtime(services, id) {
        native.replace_runtime(id, executor)
    } else {
        native.register_runtime(id, executor)
    };
}

fn provider(services: &Services, id: &str) -> Result<String, RpcError> {
    match manager(services)?.get(id).map_err(error)?.auth {
        RuntimeAuth::CodexOauth { provider_id } => Ok(provider_id),
        _ => Err(RpcError::new(
            ErrorCode::Unavailable,
            "This runtime uses an explicitly scoped vault API key. Browser login is unavailable for this adapter.",
        )
        .with_data(json!({"reason":"auth_unsupported"}))),
    }
}

/// What the official CLI last confirmed about each managed account (never
/// credential bytes), answered while the runtime gate keeps the CLI from
/// starting. An entry applies only while the runtime keeps the profile it was
/// confirmed for.
#[derive(Default)]
struct AccountStatuses(Mutex<HashMap<String, RuntimeAccountStatus>>);

impl AccountStatuses {
    fn entries(&self) -> std::sync::MutexGuard<'_, HashMap<String, RuntimeAccountStatus>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Keep a status the CLI just gave (a pending login is not kept: it ends
    /// with the gate).
    fn remember(&self, status: &RuntimeAccountStatus) {
        if status.status != "auth_pending" {
            self.entries().insert(status.id.clone(), status.clone());
        }
    }

    fn forget(&self, id: &str) {
        self.entries().remove(id);
    }

    /// A login of runtime `id` connected: its official CLI confirmed the
    /// credential just before it was bound (the login's `login status`), so
    /// that is the account's last confirmed status.
    fn connected(&self, config: &RuntimeConfig, profile: &str) {
        if config.profile.as_deref() != Some(profile) {
            self.forget(&config.id);
            return;
        }
        self.remember(&RuntimeAccountStatus {
            id: config.id.clone(),
            driver: config.driver,
            profile: config.profile.clone(),
            authenticated: true,
            credential_present: true,
            evidence: "native_credential_and_cli_exit_status".into(),
            status: "authenticated_cached".into(),
        });
    }

    /// The last confirmed status of `config`'s current profile.
    fn confirmed(&self, config: &RuntimeConfig) -> Option<RuntimeAccountStatus> {
        self.entries()
            .get(&config.id)
            .filter(|status| config.profile.is_some() && status.profile == config.profile)
            .cloned()
    }

    /// `runtime.account.status` while the CLI may not start: a runtime without
    /// a profile has no credential (as the adapter reports it, without running
    /// anything); one with a profile gives what the CLI last confirmed for it,
    /// or `offline_refused` when it never did in this daemon.
    fn paused(&self, config: &RuntimeConfig) -> Result<RuntimeAccountStatus, RpcError> {
        if config.profile.is_none() {
            return Ok(RuntimeAccountStatus {
                id: config.id.clone(),
                driver: config.driver,
                profile: None,
                authenticated: false,
                credential_present: false,
                evidence: "native_credential_missing".into(),
                status: "auth_required".into(),
            });
        }
        self.confirmed(config).ok_or_else(cli_paused)
    }
}

/// The refusal of an account command that needs the official CLI while the
/// runtime gate is closed.
fn cli_paused() -> RpcError {
    error(RuntimeError {
        code: "offline_refused".into(),
        message: "The official CLI does not run while offline, while the vault is locked, or \
                  during an emergency stop"
            .into(),
    })
}

/// Only Codex has a verified official login, as the adapter checks it.
fn account_supported(config: &RuntimeConfig) -> Result<(), RpcError> {
    if config.driver == Driver::Codex {
        return Ok(());
    }
    Err(error(RuntimeError {
        code: "auth_unsupported".into(),
        message: "Official portable login commands have only been verified for Codex".into(),
    }))
}

/// `Some(config)` when runtime `id`'s official CLI may not start now (the
/// runtime gate is closed), `None` when it may. The runtime must exist with a
/// supported driver either way, so `not_found`, `vault_locked` and
/// `auth_unsupported` come first, as when the gate is open.
fn account_paused(
    services: &Services,
    manager: &RuntimeManager,
    id: &str,
) -> Result<Option<RuntimeConfig>, RpcError> {
    if services.runtime_gate_open() {
        return Ok(None);
    }
    let config = manager.get(id).map_err(error)?;
    account_supported(&config)?;
    Ok(Some(config))
}

/// Whether the adapter's health check of `config` asks the official CLI for
/// the managed account's status.
fn managed_account(config: &RuntimeConfig) -> bool {
    config.driver == Driver::Codex
        && config.profile.is_some()
        && matches!(config.auth, RuntimeAuth::None)
}

/// Whether the runtime's executable exists: an absolute path, or a path
/// relative to the drive root without traversal (as the adapter resolves it).
fn executable_installed(root: &Path, executable: &str) -> bool {
    let path = Path::new(executable);
    if path.is_absolute() {
        return path.is_file();
    }
    !executable.contains(':')
        && path.components().all(|c| matches!(c, Component::Normal(_)))
        && root.join(path).is_file()
}

/// `runtime.health` of a managed account while the runtime gate is closed:
/// what the adapter reports then (the model probe is refused, so the protocol
/// is not ready), with the account's last confirmed status instead of a new
/// `login status` from the official CLI, in health's documented codes
/// (`RuntimeAccountStatus::health_status`).
fn paused_health(
    services: &Services,
    config: &RuntimeConfig,
    statuses: &AccountStatuses,
) -> RuntimeHealth {
    let installed = executable_installed(services.paths().root(), &config.executable);
    let (authenticated, status) = match statuses.confirmed(config) {
        _ if !installed => (false, "binary_missing".to_owned()),
        Some(account) => (account.authenticated, account.health_status().to_owned()),
        None => (false, "installed_auth_unverified".to_owned()),
    };
    RuntimeHealth {
        id: config.id.clone(),
        installed,
        protocol_ready: false,
        authenticated,
        status,
        limitations: aidex_adapters::catalog()
            .into_iter()
            .find(|info| info.driver == config.driver)
            .map(|info| info.limitations)
            .unwrap_or_default(),
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let statuses = Arc::new(AccountStatuses::default());
    // The driver catalog is static: it is served even without the manager.
    router.register(RUNTIME_CATALOG, move |_, p| {
        expect_no_params(&p)?;
        to_result(&aidex_adapters::catalog())
    })?;
    let s = services.clone();
    router.register_passive(RUNTIME_LIST, move |_, p| {
        expect_no_params(&p)?;
        let runtimes = manager(&s)?.list().map_err(error)?;
        // Best effort: a refused swap (an external run is active) keeps the
        // runtimes the engine already has.
        let _ = sync_runtimes(&s);
        to_result(&json!({"runtimes":runtimes}))
    })?;
    for method in [RUNTIME_ADD, RUNTIME_UPDATE, RUNTIME_CONFIG_SAVE] {
        let s = services.clone();
        router.register(method, move |_, p| {
            let config: RuntimeConfig = parse_params(p)?;
            let manager = manager(&s)?;
            if method == RUNTIME_ADD
                && manager
                    .list()
                    .map_err(error)?
                    .iter()
                    .any(|c| c.id == config.id)
            {
                return Err(RpcError::new(
                    ErrorCode::Conflict,
                    "Runtime ID already exists",
                ));
            }
            if method == RUNTIME_UPDATE {
                manager.get(&config.id).map_err(error)?;
            }
            if s.runtime_sync().changing(&config.id) {
                return Err(account_changing());
            }
            to_result(&change_runtimes(&s, |manager| manager.save(config))?)
        })?;
    }
    let s = services.clone();
    let accounts = Arc::clone(&statuses);
    router.register(RUNTIME_REMOVE, move |_, p| {
        let p: Id = parse_params(p)?;
        manager(&s)?;
        if s.runtime_sync().changing(&p.id) {
            return Err(account_changing());
        }
        let removed = change_runtimes(&s, |manager| manager.remove(&p.id))?;
        // The removed runtime's account went with it.
        accounts.forget(&p.id);
        Ok(json!({"removed":removed}))
    })?;
    let s = services.clone();
    router.register(RUNTIME_CONFIG_GET, move |_, p| {
        let p: Id = parse_params(p)?;
        to_result(&manager(&s)?.get(&p.id).map_err(error)?)
    })?;
    let s = services.clone();
    let accounts = Arc::clone(&statuses);
    router.register_passive(RUNTIME_HEALTH, move |_, p| {
        let p: Id = parse_params(p)?;
        let manager = manager(&s)?;
        // Otherwise the adapter starts nothing: its model probe is refused
        // while the gate is closed.
        if !s.runtime_gate_open() {
            let config = manager.get(&p.id).map_err(error)?;
            if managed_account(&config) {
                return to_result(&paused_health(&s, &config, &accounts));
            }
        }
        let health = manager.health(&p.id).map_err(error)?;
        // When the health check got a managed account's status from the
        // official CLI (it reports one exactly then: `authenticated`, or
        // `auth_required`), keep it as the account's last confirmed status, as
        // `runtime.account.status` does. The adapter answers this from the
        // status it has just cached, so the CLI does not run again.
        if (health.authenticated || health.status == "auth_required")
            && s.runtime_gate_open()
            && let Ok(config) = manager.get(&p.id)
            && managed_account(&config)
            && let Ok(status) = manager.account_status(&p.id)
        {
            accounts.remember(&status);
        }
        to_result(&health)
    })?;
    let s = services.clone();
    router.register(RUNTIME_MODELS, move |_, p| {
        let p: Id = parse_params(p)?;
        Ok(json!({"models":manager(&s)?.models(&p.id).map_err(error)?}))
    })?;
    let s = services.clone();
    router.register(RUNTIME_INSTALL, move |_, p| {
        let p: Install = parse_params(p)?;
        to_result(
            &manager(&s)?
                .install(p.driver, p.source.as_deref())
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register(RUNTIME_AUTH_START, move |_, p| {
        let p: Id = parse_params(p)?;
        let provider = provider(&s, &p.id)?;
        to_result(
            &s.models()
                .auth()
                .start(&provider, |_, _| Ok(()))
                .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?,
        )
    })?;
    let s = services.clone();
    router.register(RUNTIME_AUTH_STATUS, move |_, p| {
        let p: Auth = parse_params(p)?;
        let provider = provider(&s, &p.id)?;
        let status = s
            .models()
            .auth()
            .status(p.flow_id.as_deref(), Some(&provider))
            .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?;
        if status.provider_id != provider {
            return Err(RpcError::invalid_params(
                "Flow does not belong to runtime account",
            ));
        }
        to_result(&status)
    })?;
    let s = services.clone();
    router.register(RUNTIME_AUTH_CANCEL, move |_, p| {
        let p: Auth = parse_params(p)?;
        let provider = provider(&s, &p.id)?;
        let flow = p
            .flow_id
            .ok_or_else(|| RpcError::invalid_params("flow_id required"))?;
        let status = s
            .models()
            .auth()
            .status(Some(&flow), None)
            .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?;
        if status.provider_id != provider {
            return Err(RpcError::invalid_params(
                "Flow does not belong to runtime account",
            ));
        }
        to_result(
            &s.models()
                .auth()
                .cancel(&flow)
                .map_err(|e| RpcError::new(ErrorCode::Unavailable, e.to_string()))?,
        )
    })?;
    register_accounts(router, services, &statuses)
}

/// `runtime.account.status`, `runtime.profile.import` and `runtime.login.*`:
/// the runtime's own account, signed in by its official CLI (Codex only; other
/// drivers are `unavailable` with `data.reason = "auth_unsupported"`).
fn register_accounts(
    router: &mut Router,
    services: &Arc<Services>,
    statuses: &Arc<AccountStatuses>,
) -> Result<(), RouterError> {
    let s = services.clone();
    let accounts = Arc::clone(statuses);
    router.register_passive(RUNTIME_ACCOUNT_STATUS, move |_, p| {
        let p: Id = parse_params(p)?;
        let manager = manager(&s)?;
        if let Some(config) = account_paused(&s, manager, &p.id)? {
            return to_result(&accounts.paused(&config)?);
        }
        let status = manager.account_status(&p.id).map_err(error)?;
        accounts.remember(&status);
        to_result(&status)
    })?;
    let s = services.clone();
    let accounts = Arc::clone(statuses);
    router.register(RUNTIME_PROFILE_IMPORT, move |_, p| {
        let p: Import = parse_params(p)?;
        if account_paused(&s, manager(&s)?, &p.id)?.is_some() {
            return Err(cli_paused());
        }
        let status = change_account(&s, &p.id, |manager| {
            manager.import_profile(&p.id, &p.source)
        })?;
        accounts.remember(&status);
        to_result(&status)
    })?;
    let s = services.clone();
    router.register(RUNTIME_LOGIN_START, move |_, p| {
        let p: Id = parse_params(p)?;
        // The login's official CLI may not start now: refused before the
        // engine is asked, as for an import or a sign-out.
        if account_paused(&s, manager(&s)?, &p.id)?.is_some() {
            return Err(cli_paused());
        }
        if s.runtime_sync().changing(&p.id) {
            return Err(account_changing());
        }
        to_result(&change_runtimes(&s, |manager| manager.login_start(&p.id))?)
    })?;
    let s = services.clone();
    let accounts = Arc::clone(statuses);
    router.register_passive(RUNTIME_LOGIN_STATUS, move |_, p| {
        let p: Flow = parse_params(p)?;
        let manager = manager(&s)?;
        let status = manager.login_status(&p.id, &p.flow_id).map_err(error)?;
        // A connected login switched the runtime to its profile once its
        // official CLI confirmed the credential. Best effort: while an
        // external run is active the engine keeps its executors, and the next
        // turn of this runtime loads the new one before it starts; the poll
        // itself never fails for it.
        if status.state == "connected" {
            match manager.get(&p.id) {
                Ok(config) => accounts.connected(&config, &status.profile),
                Err(_) => accounts.forget(&p.id),
            }
            let _ = sync_runtimes(&s);
        }
        to_result(&status)
    })?;
    let s = services.clone();
    router.register(RUNTIME_LOGIN_CANCEL, move |_, p| {
        let p: Flow = parse_params(p)?;
        let manager = manager(&s)?;
        // The runtime must exist (and the vault be unlocked), as for `status`.
        manager.get(&p.id).map_err(error)?;
        to_result(&manager.login_cancel(&p.id, &p.flow_id).map_err(error)?)
    })?;
    let s = services.clone();
    let accounts = Arc::clone(statuses);
    router.register(RUNTIME_LOGIN_LOGOUT, move |_, p| {
        let p: Id = parse_params(p)?;
        // Signing out runs the official CLI's `logout` before the encrypted
        // profile is removed, so it waits until the CLI may run again.
        if account_paused(&s, manager(&s)?, &p.id)?.is_some() {
            return Err(cli_paused());
        }
        let status = change_account(&s, &p.id, |manager| manager.logout(&p.id))?;
        accounts.remember(&status);
        to_result(&status)
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "runtime_wiring_tests.rs"]
mod wiring_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use aidex_vault::SecretString;
    use serde_json::Value;

    const PASSPHRASE: &str = "fixture passphrase only";

    fn fixture() -> (tempfile::TempDir, Arc<Services>, Router, CallContext) {
        let drive = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        let context = CallContext::detached(ClientInfo::new("runtime-test", "1"), shutdown);
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        (drive, services, router, context)
    }

    fn engine_runtimes(services: &Services) -> Vec<String> {
        services
            .native()
            .runtime_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect()
    }

    #[test]
    fn runtime_rpc_catalog_is_finite_and_config_requires_vault() {
        let (_drive, services, router, context) = fixture();
        assert!(services.runtime().is_ok());
        let catalog = router.call(&context, "runtime.catalog", json!({})).unwrap();
        assert_eq!(catalog.as_array().unwrap().len(), 10);
        assert!(router.call(&context, "runtime.list", json!({})).is_err());
        assert!(
            router
                .call(&context, "runtime.install", json!({"driver":"codex"}))
                .is_err()
        );
        assert!(
            router
                .call(
                    &context,
                    "runtime.add",
                    json!({"id":"a","driver":"codex","executable":"codex.exe","token":"forbidden"})
                )
                .is_err()
        );
    }

    #[test]
    fn configured_runtimes_reach_the_engine_and_report_a_missing_binary() {
        let (drive, services, mut router, context) = fixture();
        crate::methods::vault::register(&mut router, &services).unwrap();
        services
            .vault()
            .init(&SecretString::new(PASSPHRASE.into()), false)
            .unwrap();
        // Offline: probing the runtime's models must not launch anything.
        services.models().offline().set_offline(true).unwrap();
        let listed = router.call(&context, RUNTIME_LIST, Value::Null).unwrap();
        assert_eq!(listed, json!({"runtimes":[]}));

        let executable = drive.path().join("missing").join("codex.exe");
        let config = json!({
            "id":"codex-fixture",
            "driver":"codex",
            "executable":executable.to_string_lossy(),
        });
        let added = router.call(&context, RUNTIME_ADD, config.clone()).unwrap();
        assert_eq!(added["id"], "codex-fixture");
        assert_eq!(added["enabled"], true);
        assert_eq!(added["auth"], json!({"kind":"none"}));
        assert_eq!(
            router
                .call(&context, RUNTIME_ADD, config.clone())
                .unwrap_err()
                .kind(),
            Some(ErrorCode::Conflict)
        );
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        let listed = router.call(&context, RUNTIME_LIST, Value::Null).unwrap();
        assert_eq!(listed["runtimes"][0]["id"], "codex-fixture");

        let health = router
            .call(&context, RUNTIME_HEALTH, json!({"id":"codex-fixture"}))
            .unwrap();
        assert_eq!(health["status"], "binary_missing");
        assert_eq!(health["installed"], false);
        assert_eq!(health["protocol_ready"], false);
        let models = router
            .call(&context, RUNTIME_MODELS, json!({"id":"codex-fixture"}))
            .unwrap_err();
        assert_eq!(models.kind(), Some(ErrorCode::Unavailable));
        assert_eq!(models.data.unwrap()["reason"], "offline_refused");
        assert_eq!(
            router
                .call(&context, RUNTIME_HEALTH, json!({"id":"absent"}))
                .unwrap_err()
                .kind(),
            Some(ErrorCode::NotFound)
        );

        // A daemon restart leaves the engine empty until the vault unlocks.
        services.native().synchronize_runtimes(Vec::new()).unwrap();
        services.vault().lock();
        assert_eq!(sync_runtimes(&services), Ok(()));
        assert!(engine_runtimes(&services).is_empty());
        router
            .call(&context, "vault.unlock", json!({"passphrase":PASSPHRASE}))
            .unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        // `runtime.list` also loads them lazily.
        services.native().synchronize_runtimes(Vec::new()).unwrap();
        router.call(&context, RUNTIME_LIST, Value::Null).unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);

        let mut disabled = config.clone();
        disabled["enabled"] = json!(false);
        router.call(&context, RUNTIME_UPDATE, disabled).unwrap();
        assert!(engine_runtimes(&services).is_empty());
        assert_eq!(
            router
                .call(&context, RUNTIME_REMOVE, json!({"id":"codex-fixture"}))
                .unwrap(),
            json!({"removed":true})
        );
        assert_eq!(
            router.call(&context, RUNTIME_LIST, Value::Null).unwrap(),
            json!({"runtimes":[]})
        );
    }

    #[test]
    fn the_reserved_native_id_is_refused_before_anything_is_saved() {
        let (drive, services, router, context) = fixture();
        services
            .vault()
            .init(&SecretString::new(PASSPHRASE.into()), false)
            .unwrap();
        services.models().offline().set_offline(true).unwrap();
        let executable = drive.path().join("missing").join("codex.exe");
        let config = |id: &str| {
            json!({
                "id":id,
                "driver":"codex",
                "executable":executable.to_string_lossy(),
            })
        };
        for method in [RUNTIME_ADD, RUNTIME_CONFIG_SAVE] {
            let refused = router.call(&context, method, config("native")).unwrap_err();
            assert_eq!(refused.kind(), Some(ErrorCode::InvalidArgument));
            assert_eq!(refused.data.unwrap()["reason"], "invalid_config");
        }
        assert!(services.runtime().unwrap().list().unwrap().is_empty());
        assert_eq!(sync_runtimes(&services), Ok(()));

        // Runtime changes and syncs keep working afterwards.
        router
            .call(&context, RUNTIME_ADD, config("codex-fixture"))
            .unwrap();
        services.native().synchronize_runtimes(Vec::new()).unwrap();
        assert_eq!(sync_runtimes(&services), Ok(()));
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
    }

    #[test]
    fn unlocking_a_vault_without_runtimes_adds_no_secret() {
        let (_drive, services, mut router, context) = fixture();
        crate::methods::vault::register(&mut router, &services).unwrap();
        router
            .call(&context, "vault.init", json!({"passphrase":PASSPHRASE}))
            .unwrap();
        router.call(&context, "vault.lock", Value::Null).unwrap();
        router
            .call(&context, "vault.unlock", json!({"passphrase":PASSPHRASE}))
            .unwrap();
        assert_eq!(
            router
                .call(&context, "vault.secret.list", Value::Null)
                .unwrap(),
            json!({"secrets":[],"managed":[]})
        );
        assert!(engine_runtimes(&services).is_empty());
    }

    #[test]
    fn external_runtime_sessions_load_their_runtime_before_starting() {
        let (drive, services, mut router, context) = fixture();
        crate::methods::native::register(&mut router, &services).unwrap();
        services
            .vault()
            .init(&SecretString::new(PASSPHRASE.into()), false)
            .unwrap();
        services.models().offline().set_offline(true).unwrap();
        let workspace_dir = tempfile::tempdir().unwrap();
        let workspace = services
            .workspaces()
            .add_host(workspace_dir.path(), None)
            .unwrap();
        let executable = drive.path().join("missing").join("codex.exe");
        router
            .call(
                &context,
                RUNTIME_ADD,
                json!({"id":"codex-fixture","driver":"codex","executable":executable.to_string_lossy()}),
            )
            .unwrap();
        let session = json!({
            "runtime_id":"codex-fixture",
            "workspace_id":workspace.id,
            "provider_id":aidex_native_sessions::RUNTIME_PROVIDER_ID,
            "model_id":aidex_native_sessions::RUNTIME_DEFAULT_MODEL,
            "mode":"agent",
            "permission_mode":"full_access",
        });

        services.native().synchronize_runtimes(Vec::new()).unwrap();
        let created = router
            .call(&context, "conversation.create", session)
            .unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        let id = created["id"].as_str().unwrap().to_owned();

        services.native().synchronize_runtimes(Vec::new()).unwrap();
        let run = router
            .call(
                &context,
                "message.send",
                json!({"session_id":id,"content":"hello","command_id":"send-1"}),
            )
            .unwrap();
        assert_eq!(engine_runtimes(&services), ["codex-fixture"]);
        // The run itself fails (offline, no binary); wait so nothing outlives the drive.
        let run_id = run["id"].as_str().unwrap().to_owned();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            let status = services.native().store().run(&run_id).unwrap().status;
            if !status.active() {
                break;
            }
            assert!(std::time::Instant::now() < until, "run stayed {status:?}");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}
