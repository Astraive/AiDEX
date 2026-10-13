//! Official CLI account commands in app-owned homes; only encrypted profiles persist.
use crate::{
    Driver, RuntimeConfig,
    manager::Configs,
    runtime::NetworkGate,
    snapshot,
    state::{State, error},
};
use aidex_native_sessions::{RuntimeError, digest};
use aidex_sandbox::{
    CommandSpec, ManagedProcess, NetworkMode, ResourceLimits, Sandbox, SandboxLevel, SandboxRequest,
};
use serde::Serialize;
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
/// A browser login run by the official CLI in an app-owned temporary home.
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
    output: Zeroizing<Vec<u8>>,
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
/// Output kept from a login process to find its authorization URL.
const MAX_LOGIN_OUTPUT: usize = 64 * 1024;
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
    fn forget(&self, id: &str) {
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
        let profile = temp_home()?;
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
        let temp = temp_home()?;
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
        let _account_guard = self.state.account_guard()?;
        write_profile_record(&self.state, profile, &bytes)?;
        self.state.rotate_account_revision(profile, &config.id)?;
        self.forget(&config.id);
        status.profile = Some(profile.to_owned());
        Ok(status)
    }
    /// Start the official CLI's browser login in an app-owned temporary home.
    /// The saved runtime configuration changes only once the login connected.
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
        let profile = temp_home()?;
        let process = Arc::new(command(
            &self.sandbox,
            &config,
            profile.path(),
            &["login"],
            Duration::from_secs(300),
        )?);
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
            message: "Complete the official Codex login in your browser".into(),
            error_code: None,
        };
        let job = Arc::new(Mutex::new(Login {
            status: status.clone(),
            process,
            profile: Some(profile),
            output: Zeroizing::new(Vec::new()),
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
            loop {
                let Ok(mut login) = job.lock() else {
                    break;
                };
                if login.status.state != "pending" {
                    break;
                }
                let output = login.process.take_output();
                for bytes in [&output.stdout.bytes, &output.stderr.bytes] {
                    let room = MAX_LOGIN_OUTPUT.saturating_sub(login.output.len());
                    login
                        .output
                        .extend_from_slice(&bytes[..room.min(bytes.len())]);
                }
                drop(Zeroizing::new(output.stdout.bytes));
                drop(Zeroizing::new(output.stderr.bytes));
                if login.status.authorization_url.is_none() {
                    login.status.authorization_url = authorization_url(&login.output);
                }
                // The gate also closes when the vault locks or an emergency stop begins.
                if !network() {
                    finish_cancelled(&mut login);
                    break;
                }
                if login.process.is_running() {
                    drop(login);
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }
                let success = login
                    .process
                    .try_wait()
                    .ok()
                    .flatten()
                    .is_some_and(|s| s.success());
                // The worker owns the temporary home from here, so a cancel
                // cannot remove it while the official CLI confirms it.
                let scratch = login.profile.take();
                login.status.authorization_url = None;
                login.output.clear();
                drop(login);
                // Confirmed without the job lock: status polls and cancellation
                // never wait for the official CLI's `login status`.
                let confirmed = if success {
                    scratch
                        .as_ref()
                        .ok_or_else(|| {
                            error("state_unavailable", "Login temporary profile unavailable")
                        })
                        .and_then(|profile| confirm(&sandbox, &config, profile.path()))
                } else {
                    Err(error(
                        "login_failed",
                        "Official CLI login did not complete; account or workspace eligibility may require action in the official browser flow",
                    ))
                };
                let Ok(mut login) = job.lock() else {
                    break;
                };
                // Cancelled meanwhile: nothing is saved, and the cancel already
                // ended the flow.
                if login.status.state != "pending" {
                    break;
                }
                if !network() {
                    finish_cancelled(&mut login);
                    break;
                }
                match confirmed.and_then(|bytes| connect(&state, &configs, &config, &bytes)) {
                    Ok(()) => {
                        login.status.state = "connected".into();
                        login.status.authenticated = true;
                        login.status.message =
                            "Official CLI credentials encrypted on the drive".into();
                    }
                    Err(e) => {
                        login.status.state = "failed".into();
                        login.status.error_code = Some(e.code);
                        login.status.message = e.message;
                    }
                }
                let _ = state.set_login_pending(&config.id, false);
                break;
            }
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
    /// old credential.
    pub fn logout(&self, config: &RuntimeConfig) -> Result<RuntimeAccountStatus, RuntimeError> {
        let temp = temp_home()?;
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
/// A connected login: encrypt the confirmed credential as the account profile,
/// start a new account revision, and only then point the runtime at that
/// profile. Any failure leaves the saved configuration, and so the runtime's
/// previous authentication, unchanged.
fn connect(
    state: &State,
    configs: &Configs,
    config: &RuntimeConfig,
    bytes: &[u8],
) -> Result<(), RuntimeError> {
    let profile = profile_id(config);
    let _account_guard = state.account_guard()?;
    write_profile_record(state, &profile, bytes)?;
    state.rotate_account_revision(&profile, &config.id)?;
    configs.use_profile(&config.id, &profile)
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
    login.output.clear();
    login.profile.take();
}
fn temp_home() -> Result<tempfile::TempDir, RuntimeError> {
    let temp = tempfile::Builder::new()
        .prefix("aidex-account-")
        .tempdir()
        .map_err(|_| error("state_unavailable", "Account temporary profile unavailable"))?;
    std::fs::create_dir(temp.path().join("home"))
        .map_err(|_| error("state_unavailable", "Account temporary home unavailable"))?;
    Ok(temp)
}
fn command(
    sandbox: &Sandbox,
    config: &RuntimeConfig,
    scratch: &Path,
    args: &[&str],
    timeout: Duration,
) -> Result<ManagedProcess, RuntimeError> {
    let home = scratch.join("home");
    let mut request = SandboxRequest::new(SandboxLevel::Brokered, scratch, scratch)
        .with_network(NetworkMode::Normal)
        .with_limits(ResourceLimits {
            wall_timeout: Some(timeout),
            max_output_bytes: MAX_LOGIN_OUTPUT,
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
    sandbox
        .spawn(
            &plan,
            CommandSpec::new(PathBuf::from(&config.executable)).args(argv),
        )
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
fn authorization_url(bytes: &[u8]) -> Option<String> {
    // OAuth authorization URLs are intended for the user. No callback/code/token URL
    // or arbitrary vendor stderr can become frontend text.
    String::from_utf8_lossy(bytes)
        .split_whitespace()
        .find(|v| {
            v.starts_with("https://auth.openai.com/oauth/authorize?")
                && v.len() <= 8192
                && !v.contains("access_token=")
                && !v.contains("refresh_token=")
                && !v.contains("code=")
                && !v.contains('#')
        })
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn authorization_output_only_accepts_official_browser_authorize_urls() {
        assert_eq!(
            authorization_url(b"https://evil.test/?access_token=opaque"),
            None
        );
        assert_eq!(
            authorization_url(b"https://auth.openai.com/oauth/authorize?code=opaque"),
            None
        );
        assert!(authorization_url(b"Open https://auth.openai.com/oauth/authorize?state=fixture&code_challenge=fixture").is_some());
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
        let temp = temp_home().unwrap();
        let process = command(
            &Sandbox::locate(),
            &config,
            temp.path(),
            &["login", "status"],
            Duration::from_secs(15),
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
