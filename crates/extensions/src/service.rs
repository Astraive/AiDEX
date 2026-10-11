use crate::{
    model::{SkillRecord, State},
    storage::{atomic, digest, read, safe_path, valid_id, validate_grants},
    *,
};
use aidex_drive::DrivePaths;
use aidex_tools::CancelToken;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize},
    },
};

pub(crate) struct Connected {
    pub scope: CallScope,
    pub config_digest: String,
    pub revision: String,
    pub tools: Vec<McpTool>,
    pub connection: Mutex<Box<dyn McpConnection>>,
    pub active_cancel: Mutex<Option<CancelToken>>,
    pub owner_session: Mutex<Option<String>>,
    pub credential_versions: BTreeMap<String, aidex_vault::SecretVersion>,
}

pub(crate) struct PendingConnect {
    pub generation: String,
    pub cancel: CancelToken,
}

pub struct ExtensionService {
    pub(crate) paths: DrivePaths,
    pub(crate) state: Mutex<State>,
    pub(crate) connected: Mutex<BTreeMap<String, Arc<Connected>>>,
    pub(crate) scopes: Mutex<BTreeMap<String, CallScope>>,
    pub(crate) secrets: Arc<dyn ExtensionSecrets>,
    pub(crate) connector: Arc<dyn McpConnector>,
    pub(crate) network_gate: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) lifecycle: Mutex<()>,
    pub(crate) pending_connect: Mutex<BTreeMap<String, PendingConnect>>,
    pub(crate) connect_errors: Mutex<BTreeMap<String, String>>,
    pub(crate) connect_jobs: Arc<AtomicUsize>,
    pub(crate) stopped: AtomicBool,
}
pub(crate) fn lock<T>(value: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    value
        .lock()
        .map_err(|_| ExtensionError::Unavailable("extension service lock poisoned".into()))
}
pub(crate) fn hash_json(value: &impl serde::Serialize) -> Result<String> {
    Ok(digest(&serde_json::to_vec(value).map_err(|_| {
        ExtensionError::Invalid("invalid metadata".into())
    })?))
}

impl ExtensionService {
    pub fn open(
        paths: DrivePaths,
        vault: Arc<aidex_vault::VaultService>,
        network_gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Arc<Self>> {
        let admission_vault = vault.clone();
        let network_gate: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            network_gate() && admission_vault.status().state == aidex_vault::VaultState::Unlocked
        });
        let connector = RealConnector::for_drive(aidex_sandbox::Sandbox::locate(), &paths)
            .with_network_gate(network_gate.clone());
        Self::with_connector_and_gate(
            paths,
            Arc::new(VaultSecrets(vault)),
            Arc::new(connector),
            network_gate,
        )
    }
    pub fn with_connector(
        paths: DrivePaths,
        secrets: Arc<dyn ExtensionSecrets>,
        connector: Arc<dyn McpConnector>,
    ) -> Result<Arc<Self>> {
        Self::with_connector_and_gate(paths, secrets, connector, Arc::new(|| true))
    }
    pub fn with_connector_and_gate(
        paths: DrivePaths,
        secrets: Arc<dyn ExtensionSecrets>,
        connector: Arc<dyn McpConnector>,
        network_gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Arc<Self>> {
        for path in [
            paths.config(),
            paths.skills(),
            paths.mcp(),
            paths.plugins(),
            paths.tmp(),
        ] {
            paths.ensure_dir(&path).map_err(|_| ExtensionError::Io)?;
        }
        let path = safe_path(&paths.config(), "extensions.json")?;
        let state: State = if path.exists() {
            serde_json::from_str(&read(&path, 4 * 1024 * 1024)?)
                .map_err(|_| ExtensionError::Invalid("malformed extension registry".into()))?
        } else {
            State::default()
        };
        if state.skills.len() > 1024 || state.servers.len() > 128 || state.plugins.len() > 128 {
            return Err(ExtensionError::TooLarge);
        }
        for (id, skill) in &state.skills {
            valid_id(id)?;
            if id != &skill.summary.id {
                return Err(ExtensionError::Invalid("skill registry id mismatch".into()));
            }
            let root = if skill.summary.plugin_id.is_some() {
                paths.plugins()
            } else {
                paths.skills()
            };
            safe_path(&root, &skill.path)?;
        }
        for (id, server) in &state.servers {
            valid_id(id)?;
            if id != &server.config.id {
                return Err(ExtensionError::Invalid("MCP registry id mismatch".into()));
            }
            validate_server(&server.config)?;
        }
        for (id, plugin) in &state.plugins {
            valid_id(id)?;
            if id != &plugin.manifest.id {
                return Err(ExtensionError::Invalid(
                    "plugin registry id mismatch".into(),
                ));
            }
        }
        Ok(Arc::new(Self {
            paths,
            state: Mutex::new(state),
            connected: Mutex::new(BTreeMap::new()),
            scopes: Mutex::new(BTreeMap::new()),
            secrets,
            connector,
            network_gate,
            lifecycle: Mutex::new(()),
            pending_connect: Mutex::new(BTreeMap::new()),
            connect_errors: Mutex::new(BTreeMap::new()),
            connect_jobs: Arc::new(AtomicUsize::new(0)),
            stopped: AtomicBool::new(false),
        }))
    }
    pub(crate) fn persist(&self, state: &State) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(state).map_err(|_| ExtensionError::Io)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err(ExtensionError::TooLarge);
        }
        atomic(&safe_path(&self.paths.config(), "extensions.json")?, &bytes)
    }
    pub(crate) fn skill_path(&self, record: &SkillRecord) -> Result<PathBuf> {
        let root = if record.summary.plugin_id.is_some() {
            self.paths.plugins()
        } else {
            self.paths.skills()
        };
        safe_path(&root, &record.path)
    }
    pub fn bind_scope(&self, session_id: &str, scope: CallScope) -> Result<()> {
        aidex_core::EntityId::new(session_id)
            .map_err(|_| ExtensionError::Invalid("invalid session id".into()))?;
        if !scope.workspace_root.is_absolute() || !scope.workspace_root.is_dir() {
            return Err(ExtensionError::Invalid("workspace root must exist".into()));
        }
        if scope.selected_skill_ids.len() > 128 {
            return Err(ExtensionError::TooLarge);
        }
        for id in &scope.selected_skill_ids {
            valid_id(id)?;
        }
        let mut scopes = lock(&self.scopes)?;
        if scopes.len() >= 1024 && !scopes.contains_key(session_id) {
            return Err(ExtensionError::TooLarge);
        }
        scopes.insert(session_id.into(), scope);
        Ok(())
    }
    pub fn clear_scope(&self, session_id: &str) {
        if let Ok(mut scopes) = self.scopes.lock() {
            scopes.remove(session_id);
        }
    }
    pub(crate) fn scope(&self, ctx: &aidex_tools::ToolContext) -> Result<CallScope> {
        let scope = lock(&self.scopes)?
            .get(ctx.session_id.as_str())
            .cloned()
            .ok_or_else(|| ExtensionError::Forbidden("session scope is not bound".into()))?;
        if scope.workspace_id != ctx.workspace_id.as_str()
            || fs::canonicalize(&scope.workspace_root)?
                != fs::canonicalize(ctx.root.display_root())?
        {
            return Err(ExtensionError::Forbidden(
                "session workspace mismatch".into(),
            ));
        }
        Ok(scope)
    }
}

impl Drop for ExtensionService {
    fn drop(&mut self) {
        self.shutdown();
    }
}
pub(crate) fn validate_server(config: &McpServerConfig) -> Result<()> {
    valid_id(&config.id)?;
    validate_grants(&config.workspace_ids)?;
    validate_grants(&config.profiles)?;
    if config.name.is_empty()
        || config.name.len() > 128
        || config.timeout_ms < 100
        || config.timeout_ms > 120000
        || config.max_output_bytes < 1024
        || config.max_output_bytes > 1024 * 1024
        || config.secret_refs.len() > 32
    {
        return Err(ExtensionError::Invalid("invalid MCP limits or name".into()));
    }
    for (target, name) in &config.secret_refs {
        if target.is_empty()
            || target.len() > 128
            || !target
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(ExtensionError::Invalid(
                "invalid credential destination".into(),
            ));
        }
        aidex_vault::SecretName::new(name)
            .map_err(|_| ExtensionError::Invalid("invalid vault secret name".into()))?;
    }
    match &config.transport {
        McpTransport::Stdio { command, args, cwd } => {
            if command.is_empty()
                || command.len() > 4096
                || command.contains('\0')
                || args.len() > 128
                || args.iter().any(|a| a.len() > 8192 || a.contains('\0'))
            {
                return Err(ExtensionError::Invalid("invalid stdio command".into()));
            }
            if cwd
                .as_ref()
                .is_some_and(|cwd| cwd.len() > 4096 || cwd.contains('\0'))
            {
                return Err(ExtensionError::Invalid("invalid MCP cwd".into()));
            }
        }
        McpTransport::Https { url } => {
            crate::transport::endpoint(url)?;
            for name in config.secret_refs.keys() {
                if matches!(
                    name.to_ascii_lowercase().as_str(),
                    "host"
                        | "content-type"
                        | "content-length"
                        | "mcp-session-id"
                        | "mcp-protocol-version"
                        | "accept"
                        | "cookie"
                ) {
                    return Err(ExtensionError::Invalid("reserved MCP header".into()));
                }
            }
        }
    }
    Ok(())
}
