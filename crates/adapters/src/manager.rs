use crate::{
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
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;
pub struct RuntimeManager {
    root: PathBuf,
    state: Arc<State>,
    auth: Arc<AuthService>,
    sandbox: Sandbox,
    network: NetworkGate,
    changes: Mutex<()>,
    executors: Mutex<BTreeMap<String, (String, Arc<ExternalRuntime>)>>,
    accounts: crate::profiles::Accounts,
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
        let private = drive_root.join("private");
        Self::open_with_private(drive_root, private, vault, auth, sandbox, network_gate)
    }
    /// Daemon composition passes the private directory declared by DrivePaths.
    pub fn open_with_private(
        drive_root: PathBuf,
        private_root: PathBuf,
        vault: Arc<VaultService>,
        auth: Arc<AuthService>,
        sandbox: Sandbox,
        network_gate: NetworkGate,
    ) -> Result<Self, RuntimeError> {
        let root = drive_root
            .canonicalize()
            .map_err(|_| error("state_unavailable", "Runtime drive unavailable"))?;
        migrate_legacy_records(&root.join("private"), &private_root)?;
        let state = Arc::new(State::new(private_root, vault)?);
        let accounts =
            crate::profiles::Accounts::new(state.clone(), sandbox.clone(), network_gate.clone());
        Ok(Self {
            root,
            state,
            auth,
            sandbox,
            network: network_gate,
            changes: Mutex::new(()),
            executors: Mutex::new(BTreeMap::new()),
            accounts,
        })
    }
    pub fn catalog(&self) -> Vec<DriverInfo> {
        catalog()
    }
    pub fn list(&self) -> Result<Vec<RuntimeConfig>, RuntimeError> {
        let Some(bytes) = self.state.read("runtime-config.sealed")? else {
            return Ok(vec![]);
        };
        serde_json::from_slice(&bytes)
            .map_err(|_| error("state_corrupt", "Encrypted runtime config is invalid"))
    }
    pub fn get(&self, id: &str) -> Result<RuntimeConfig, RuntimeError> {
        self.list()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| error("not_found", "Runtime configuration not found"))
    }
    pub fn save(&self, mut config: RuntimeConfig) -> Result<RuntimeConfig, RuntimeError> {
        let _guard = self.changes.lock().map_err(|_| {
            error(
                "state_unavailable",
                "Runtime configuration lock unavailable",
            )
        })?;
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
        let bytes = Zeroizing::new(
            serde_json::to_vec(&configs)
                .map_err(|_| error("invalid_config", "Runtime config cannot be encoded"))?,
        );
        if bytes.len() > 64 * 1024 {
            return Err(error(
                "config_limit",
                "Runtime configuration exceeds 64 KiB",
            ));
        }
        self.state.write("runtime-config.sealed", &bytes)?;
        self.executors
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime registry unavailable"))?
            .remove(&config.id);
        Ok(config)
    }
    pub fn remove(&self, id: &str) -> Result<bool, RuntimeError> {
        if !self.sandbox.registry().is_empty() {
            return Err(error(
                "runtime_busy",
                "Stop active external runtimes before removing configuration",
            ));
        }
        let _guard = self
            .changes
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime config lock unavailable"))?;
        let mut configs = self.list()?;
        let before = configs.len();
        configs.retain(|c| c.id != id);
        let changed = configs.len() != before;
        if changed {
            self.state.write(
                "runtime-config.sealed",
                &serde_json::to_vec(&configs).unwrap(),
            )?;
            self.executors
                .lock()
                .map_err(|_| error("state_unavailable", "Runtime registry unavailable"))?
                .remove(id);
        }
        Ok(changed)
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
        for mut config in configs.into_iter().filter(|c| c.enabled) {
            validate_config(&config)?;
            config.executable = self
                .executable(&config.executable)?
                .to_string_lossy()
                .into_owned();
            let signature =
                aidex_native_sessions::digest(serde_json::to_vec(&config).unwrap().as_slice());
            let entry = cache.entry(config.id.clone()).or_insert_with(|| {
                (
                    signature.clone(),
                    Arc::new(ExternalRuntime::new(
                        config.clone(),
                        self.state.clone(),
                        self.auth.clone(),
                        self.sandbox.clone(),
                        self.network.clone(),
                    )),
                )
            });
            if entry.0 != signature {
                *entry = (
                    signature,
                    Arc::new(ExternalRuntime::new(
                        config,
                        self.state.clone(),
                        self.auth.clone(),
                        self.sandbox.clone(),
                        self.network.clone(),
                    )),
                );
            }
            out.push(entry.1.clone());
        }
        Ok(out)
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
        if config.driver == Driver::Codex
            && config.profile.is_some()
            && matches!(config.auth, RuntimeAuth::None)
        {
            let native = self.account_status(id)?;
            authenticated = native.authenticated;
            status = native.status;
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
            && self.models(id).is_ok()
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
            bridge: None,
        };
        self.executor(id)?.discover_models(&r)
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
        crate::install::place(&self.root, driver, &relative, &component.sha256)
    }

    pub fn auth(&self) -> &Arc<AuthService> {
        &self.auth
    }
    pub fn cancel_logins(&self) -> Result<(), RuntimeError> {
        self.accounts.cancel_all()
    }
    pub fn shutdown(&self) -> Result<(), RuntimeError> {
        let cancelled = self.cancel_logins();
        let report = self
            .sandbox
            .registry()
            .shutdown(aidex_sandbox::StopReason::DaemonShutdown);
        if !report.is_clean() {
            return Err(error(
                "process_shutdown_failed",
                "A native runtime tree could not be confirmed terminated",
            ));
        }
        if cancelled.is_err() {
            self.cancel_logins()?;
        }
        self.executors
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime registry unavailable"))?
            .clear();
        Ok(())
    }
    pub fn account_status(&self, id: &str) -> Result<crate::RuntimeAccountStatus, RuntimeError> {
        let mut config = self.get(id)?;
        config.executable = self
            .executable(&config.executable)?
            .to_string_lossy()
            .into_owned();
        self.accounts.status(&config)
    }
    pub fn import_profile(
        &self,
        id: &str,
        source: &Path,
    ) -> Result<crate::RuntimeAccountStatus, RuntimeError> {
        let mut config = self.get(id)?;
        self.accounts.supported(&config)?;
        config.profile = Some(crate::profiles::profile_id(&config));
        config.auth = RuntimeAuth::None;
        if !self.sandbox.registry().is_empty() {
            return Err(error(
                "runtime_busy",
                "Stop active external runtimes before importing an account",
            ));
        }
        let mut resolved = config.clone();
        resolved.executable = self
            .executable(&config.executable)?
            .to_string_lossy()
            .into_owned();
        let status = self.accounts.import(&resolved, source)?;
        self.save(config)?;
        Ok(status)
    }
    pub fn login_start(&self, id: &str) -> Result<crate::RuntimeLoginStatus, RuntimeError> {
        let mut config = self.get(id)?;
        self.accounts.supported(&config)?;
        if !(self.network)() {
            return Err(error(
                "offline_refused",
                "Enable network access before starting official CLI login",
            ));
        }
        let original = config.clone();
        config.profile = Some(crate::profiles::profile_id(&config));
        config.auth = RuntimeAuth::None;
        let mut config = self.save(config)?;
        config.executable = self
            .executable(&config.executable)?
            .to_string_lossy()
            .into_owned();
        match self.accounts.start(config) {
            Ok(status) => Ok(status),
            Err(e) => {
                self.save(original)?;
                Err(e)
            }
        }
    }
    pub fn login_status(
        &self,
        id: &str,
        flow_id: &str,
    ) -> Result<crate::RuntimeLoginStatus, RuntimeError> {
        self.get(id)?;
        self.accounts.flow(id, flow_id, false)
    }
    pub fn login_cancel(
        &self,
        id: &str,
        flow_id: &str,
    ) -> Result<crate::RuntimeLoginStatus, RuntimeError> {
        self.accounts.flow(id, flow_id, true)
    }
    pub fn logout(&self, id: &str) -> Result<crate::RuntimeAccountStatus, RuntimeError> {
        let mut config = self.get(id)?;
        self.accounts.supported(&config)?;
        self.accounts.cancel_runtime(id)?;
        if !self.sandbox.registry().is_empty() {
            return Err(error(
                "runtime_busy",
                "Stop active runtimes before signing out",
            ));
        }
        config.executable = self
            .executable(&config.executable)?
            .to_string_lossy()
            .into_owned();
        self.accounts.logout(&config)
    }
}
fn migrate_legacy_records(legacy: &Path, destination: &Path) -> Result<(), RuntimeError> {
    if legacy == destination
        || !legacy.join("runtime-config.sealed").exists()
        || destination.join("runtime-config.sealed").exists()
    {
        return Ok(());
    }
    let legacy_meta = std::fs::symlink_metadata(legacy)
        .map_err(|_| error("state_unavailable", "Legacy runtime records unavailable"))?;
    if !legacy_meta.is_dir() || legacy_meta.file_type().is_symlink() {
        return Err(error(
            "state_corrupt",
            "Legacy runtime directory is invalid",
        ));
    }
    std::fs::create_dir_all(destination)
        .map_err(|_| error("state_unavailable", "Managed runtime directory unavailable"))?;
    if std::fs::symlink_metadata(destination).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(error(
            "state_corrupt",
            "Managed runtime directory is invalid",
        ));
    }
    let mut records = Vec::new();
    for entry in std::fs::read_dir(legacy)
        .map_err(|_| error("state_unavailable", "Legacy runtime records unavailable"))?
    {
        let entry =
            entry.map_err(|_| error("state_unavailable", "Legacy runtime records unavailable"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let known = name == "runtime-config.sealed"
            || ["profile-", "runtime-"].iter().any(|prefix| {
                name.strip_prefix(prefix)
                    .and_then(|n| n.strip_suffix(".sealed"))
                    .is_some_and(|s| {
                        (s.len() == 64 || (s.len() == 129 && s.as_bytes()[64] == b'-'))
                            && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                    })
            });
        if !known {
            continue;
        }
        let m = entry
            .metadata()
            .map_err(|_| error("state_unavailable", "Legacy runtime record unavailable"))?;
        if entry.file_type().is_ok_and(|t| t.is_symlink())
            || !m.is_file()
            || m.len() > crate::state::MAX_STATE as u64 + 64
        {
            return Err(error("state_corrupt", "Legacy runtime record is invalid"));
        }
        records.push((name, entry.path()));
        if records.len() > 4096 {
            return Err(error("state_limit", "Too many legacy runtime records"));
        }
    }
    // Config commits last; a partial copy can be retried without reading secrets.
    records.sort_by_key(|(name, _)| name == "runtime-config.sealed");
    for (name, source) in records {
        let target = destination.join(name);
        if std::fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(error(
                "state_corrupt",
                "Runtime migration target is invalid",
            ));
        }
        if !target.exists() {
            std::fs::copy(source, target).map_err(|_| {
                error(
                    "state_unavailable",
                    "Legacy runtime ciphertext cannot be copied",
                )
            })?;
        }
    }
    Ok(())
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
