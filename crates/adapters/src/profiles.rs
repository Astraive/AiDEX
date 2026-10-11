//! Official CLI account commands in app-owned homes; only encrypted profiles persist.
use crate::{
    Driver, RuntimeConfig,
    runtime::NetworkGate,
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
    time::Duration,
};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Serialize)]
pub struct RuntimeAccountStatus {
    pub id: String,
    pub driver: Driver,
    pub profile: Option<String>,
    pub authenticated: bool,
    pub credential_present: bool,
    pub evidence: String,
    pub status: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct RuntimeLoginStatus {
    pub id: String,
    pub flow_id: String,
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
pub(crate) struct Accounts {
    state: Arc<State>,
    sandbox: Sandbox,
    network: NetworkGate,
    jobs: Mutex<BTreeMap<String, Arc<Mutex<Login>>>>,
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
    pub fn new(state: Arc<State>, sandbox: Sandbox, network: NetworkGate) -> Self {
        Self {
            state,
            sandbox,
            network,
            jobs: Mutex::new(BTreeMap::new()),
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
        let profile = temp_home()?;
        if let Some(id) = &config.profile {
            self.state
                .restore(&profile_record(id), &profile.path().join("home"))?;
        }
        account_status(&self.sandbox, config, profile.path())
    }
    pub fn import(
        &self,
        config: &RuntimeConfig,
        source: &Path,
    ) -> Result<RuntimeAccountStatus, RuntimeError> {
        self.supported(config)?;
        if !source.is_absolute() {
            return Err(error(
                "invalid_request",
                "Select an explicit absolute auth.json file or profile directory",
            ));
        }
        let source = if source.is_dir() {
            source.join("auth.json")
        } else {
            source.to_owned()
        };
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|_| error("profile_invalid", "Selected credential file is unavailable"))?;
        if !metadata.is_file()
            || crate::state::unsafe_link(&metadata)
            || metadata.len() > 1024 * 1024
        {
            return Err(error(
                "profile_invalid",
                "Select a regular bounded auth.json credential file",
            ));
        }
        let bytes =
            Zeroizing::new(std::fs::read(&source).map_err(|_| {
                error("profile_invalid", "Selected credential file cannot be read")
            })?);
        if !credential_json(&bytes) {
            return Err(error(
                "profile_invalid",
                "Selected file has no recognized native Codex credential evidence",
            ));
        }
        let temp = temp_home()?;
        let home = temp.path().join("home");
        std::fs::write(home.join("auth.json"), &bytes)
            .map_err(|_| error("state_unavailable", "Temporary credential cannot be copied"))?;
        let status = account_status(&self.sandbox, config, temp.path())?;
        if !status.authenticated {
            return Err(error(
                "auth_required",
                "Official CLI did not confirm the selected native account",
            ));
        }
        let _account_guard = self.state.account_guard()?;
        self.state
            .snapshot(&profile_record(&profile_id(config)), &home)?;
        self.state
            .rotate_account_revision(&profile_id(config), &config.id)?;
        Ok(status)
    }
    pub fn start(&self, config: RuntimeConfig) -> Result<RuntimeLoginStatus, RuntimeError> {
        self.supported(&config)?;
        if !(self.network)() {
            return Err(error(
                "offline_refused",
                "Enable network access before starting official CLI login",
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
                    let room = (64 * 1024usize).saturating_sub(login.output.len());
                    login
                        .output
                        .extend_from_slice(&bytes[..room.min(bytes.len())]);
                }
                if login.status.authorization_url.is_none() {
                    login.status.authorization_url = authorization_url(&login.output);
                }
                if !network() {
                    finish_cancelled(&mut login);
                    break;
                }
                if !login.process.is_running() {
                    let success = login
                        .process
                        .try_wait()
                        .ok()
                        .flatten()
                        .is_some_and(|s| s.success());
                    let result = if success {
                        login
                            .profile
                            .as_ref()
                            .ok_or_else(|| {
                                error("state_unavailable", "Login temporary profile unavailable")
                            })
                            .and_then(|profile| {
                                let status = account_status(&sandbox, &config, profile.path())?;
                                if !status.authenticated {
                                    return Err(error(
                                        "auth_required",
                                        "Official CLI exited without native credential evidence",
                                    ));
                                }
                                let _account_guard = state.account_guard()?;
                                state.snapshot(
                                    &profile_record(&profile_id(&config)),
                                    &profile.path().join("home"),
                                )?;
                                state.rotate_account_revision(&profile_id(&config), &config.id)
                            })
                    } else {
                        Err(error(
                            "login_failed",
                            "Official CLI login did not complete; account or workspace eligibility may require action in the official browser flow",
                        ))
                    };
                    match result {
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
                    login.output.clear();
                    login.profile.take();
                    let _ = state.set_login_pending(&config.id, false);
                    break;
                }
                drop(login);
                std::thread::sleep(Duration::from_millis(25));
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
        if cancel && job.profile.is_some() {
            finish_cancelled(&mut job);
            if job.status.error_code.as_deref() == Some("process_shutdown_failed") {
                return Err(error(
                    "process_shutdown_failed",
                    "Official login tree termination could not be confirmed",
                ));
            }
        }
        Ok(job.status.clone())
    }
    pub fn cancel_runtime(&self, id: &str) -> Result<(), RuntimeError> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| error("state_unavailable", "Login registry unavailable"))?;
        for job in jobs.values() {
            let mut job = job
                .lock()
                .map_err(|_| error("state_unavailable", "Login job unavailable"))?;
            if job.status.id == id && job.profile.is_some() {
                finish_cancelled(&mut job);
                if job.status.error_code.as_deref() == Some("process_shutdown_failed") {
                    return Err(error(
                        "process_shutdown_failed",
                        "Official login tree termination could not be confirmed",
                    ));
                }
            }
        }
        Ok(())
    }
    pub fn cancel_all(&self) -> Result<(), RuntimeError> {
        let jobs = self
            .jobs
            .lock()
            .map_err(|_| error("state_unavailable", "Login registry unavailable"))?;
        for job in jobs.values() {
            let mut job = job
                .lock()
                .map_err(|_| error("state_unavailable", "Login job unavailable"))?;
            if job.profile.is_some() {
                finish_cancelled(&mut job);
                if job.status.error_code.as_deref() == Some("process_shutdown_failed") {
                    return Err(error(
                        "process_shutdown_failed",
                        "Official login tree termination could not be confirmed",
                    ));
                }
            }
        }
        Ok(())
    }
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
        let _account_guard = self.state.account_guard()?;
        if let Some(id) = &config.profile {
            self.state.remove(&profile_record(id))?;
        }
        self.state.remove_runtime_snapshots(&config.id)?;
        drop(_account_guard);
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
                    && job.profile.is_some()
                {
                    finish_cancelled(&mut job);
                }
            }
        }
    }
}
fn finish_cancelled(login: &mut Login) {
    let killed = login.process.kill_tree().is_ok();
    let exited = login
        .process
        .wait_timeout(Duration::from_secs(5))
        .is_ok_and(|exit| exit.is_some());
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
    if let Some(state) = login.state.upgrade() {
        let _ = state.set_login_pending(&login.status.id, false);
    }
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
            max_output_bytes: 64 * 1024,
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
    let path = scratch.join("home/auth.json");
    let credential_present = std::fs::metadata(&path)
        .is_ok_and(|m| m.is_file() && m.len() <= 1024 * 1024)
        && std::fs::read(&path).is_ok_and(|b| credential_json(&Zeroizing::new(b)));
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
/// Exact credential redaction is scoped to the app-owned restored native profile.
/// These values are never DTOs, diagnostics or global-host credential imports.
pub(crate) fn credential_secrets(home: &Path) -> Result<Vec<Zeroizing<String>>, RuntimeError> {
    let path = home.join("auth.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => {
            return Err(error(
                "auth_required",
                "Managed native credential cannot be read",
            ));
        }
    };
    if !metadata.is_file() || crate::state::unsafe_link(&metadata) || metadata.len() > 1024 * 1024 {
        return Err(error(
            "profile_invalid",
            "Managed native credential format is invalid",
        ));
    }
    let bytes = Zeroizing::new(
        std::fs::read(path)
            .map_err(|_| error("auth_required", "Managed native credential cannot be read"))?,
    );
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
    let path = home.join("auth.json");
    if !path.exists() {
        return Err(error(
            "auth_required",
            "Official CLI did not retain portable credential evidence",
        ));
    }
    let _secrets = credential_secrets(home)?;
    let temp = temp_home()?;
    std::fs::copy(path, temp.path().join("home/auth.json")).map_err(|_| {
        error(
            "state_unavailable",
            "Refreshed native credentials cannot be encrypted",
        )
    })?;
    state.snapshot(&profile_record(profile_id), &temp.path().join("home"))
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
