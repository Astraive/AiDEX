//! Official CLI account commands in app-owned homes; only encrypted profiles persist.
use crate::{
    Driver, RuntimeConfig,
    manager::Configs,
    runtime::NetworkGate,
    snapshot,
    state::{State, error},
    transport::Transport,
};
use aidex_native_sessions::{RuntimeError, digest};
use aidex_sandbox::{
    CommandSpec, ManagedProcess, NetworkMode, ResourceLimits, Sandbox, SandboxLevel, SandboxRequest,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

/// A managed runtime account. Never carries credential bytes.
#[derive(Clone, Debug, Serialize)]
pub struct RuntimeAccountStatus {
    pub id: String,
    pub driver: Driver,
    pub profile: Option<String>,
    /// The official CLI's `login status` succeeded on the encrypted credential:
    /// cached-credential evidence, not proof of inference entitlement.
    pub authenticated: bool,
    pub credential_present: bool,
    /// A stable evidence code, for example `native_credential_and_cli_exit_status`.
    pub evidence: String,
    /// `authenticated_cached`, `auth_required`, `auth_pending` or `signed_out`.
    pub status: String,
}
impl RuntimeAccountStatus {
    /// This status in `runtime.health`'s documented codes: a credential the
    /// official CLI confirmed is `installed_auth_unverified` (with
    /// `authenticated`; the model probe may still make it `ready`), anything
    /// else (`auth_required`, `auth_pending`, `signed_out`) is
    /// `auth_required`.
    pub fn health_status(&self) -> &'static str {
        if self.authenticated {
            "installed_auth_unverified"
        } else {
            "auth_required"
        }
    }
}
/// A browser login run by the official CLI in an app-owned temporary home.
/// The CLI never opens a browser: the client opens `authorization_url`.
#[derive(Clone, Debug, Serialize)]
pub struct RuntimeLoginStatus {
    pub id: String,
    pub flow_id: String,
    /// `pending`, `connected`, `cancelled` or `failed`.
    pub state: String,
    pub profile: String,
    pub authenticated: bool,
    pub authorization_url: Option<String>,
    pub message: String,
    pub error_code: Option<String>,
}
struct Login {
    status: RuntimeLoginStatus,
    process: Arc<ManagedProcess>,
    profile: Option<tempfile::TempDir>,
    state: std::sync::Weak<State>,
}
impl Drop for Login {
    fn drop(&mut self) {
        let _ = self.process.kill_tree();
        let _ = self.process.wait_timeout(Duration::from_secs(5));
    }
}
/// A `login status` result reused for health checks; the key covers the CLI,
/// the profile and its account revision.
struct CachedStatus {
    key: String,
    at: Instant,
    status: RuntimeAccountStatus,
}
const STATUS_TTL: Duration = Duration::from_secs(60);
/// The largest credential file an account profile holds.
const MAX_CREDENTIAL: u64 = 1024 * 1024;
/// Output kept from an account command; nothing reads it.
const MAX_ACCOUNT_OUTPUT: usize = 64 * 1024;
/// How long an official login may take, its process included.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
/// JSON-RPC ids of the login's requests to `codex app-server`.
const LOGIN_INITIALIZE: &str = "aidex-login-initialize";
const LOGIN_START: &str = "aidex-login-start";
pub(crate) struct Accounts {
    state: Arc<State>,
    configs: Arc<Configs>,
    sandbox: Sandbox,
    network: NetworkGate,
    jobs: Mutex<BTreeMap<String, Arc<Mutex<Login>>>>,
    statuses: Mutex<BTreeMap<String, CachedStatus>>,
}
pub(crate) fn profile_id(config: &RuntimeConfig) -> String {
    config
        .profile
        .clone()
        .unwrap_or_else(|| format!("account-{}", &digest(config.id.as_bytes())[..24]))
}
pub(crate) fn profile_record(id: &str) -> String {
    format!("profile-{}.sealed", digest(id.as_bytes()))
}
impl Accounts {
    pub fn new(
        state: Arc<State>,
        configs: Arc<Configs>,
        sandbox: Sandbox,
        network: NetworkGate,
    ) -> Self {
        Self {
            state,
            configs,
            sandbox,
            network,
            jobs: Mutex::new(BTreeMap::new()),
            statuses: Mutex::new(BTreeMap::new()),
        }
    }
    pub fn supported(&self, config: &RuntimeConfig) -> Result<(), RuntimeError> {
        if config.driver != Driver::Codex {
            return Err(error(
                "auth_unsupported",
                "Official portable login commands have only been verified for Codex",
            ));
        }
        Ok(())
    }
    /// Drop the cached account status of runtime `id`.
    pub(crate) fn forget(&self, id: &str) {
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.remove(id);
        }
    }
    pub fn status(&self, config: &RuntimeConfig) -> Result<RuntimeAccountStatus, RuntimeError> {
        self.supported(config)?;
        if self.state.login_pending(&config.id)? {
            return Ok(RuntimeAccountStatus {
                id: config.id.clone(),
                driver: config.driver,
                profile: config.profile.clone(),
                authenticated: false,
                credential_present: false,
                evidence: "official_cli_login_pending".into(),
                status: "auth_pending".into(),
            });
        }
        let revision = config
            .profile
            .as_ref()
            .map(|profile| self.state.account_revision(profile))
            .transpose()?
            .flatten();
        let key = digest(
            serde_json::json!({"executable":config.executable,"argv":config.argv,"profile":config.profile,"revision":revision})
                .to_string()
                .as_bytes(),
        );
        if let Ok(statuses) = self.statuses.lock()
            && let Some(cached) = statuses.get(&config.id)
            && cached.key == key
            && cached.at.elapsed() < STATUS_TTL
        {
            return Ok(cached.status.clone());
        }
        let profile = temp_home(&self.state)?;
        if let Some(id) = &config.profile {
            self.state
                .restore(&profile_record(id), &profile.path().join("home"))?;
        }
        let status = account_status(&self.sandbox, config, profile.path())?;
        if let Ok(mut statuses) = self.statuses.lock() {
            statuses.insert(
                config.id.clone(),
                CachedStatus {
                    key,
                    at: Instant::now(),
                    status: status.clone(),
                },
            );
        }
        Ok(status)
    }
    /// Validate an explicitly selected `auth.json` with the official CLI and
    /// encrypt it as the runtime's account profile. The caller switches the
    /// runtime to that profile afterwards.
    pub fn import(
        &self,
        config: &RuntimeConfig,
        profile: &str,
        source: &Path,
    ) -> Result<RuntimeAccountStatus, RuntimeError> {
        self.supported(config)?;
        if !source.is_absolute() {
            return Err(error(
                "invalid_request",
                "Select an explicit absolute auth.json file or profile directory",
            ));
        }
        // The selection itself is never followed through a link or junction.
        let selected = std::fs::symlink_metadata(source)
            .map_err(|_| error("profile_invalid", "Selected credential file is unavailable"))?;
        if crate::state::unsafe_link(&selected) {
            return Err(error(
                "profile_invalid",
                "Select a regular bounded auth.json credential file",
            ));
        }
        let source = if selected.is_dir() {
            source.join("auth.json")
        } else {
            source.to_owned()
        };
        let bytes = read_credential(&source).map_err(|e| {
            if e.code == "auth_required" {
                error("profile_invalid", "Selected credential file is unavailable")
            } else {
                e
            }
        })?;
        let temp = temp_home(&self.state)?;
        let home = temp.path().join("home");
        std::fs::write(home.join("auth.json"), bytes.as_slice())
            .map_err(|_| error("state_unavailable", "Temporary credential cannot be copied"))?;
        let mut status = account_status(&self.sandbox, config, temp.path())?;
        if !status.authenticated {
            return Err(error(
                "auth_required",
                "Official CLI did not confirm the selected native account",
            ));
        }
        connect(&self.state, &self.configs, config, profile, &bytes)?;
        self.forget(&config.id);
        status.profile = Some(profile.to_owned());
        Ok(status)
    }
    /// Start the official ChatGPT login through the configured CLI's
    /// `app-server` in an app-owned temporary home. The CLI opens no browser
    /// (on Windows a browser it started would join the login's kill-on-close
    /// process tree and close with it): the flow's `authorization_url` is the
    /// page the client opens. The saved runtime configuration changes only
    /// once the login connected.
    pub fn start(&self, config: RuntimeConfig) -> Result<RuntimeLoginStatus, RuntimeError> {
        self.supported(&config)?;
        if !(self.network)() {
            return Err(error(
                "offline_refused",
                "Official CLI login needs network access, an unlocked vault and no emergency stop",
            ));
        }
        let mut jobs = self
            .jobs
            .lock()
            .map_err(|_| error("state_unavailable", "Login registry unavailable"))?;
        if jobs.values().any(|job| {
            job.lock()
                .is_ok_and(|j| j.status.id == config.id && j.status.state == "pending")
        }) {
            return Err(error(
                "runtime_busy",
                "An official login is already pending for this runtime",
            ));
        }
        jobs.retain(|_, job| job.lock().is_ok_and(|j| j.status.state == "pending"));
        if jobs.len() >= 8 {
            return Err(error(
                "runtime_busy",
                "Too many official logins are pending",
            ));
        }
        let profile = temp_home(&self.state)?;
        let transport = Transport::new(
            command(
                &self.sandbox,
                &config,
                profile.path(),
                &["app-server"],
                LOGIN_TIMEOUT,
                true,
            )?,
            vec![],
        );
        let mut random = [0u8; 24];
        getrandom::fill(&mut random)
            .map_err(|_| error("state_unavailable", "Login flow ID unavailable"))?;
        let status = RuntimeLoginStatus {
            id: config.id.clone(),
            flow_id: digest(&random),
            state: "pending".into(),
            profile: profile_id(&config),
            authenticated: false,
            authorization_url: None,
            message: "Open the official Codex authorization page to sign in".into(),
            error_code: None,
        };
        let job = Arc::new(Mutex::new(Login {
            status: status.clone(),
            process: transport.process.clone(),
            profile: Some(profile),
            state: Arc::downgrade(&self.state),
        }));
        self.state.set_login_pending(&config.id, true)?;
        jobs.insert(status.flow_id.clone(), job.clone());
        let state = self.state.clone();
        let configs = self.configs.clone();
        let sandbox = self.sandbox.clone();
        let network = self.network.clone();
        // The worker owns cleanup even when the frontend closes or never polls status.
        std::thread::spawn(move || {
            login_worker(
                &job, transport, &state, &configs, &sandbox, &network, &config,
            );
        });
        Ok(status)
    }
    pub fn flow(
        &self,
        id: &str,
        flow_id: &str,
        cancel: bool,
    ) -> Result<RuntimeLoginStatus, RuntimeError> {
        let job = self
            .jobs
            .lock()
            .map_err(|_| error("state_unavailable", "Login registry unavailable"))?
            .get(flow_id)
            .cloned()
            .ok_or_else(|| error("not_found", "Official login flow not found"))?;
        let mut job = job
            .lock()
            .map_err(|_| error("state_unavailable", "Login job unavailable"))?;
        if job.status.id != id {
            return Err(error(
                "invalid_request",
                "Login flow does not belong to this runtime",
            ));
        }
        if cancel && cancellable(&job) {
            finish_cancelled(&mut job);
            confirmed(&job)?;
        }
        Ok(job.status.clone())
    }
    /// Cancel the pending logins of one runtime (`None`: of every runtime).
    /// The manager stays usable afterwards.
    pub fn cancel(&self, runtime: Option<&str>) -> Result<(), RuntimeError> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| error("state_unavailable", "Login registry unavailable"))?;
        let mut result = Ok(());
        for job in jobs.values() {
            let mut job = job
                .lock()
                .map_err(|_| error("state_unavailable", "Login job unavailable"))?;
            if cancellable(&job) && runtime.is_none_or(|id| job.status.id == id) {
                finish_cancelled(&mut job);
                // Keep cancelling the others before reporting.
                if result.is_ok() {
                    result = confirmed(&job);
                }
            }
        }
        result
    }
    /// Sign out with the official CLI, then remove the encrypted account profile
    /// and every session snapshot of the runtime, so no session restores the
    /// old credential, and every snapshot an older daemon saved under the
    /// legacy name (any of which can hold the account's `auth.json`).
    pub fn logout(&self, config: &RuntimeConfig) -> Result<RuntimeAccountStatus, RuntimeError> {
        let temp = temp_home(&self.state)?;
        if let Some(id) = &config.profile {
            self.state
                .restore(&profile_record(id), &temp.path().join("home"))?;
        }
        let process = command(
            &self.sandbox,
            config,
            temp.path(),
            &["logout"],
            Duration::from_secs(15),
            false,
        )?;
        let exit = process
            .wait_timeout(Duration::from_secs(20))
            .map_err(|_| error("logout_failed", "Official CLI logout status unavailable"))?;
        if exit.is_none() {
            let _ = process.kill_tree();
            let _ = process.wait_timeout(Duration::from_secs(5));
        }
        let native_confirmed =
            exit.is_some_and(|s| s.success()) && !temp.path().join("home/auth.json").exists();
        let account_guard = self.state.account_guard()?;
        if let Some(id) = &config.profile {
            self.state.remove(&profile_record(id))?;
        }
        self.state.remove_runtime_snapshots(&config.id)?;
        self.state.remove_legacy_snapshots()?;
        drop(account_guard);
        self.forget(&config.id);
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| error("state_unavailable", "Login registry unavailable"))?;
        for job in jobs.values() {
            let mut job = job
                .lock()
                .map_err(|_| error("state_unavailable", "Login job unavailable"))?;
            if job.status.id == config.id {
                job.status.authenticated = false;
                job.status.state = "cancelled".into();
                job.status.authorization_url = None;
                job.status.message = "Managed native account signed out".into();
            }
        }
        Ok(RuntimeAccountStatus {
            id: config.id.clone(),
            driver: config.driver,
            profile: config.profile.clone(),
            authenticated: false,
            credential_present: false,
            evidence: if native_confirmed {
                "native_cli_logout_and_profile_removed"
            } else {
                "managed_profile_removed_cli_unconfirmed"
            }
            .into(),
            status: "signed_out".into(),
        })
    }
}
impl Drop for Accounts {
    fn drop(&mut self) {
        if let Ok(jobs) = self.jobs.lock() {
            for job in jobs.values() {
                if let Ok(mut job) = job.lock()
                    && cancellable(&job)
                {
                    finish_cancelled(&mut job);
                }
            }
        }
    }
}
/// The credential a finished login left, once the official CLI's `login
/// status` confirms it.
fn confirm(
    sandbox: &Sandbox,
    config: &RuntimeConfig,
    scratch: &Path,
) -> Result<Zeroizing<Vec<u8>>, RuntimeError> {
    let status = account_status(sandbox, config, scratch)?;
    if !status.authenticated {
        return Err(error(
            "auth_required",
            "Official CLI exited without native credential evidence",
        ));
    }
    read_credential(&scratch.join("home/auth.json"))
}
/// A confirmed login or import: encrypt the credential as the account
/// profile, start a new account revision, and only then point the runtime at
/// that profile ([`store_account`]). A runtime removed meanwhile gets nothing.
/// A failure leaves the saved configuration unchanged and, best effort, the
/// previous credential; the runtime's session snapshots are gone either way.
fn connect(
    state: &State,
    configs: &Configs,
    config: &RuntimeConfig,
    profile: &str,
    bytes: &[u8],
) -> Result<(), RuntimeError> {
    let _account_guard = state.account_guard()?;
    configs.bind_profile(&config.id, profile, || {
        store_account(state, profile, &config.id, bytes)
    })
}
/// Encrypt `bytes` as `profile`'s credential and start a new account
/// revision, under the account guard. The runtime's session snapshots (and
/// every legacy-named one) are removed first, so nothing saved under the
/// previous account can be resumed under the new one whichever step fails
/// later. A failed rotation puts the previous credential record back (or
/// removes the new one), best effort, so the runtime keeps the account it
/// had instead of using the new credential with the previous revision.
fn store_account(
    state: &State,
    profile: &str,
    runtime: &str,
    bytes: &[u8],
) -> Result<(), RuntimeError> {
    state.remove_runtime_snapshots(runtime)?;
    state.remove_legacy_snapshots()?;
    let record = profile_record(profile);
    let previous = state.read(&record)?;
    write_profile_record(state, profile, bytes)?;
    if let Err(e) = state.rotate_account_revision(profile, runtime) {
        let _ = match &previous {
            Some(plain) => state.write(&record, plain),
            None => state.remove(&record),
        };
        return Err(e);
    }
    Ok(())
}
/// What one frame of a login's `app-server` exchange meant.
enum LoginStep {
    Pending,
    /// The official authorization page to open.
    Authorize(String),
    Completed,
}
/// Where a login's `app-server` exchange is.
#[derive(Default)]
struct LoginWire {
    /// `account/login/start` answered.
    started: bool,
    login_id: Option<String>,
}
fn login_failed() -> RuntimeError {
    error(
        "login_failed",
        "Official CLI login did not complete; account or workspace eligibility may require action in the official browser flow",
    )
}
/// The CLI could not start its app-server login (an older CLI, an error).
fn login_unsupported() -> RuntimeError {
    error(
        "login_failed",
        "The configured Codex CLI could not start the official app-server login; it may need an update",
    )
}
/// Answer one frame of the login's `codex app-server` exchange: initialize,
/// `account/login/start` (`type: chatgpt`, which returns the authorization
/// page and opens no browser), then the `account/login/completed`
/// notification of that login. Vendor text never reaches the flow.
fn login_step(
    frame: &Value,
    transport: &Transport,
    wire: &mut LoginWire,
) -> Result<LoginStep, RuntimeError> {
    let id = frame.get("id").and_then(Value::as_str);
    let method = frame.get("method").and_then(Value::as_str);
    let answer = || {
        frame
            .get("result")
            .filter(|_| frame.get("error").is_none())
            .ok_or_else(login_unsupported)
    };
    match (id, method) {
        (Some(LOGIN_INITIALIZE), None) => {
            answer()?;
            transport
                .send(json!({"jsonrpc":"2.0","method":"initialized"}))
                .and_then(|()| {
                    transport.send(json!({"jsonrpc":"2.0","id":LOGIN_START,"method":"account/login/start","params":{"type":"chatgpt"}}))
                })
                .map_err(|_| login_failed())?;
            Ok(LoginStep::Pending)
        }
        (Some(LOGIN_START), None) => {
            let result = answer()?;
            if result["type"] != "chatgpt" {
                return Err(login_unsupported());
            }
            let url = result["authUrl"]
                .as_str()
                .and_then(authorization_url)
                .ok_or_else(|| {
                    error(
                        "login_failed",
                        "Official CLI did not return the official authorization page",
                    )
                })?;
            wire.started = true;
            wire.login_id = result["loginId"].as_str().map(str::to_owned);
            Ok(LoginStep::Authorize(url))
        }
        (None, Some("account/login/completed")) if wire.started => {
            let params = &frame["params"];
            // Another login's completion is not this one's.
            if let (Some(ours), Some(theirs)) =
                (wire.login_id.as_deref(), params["loginId"].as_str())
                && ours != theirs
            {
                return Ok(LoginStep::Pending);
            }
            if params["success"] == true {
                Ok(LoginStep::Completed)
            } else {
                Err(login_failed())
            }
        }
        _ => Ok(LoginStep::Pending),
    }
}
/// Drive the login's `codex app-server` until the login completes (`Ok`) or
/// fails (`Err`); `None` when the flow ended otherwise (cancelled, or the
/// gate closed, which cancels it here).
fn await_login(
    job: &Mutex<Login>,
    transport: &mut Transport,
    network: &NetworkGate,
) -> Option<Result<(), RuntimeError>> {
    let initialize = json!({"jsonrpc":"2.0","id":LOGIN_INITIALIZE,"method":"initialize","params":{"clientInfo":{"name":"AiDEX","title":"AiDEX","version":"0.1.0"},"capabilities":{}}});
    if transport.send(initialize).is_err() {
        return Some(Err(login_failed()));
    }
    let mut wire = LoginWire::default();
    loop {
        let mut login = job.lock().ok()?;
        if login.status.state != "pending" {
            return None;
        }
        // The gate also closes when the vault locks or an emergency stop begins.
        if !network() {
            finish_cancelled(&mut login);
            return None;
        }
        let frame = match transport.poll() {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                drop(login);
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            // Ended, timed out or out of bounds before it completed.
            Err(_) => return Some(Err(login_failed())),
        };
        match login_step(&frame, transport, &mut wire) {
            Ok(LoginStep::Pending) => {}
            Ok(LoginStep::Authorize(url)) => login.status.authorization_url = Some(url),
            Ok(LoginStep::Completed) => return Some(Ok(())),
            Err(e) => return Some(Err(e)),
        }
    }
}
/// One official login, from the app-server exchange to the connected
/// account. It owns the flow's cleanup even when nobody polls its status.
fn login_worker(
    job: &Mutex<Login>,
    mut transport: Transport,
    state: &State,
    configs: &Configs,
    sandbox: &Sandbox,
    network: &NetworkGate,
    config: &RuntimeConfig,
) {
    let Some(completed) = await_login(job, &mut transport, network) else {
        return;
    };
    // The app-server's part is over: it may exit by itself, and is stopped
    // otherwise, so the official CLI's `login status` runs alone in the home.
    transport.process.close_stdin();
    let _ = transport.process.wait_timeout(Duration::from_secs(2));
    drop(transport);
    let Ok(mut login) = job.lock() else {
        return;
    };
    // Cancelled meanwhile: the cancel already ended the flow.
    if login.status.state != "pending" {
        return;
    }
    // The worker owns the temporary home from here, so a cancel cannot
    // remove it while the official CLI confirms it.
    let scratch = login.profile.take();
    login.status.authorization_url = None;
    drop(login);
    // Confirmed without the job lock: status polls and cancellation never
    // wait for the official CLI's `login status`.
    let confirmed = completed.and_then(|()| {
        scratch
            .as_ref()
            .ok_or_else(|| error("state_unavailable", "Login temporary profile unavailable"))
            .and_then(|profile| confirm(sandbox, config, profile.path()))
    });
    let Ok(mut login) = job.lock() else {
        return;
    };
    // Cancelled meanwhile: nothing is saved, and the cancel already ended
    // the flow.
    if login.status.state != "pending" {
        return;
    }
    if !network() {
        finish_cancelled(&mut login);
        return;
    }
    match confirmed.and_then(|bytes| connect(state, configs, config, &profile_id(config), &bytes)) {
        Ok(()) => {
            login.status.state = "connected".into();
            login.status.authenticated = true;
            login.status.message = "Official CLI credentials encrypted on the drive".into();
        }
        Err(e) => {
            login.status.state = "failed".into();
            login.status.error_code = Some(e.code);
            login.status.message = e.message;
        }
    }
    let _ = state.set_login_pending(&config.id, false);
}
/// A flow still to end: pending (its process running or its credential being
/// confirmed), or one whose termination could not be confirmed and is retried.
fn cancellable(login: &Login) -> bool {
    login.status.state == "pending" || login.profile.is_some()
}
fn confirmed(login: &Login) -> Result<(), RuntimeError> {
    if login.status.error_code.as_deref() == Some("process_shutdown_failed") {
        return Err(error(
            "process_shutdown_failed",
            "Official login tree termination could not be confirmed",
        ));
    }
    Ok(())
}
fn finish_cancelled(login: &mut Login) {
    // An ended tree needs no kill (its login may be under confirmation).
    let killed = !login.process.is_running() || login.process.kill_tree().is_ok();
    let exited = login
        .process
        .wait_timeout(Duration::from_secs(5))
        .is_ok_and(|exit| exit.is_some());
    // The flow ends either way: nothing connects it any more, and a pending
    // flag left behind would refuse every turn of the runtime. Only the flow
    // that set the flag clears it, never a retry after a newer login started.
    if login.status.state == "pending"
        && let Some(state) = login.state.upgrade()
    {
        let _ = state.set_login_pending(&login.status.id, false);
    }
    if !killed || !exited {
        login.status.state = "failed".into();
        login.status.error_code = Some("process_shutdown_failed".into());
        login.status.message = "Official login tree termination could not be confirmed".into();
        return;
    }
    login.status.state = "cancelled".into();
    login.status.error_code = None;
    login.status.message = "Official CLI login cancelled; temporary credentials removed".into();
    login.status.authorization_url = None;
    login.profile.take();
}
/// An account command's scratch folder, under the daemon's locked temporary
/// root so a crash mid-login leaves its credential to a later sweep.
fn temp_home(state: &State) -> Result<tempfile::TempDir, RuntimeError> {
    let temp = state
        .temp_home("aidex-account-")
        .map_err(|_| error("state_unavailable", "Account temporary profile unavailable"))?;
    with_home(temp)
}
fn with_home(temp: tempfile::TempDir) -> Result<tempfile::TempDir, RuntimeError> {
    std::fs::create_dir(temp.path().join("home"))
        .map_err(|_| error("state_unavailable", "Account temporary home unavailable"))?;
    Ok(temp)
}
/// An official CLI account command in `scratch`'s home; `interactive` keeps
/// its stdin for a protocol (`app-server`).
fn command(
    sandbox: &Sandbox,
    config: &RuntimeConfig,
    scratch: &Path,
    args: &[&str],
    timeout: Duration,
    interactive: bool,
) -> Result<ManagedProcess, RuntimeError> {
    let home = scratch.join("home");
    let mut request = SandboxRequest::new(SandboxLevel::Brokered, scratch, scratch)
        .with_network(NetworkMode::Normal)
        .with_limits(ResourceLimits {
            wall_timeout: Some(timeout),
            max_output_bytes: if interactive {
                crate::transport::MAX_FRAME
            } else {
                MAX_ACCOUNT_OUTPUT
            },
            ..Default::default()
        });
    request
        .env
        .push(("CODEX_HOME".into(), home.to_string_lossy().into_owned()));
    let plan = sandbox.negotiate(&request).map_err(|_| {
        error(
            "scope_unsupported",
            "Managed login process controls unavailable",
        )
    })?;
    let mut argv = config.argv.clone();
    argv.extend(args.iter().map(|s| s.to_string()));
    // A file credential in CODEX_HOME, never the host keyring.
    argv.extend(["-c".into(), "cli_auth_credentials_store=\"file\"".into()]);
    let command = CommandSpec::new(PathBuf::from(&config.executable)).args(argv);
    if interactive {
        sandbox.spawn_interactive(&plan, command)
    } else {
        sandbox.spawn(&plan, command)
    }
    .map_err(|_| {
        error(
            "launch_failed",
            "Configured official CLI account command could not be launched",
        )
    })
}
fn account_status(
    sandbox: &Sandbox,
    config: &RuntimeConfig,
    scratch: &Path,
) -> Result<RuntimeAccountStatus, RuntimeError> {
    let credential_present = read_credential(&scratch.join("home/auth.json")).is_ok();
    let (authenticated, evidence) = if credential_present {
        let process = command(
            sandbox,
            config,
            scratch,
            &["login", "status"],
            Duration::from_secs(15),
            false,
        )?;
        let result = process.wait_timeout(Duration::from_secs(20)).map_err(|_| {
            error(
                "auth_probe_failed",
                "Official CLI account status unavailable",
            )
        })?;
        if result.is_none() {
            let _ = process.kill_tree();
            let _ = process.wait_timeout(Duration::from_secs(5));
        }
        (
            result.is_some_and(|s| s.success()),
            "native_credential_and_cli_exit_status",
        )
    } else {
        (false, "native_credential_missing")
    };
    Ok(RuntimeAccountStatus {
        id: config.id.clone(),
        driver: config.driver,
        profile: config.profile.clone(),
        authenticated,
        credential_present,
        evidence: evidence.into(),
        status: if authenticated {
            "authenticated_cached"
        } else {
            "auth_required"
        }
        .into(),
    })
}
fn credential_json(bytes: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    v["OPENAI_API_KEY"].as_str().is_some_and(|v| !v.is_empty())
        || v.pointer("/tokens/access_token")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|v| !v.is_empty())
}
/// A regular, non-link, bounded native credential file with credential evidence.
fn read_credential(path: &Path) -> Result<Zeroizing<Vec<u8>>, RuntimeError> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| error("auth_required", "Native credential is unavailable"))?;
    if !metadata.is_file()
        || crate::state::unsafe_link(&metadata)
        || metadata.len() > MAX_CREDENTIAL
    {
        return Err(error(
            "profile_invalid",
            "Select a regular bounded auth.json credential file",
        ));
    }
    let bytes = Zeroizing::new(
        std::fs::read(path)
            .map_err(|_| error("auth_required", "Native credential cannot be read"))?,
    );
    if !credential_json(&bytes) {
        return Err(error(
            "profile_invalid",
            "Credential file has no recognized native Codex credential evidence",
        ));
    }
    Ok(bytes)
}
/// Encrypt the credential as the account profile record, in the legacy archive
/// encoding so a daemon from before `base64_v1` can still restore it.
fn write_profile_record(state: &State, profile: &str, bytes: &[u8]) -> Result<(), RuntimeError> {
    let files = BTreeMap::from([("auth.json".to_owned(), Zeroizing::new(bytes.to_vec()))]);
    state.write(&profile_record(profile), &snapshot::encode_legacy(&files)?)
}
/// Digest of the restored native credential, to notice a vendor refresh.
pub(crate) fn credential_digest(home: &Path) -> Result<String, RuntimeError> {
    read_credential(&home.join("auth.json")).map(|bytes| digest(&bytes))
}
/// Exact credential redaction is scoped to the app-owned restored native profile.
/// These values are never DTOs, diagnostics or global-host credential imports.
pub(crate) fn credential_secrets(home: &Path) -> Result<Vec<Zeroizing<String>>, RuntimeError> {
    let bytes = read_credential(&home.join("auth.json"))?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| {
        error(
            "profile_invalid",
            "Managed native credential format is invalid",
        )
    })?;
    Ok([
        "/OPENAI_API_KEY",
        "/tokens/access_token",
        "/tokens/refresh_token",
        "/tokens/id_token",
    ]
    .iter()
    .filter_map(|p| {
        v.pointer(p)
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| Zeroizing::new(s.to_owned()))
    })
    .collect())
}
/// Keep a credential the official CLI refreshed during a turn, unless a
/// different account was connected meanwhile.
pub(crate) fn persist_native_credentials(
    state: &State,
    profile_id: &str,
    home: &Path,
    expected_revision: &Option<String>,
) -> Result<(), RuntimeError> {
    let _account_guard = state.account_guard()?;
    if &state.account_revision(profile_id)? != expected_revision {
        return Err(error(
            "account_changed",
            "Managed account changed while this native session was running",
        ));
    }
    // A signed-out profile stays signed out.
    if state.read(&profile_record(profile_id))?.is_none() {
        return Err(error(
            "auth_required",
            "Managed account was signed out while this native session was running",
        ));
    }
    let bytes = read_credential(&home.join("auth.json"))?;
    write_profile_record(state, profile_id, &bytes)
}
/// The official authorization page the CLI asks the user to open. It is
/// meant for the user; no other URL (a callback, a code or token, arbitrary
/// vendor text) becomes frontend text.
fn authorization_url(url: &str) -> Option<String> {
    (url.starts_with("https://auth.openai.com/oauth/authorize?")
        && url.len() <= 8192
        && !url.contains("access_token=")
        && !url.contains("refresh_token=")
        && !url.contains("code=")
        && !url.contains('#')
        && !url.chars().any(|c| c.is_whitespace() || c.is_control()))
    .then(|| url.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn only_official_browser_authorize_urls_are_accepted() {
        for refused in [
            "https://evil.test/?access_token=opaque",
            "https://auth.openai.com/oauth/authorize?code=opaque",
            "https://auth.openai.com/oauth/authorize?state=a#fragment",
            "https://auth.openai.com/oauth/authorize?state=a b",
            "Open https://auth.openai.com/oauth/authorize?state=fixture",
            "http://auth.openai.com/oauth/authorize?state=fixture",
        ] {
            assert_eq!(authorization_url(refused), None, "{refused}");
        }
        let official = "https://auth.openai.com/oauth/authorize?response_type=code&client_id=fixture&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&code_challenge=fixture&code_challenge_method=S256&state=fixture";
        assert_eq!(authorization_url(official).as_deref(), Some(official));
    }
    #[test]
    fn health_reports_account_statuses_in_its_own_codes() {
        let status = |authenticated: bool, status: &str| RuntimeAccountStatus {
            id: "fixture".into(),
            driver: Driver::Codex,
            profile: Some("p".into()),
            authenticated,
            credential_present: authenticated,
            evidence: "fixture".into(),
            status: status.into(),
        };
        assert_eq!(
            status(true, "authenticated_cached").health_status(),
            "installed_auth_unverified"
        );
        for code in ["auth_required", "auth_pending", "signed_out"] {
            assert_eq!(status(false, code).health_status(), "auth_required");
        }
    }
    #[test]
    fn profile_records_stay_readable_by_the_legacy_decoder() {
        let root = tempfile::tempdir().unwrap();
        let vault = Arc::new(
            aidex_vault::VaultService::open(
                root.path().join("vault"),
                aidex_vault::VaultServiceConfig {
                    kdf: aidex_vault::KdfParams::new(8, 1, 1).unwrap(),
                    idle_timeout: None,
                },
            )
            .unwrap(),
        );
        vault
            .init(
                &aidex_vault::SecretString::from("profile-fixture-password".to_owned()),
                false,
            )
            .unwrap();
        let state = State::new(root.path().join("private"), vault).unwrap();
        let credential = br#"{"tokens":{"access_token":"legacy-readable-fixture"}}"#;
        write_profile_record(&state, "account-fixture", credential).unwrap();
        let plain = state
            .read(&profile_record("account-fixture"))
            .unwrap()
            .unwrap();
        // Exactly how a daemon from before `base64_v1` decodes any archive.
        let legacy: BTreeMap<String, Vec<u8>> = serde_json::from_slice(&plain).unwrap();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy["auth.json"], credential);
    }
    #[test]
    #[ignore = "requires explicitly selected installed Codex binary; isolated read-only native auth status, no login or inference"]
    fn installed_codex_empty_profile_reports_actual_signed_out_exit() {
        let executable =
            std::env::var("AIDEX_TEST_CODEX").expect("Explicit installed Codex path required");
        let config = RuntimeConfig {
            id: "read-only-probe".into(),
            driver: Driver::Codex,
            executable,
            argv: vec![],
            profile: None,
            enabled: true,
            auth: crate::RuntimeAuth::None,
        };
        let temp = with_home(tempfile::tempdir().unwrap()).unwrap();
        let process = command(
            &Sandbox::locate(),
            &config,
            temp.path(),
            &["login", "status"],
            Duration::from_secs(15),
            false,
        )
        .unwrap();
        let exit = process
            .wait_timeout(Duration::from_secs(20))
            .unwrap()
            .unwrap();
        assert!(!exit.success());
        assert!(!temp.path().join("home/auth.json").exists());
        println!(
            "Isolated official Codex auth-status exit={:?}; native credentials absent",
            exit.code
        );
    }
}
