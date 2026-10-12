//! How the supervisor reaches a server: [`McpHost`] launches stdio servers and
//! prepares HTTP endpoints. [`SandboxHost`] is the production host: stdio
//! servers run through `aidex-sandbox` `spawn_interactive` with a cleared
//! environment (the sandbox basics plus the server's `env_allowlist`) and the
//! vault secrets mapped by `env_from_secrets`; HTTP servers get their header
//! secrets and an egress check before every request. Imported servers' paths
//! resolve against their plugin's folder or workspace root (see `roots`).

use std::{
    fmt, fs,
    path::PathBuf,
    sync::{Arc, PoisonError},
    thread,
    time::Duration,
};

use aidex_events::{EventKind, EventScope, EventSink};
use aidex_sandbox::{
    CommandSpec, DEFAULT_ENV_ALLOWLIST, ManagedProcess, Mount, NetworkMode, ResourceLimits,
    Sandbox, SandboxError, SandboxLevel, SandboxRequest,
};
use aidex_vault::{ConsumerScope, SecretName, VaultErrorKind, VaultService};
use zeroize::Zeroizing;

use super::roots::SourceRoots;
use crate::{
    EndpointUrl, HttpEndpoint, LogBuffer, LogStream, McpError, McpErrorKind, ServerConfig,
    TransportKind, client::MAX_FRAME, config::validate_id, redact::bounded_redacted,
};

/// Bytes buffered per output stream of a server process between reads (several
/// frames, so a fast server never loses output to a slow poll).
const MAX_BUFFERED_OUTPUT: usize = 4 * MAX_FRAME;
/// Lifetime of the vault handle used to read one secret at launch.
const SECRET_HANDLE_TTL: Duration = Duration::from_secs(5);

/// A launched stdio server and the secret values it received (for scrubbing).
pub struct LaunchedServer {
    pub process: ManagedProcess,
    pub secrets: Vec<Zeroizing<String>>,
}

impl fmt::Debug for LaunchedServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchedServer")
            .field("process", self.process.id())
            .field(
                "secrets",
                &format_args!("{} (values redacted)", self.secrets.len()),
            )
            .finish()
    }
}

/// Launching and connecting, injected into the supervisor.
pub trait McpHost: Send + Sync {
    /// Start a stdio server process. Diagnostics may go to `logs`.
    fn launch_stdio(
        &self,
        config: &ServerConfig,
        logs: &LogBuffer,
    ) -> Result<LaunchedServer, McpError>;
    /// The endpoint (resolved credential headers and egress check) of an HTTP server.
    fn http_endpoint(&self, config: &ServerConfig) -> Result<HttpEndpoint, McpError>;
    /// A stdio server process ended after it was closed (for audit events).
    fn process_exited(&self, _process: &ManagedProcess) {}
    /// The server was removed: drop what the host kept for it between starts.
    fn forget(&self, _server_id: &str) {}
}

/// Policy checks around servers: the emergency stop for launches, and the offline
/// switch and network policy for every request to a remote server.
pub trait McpGate: Send + Sync {
    fn check_launch(&self, config: &ServerConfig) -> Result<(), McpError>;
    fn check_egress(&self, server_id: &str, url: &EndpointUrl) -> Result<(), McpError>;
}

/// A gate that allows everything (embedders and tests).
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenGate;

impl McpGate for OpenGate {
    fn check_launch(&self, _config: &ServerConfig) -> Result<(), McpError> {
        Ok(())
    }

    fn check_egress(&self, _server_id: &str, _url: &EndpointUrl) -> Result<(), McpError> {
        Ok(())
    }
}

/// Where secret values come from.
pub trait SecretSource: Send + Sync {
    /// The UTF-8 value of `name` for `consumer` (`mcp:<id>`).
    fn resolve(
        &self,
        consumer: &ConsumerScope,
        name: &SecretName,
    ) -> Result<Zeroizing<String>, McpError>;
}

/// Secrets from the drive vault: each value is read through a short-lived handle
/// scoped to the server's `mcp:<id>` consumer, which is revoked right away.
pub struct VaultSecrets {
    vault: Arc<VaultService>,
}

impl VaultSecrets {
    pub fn new(vault: Arc<VaultService>) -> Self {
        Self { vault }
    }
}

impl fmt::Debug for VaultSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VaultSecrets")
    }
}

impl SecretSource for VaultSecrets {
    fn resolve(
        &self,
        consumer: &ConsumerScope,
        name: &SecretName,
    ) -> Result<Zeroizing<String>, McpError> {
        let vault_error = |kind: VaultErrorKind| match kind {
            VaultErrorKind::Locked => McpError::new(
                McpErrorKind::VaultLocked,
                format!("unlock the vault so AiDEX can read secret `{name}` for {consumer}"),
            ),
            VaultErrorKind::NotFound => McpError::new(
                McpErrorKind::NeedsAuth,
                format!(
                    "no vault exists on this drive; create one and store secret `{name}` for {consumer}"
                ),
            ),
            VaultErrorKind::SecretNotFound => McpError::new(
                McpErrorKind::NeedsAuth,
                format!("store secret `{name}` in the vault with consumer {consumer}"),
            ),
            VaultErrorKind::ConsumerNotAllowed | VaultErrorKind::ForbiddenConsumer => {
                McpError::new(
                    McpErrorKind::NeedsAuth,
                    format!("secret `{name}` is not available to consumer {consumer}"),
                )
            }
            other => McpError::new(
                McpErrorKind::Unavailable,
                format!("the vault refused secret `{name}` ({})", other.as_str()),
            ),
        };
        let handle = self
            .vault
            .issue_handle(name, consumer, SECRET_HANDLE_TTL)
            .map_err(|error| vault_error(error.kind()))?;
        let secret = self.vault.resolve(&handle, consumer);
        self.vault.revoke_handle(&handle.handle_id);
        let secret = secret.map_err(|error| vault_error(error.kind()))?;
        let text = std::str::from_utf8(secret.expose()).map_err(|_| {
            McpError::new(
                McpErrorKind::NeedsAuth,
                format!("secret `{name}` is not UTF-8 text"),
            )
        })?;
        Ok(Zeroizing::new(text.to_owned()))
    }
}

/// The production host. See the module documentation.
pub struct SandboxHost {
    sandbox: Sandbox,
    scratch_root: PathBuf,
    secrets: Arc<dyn SecretSource>,
    gate: Arc<dyn McpGate>,
    events: Option<Arc<dyn EventSink>>,
    roots: SourceRoots,
}

impl fmt::Debug for SandboxHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxHost")
            .field("helper", &self.sandbox.helper_path())
            .field("scratch_root", &self.scratch_root)
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

impl SandboxHost {
    /// `scratch_root` holds one home/temp directory per server (kept between
    /// starts so package caches survive).
    pub fn new(
        sandbox: Sandbox,
        scratch_root: impl Into<PathBuf>,
        secrets: Arc<dyn SecretSource>,
        gate: Arc<dyn McpGate>,
    ) -> Self {
        Self {
            sandbox,
            scratch_root: scratch_root.into(),
            secrets,
            gate,
            events: None,
            roots: SourceRoots::default(),
        }
    }

    /// Record `process.started`/`process.exited` and `sandbox.negotiated` here.
    #[must_use]
    pub fn with_events(mut self, events: Arc<dyn EventSink>) -> Self {
        self.events = Some(events);
        self
    }

    /// Where plugins are installed (`DrivePaths::plugins()`): a plugin server's
    /// `${CLAUDE_PLUGIN_ROOT}`, `${AIDEX_PLUGIN_ROOT}`, and relative command
    /// resolve inside `<plugins>/<plugin name>`.
    #[must_use]
    pub fn with_plugins_dir(mut self, plugins: impl Into<PathBuf>) -> Self {
        self.roots.plugins = Some(plugins.into());
        self
    }

    /// How to find a workspace's root by id: a workspace server runs there, and
    /// its relative command resolves there. Without it, workspace servers run in
    /// their scratch folder.
    #[must_use]
    pub fn with_workspace_roots(
        mut self,
        roots: impl Fn(&str) -> Option<PathBuf> + Send + Sync + 'static,
    ) -> Self {
        self.roots.workspaces = Some(Arc::new(roots));
        self
    }

    fn secret(&self, config: &ServerConfig, name: &str) -> Result<Zeroizing<String>, McpError> {
        let consumer = ConsumerScope::mcp(config.id.as_str()).map_err(|_| {
            McpError::invalid_config(format!("`{}` is not a valid MCP consumer id", config.id))
        })?;
        let name = SecretName::new(name).map_err(|_| {
            McpError::invalid_config(format!(
                "`{}` is not a valid vault secret name",
                bounded_redacted(name, 64)
            ))
        })?;
        self.secrets.resolve(&consumer, &name)
    }

    fn emit(&self, event: EventKind) {
        if let Some(events) = &self.events {
            let _ = events.emit(EventScope::global(), event);
        }
    }
}

impl McpHost for SandboxHost {
    fn launch_stdio(
        &self,
        config: &ServerConfig,
        logs: &LogBuffer,
    ) -> Result<LaunchedServer, McpError> {
        if config.transport != TransportKind::Stdio {
            return Err(McpError::invalid_config("the server is not a stdio server"));
        }
        let command = config
            .command
            .as_deref()
            .ok_or_else(|| McpError::invalid_config("a stdio server needs a command"))?;
        self.gate.check_launch(config)?;
        let launch = self.roots.launch(config, command)?;
        let mut secrets: Vec<Zeroizing<String>> = Vec::new();
        let mut environment: Vec<(String, Zeroizing<String>)> = Vec::new();
        for mapping in &config.env_from_secrets {
            let value = self.secret(config, &mapping.secret_name)?;
            secrets.push(value.clone());
            environment.push((mapping.env.clone(), value));
        }
        let scratch = self.scratch_root.join(&config.id);
        fs::create_dir_all(&scratch).map_err(|error| {
            McpError::io("cannot create the server's scratch directory", &error)
        })?;
        let mut allowlist: Vec<String> = DEFAULT_ENV_ALLOWLIST
            .iter()
            .map(|name| (*name).to_owned())
            .collect();
        for name in &config.env_allowlist {
            if !allowlist
                .iter()
                .any(|known| known.eq_ignore_ascii_case(name))
            {
                allowlist.push(name.clone());
            }
        }
        let cwd = launch.cwd.clone().unwrap_or_else(|| scratch.clone());
        let mut request = SandboxRequest::new(SandboxLevel::Brokered, &cwd, &scratch)
            .with_network(NetworkMode::Normal)
            .with_limits(ResourceLimits {
                max_output_bytes: MAX_BUFFERED_OUTPUT,
                ..ResourceLimits::default()
            });
        if let Some(root) = &launch.cwd {
            request = request.with_mount(Mount::read_write(root));
        }
        request.env_allowlist = allowlist;
        request.env.extend(launch.env.iter().cloned());
        for (name, value) in &environment {
            request.env.push((name.clone(), value.as_str().to_owned()));
        }
        let negotiated = self.sandbox.negotiate(&request);
        // The request is the only other place holding the secret values.
        for (_, value) in &mut request.env {
            zeroize::Zeroize::zeroize(value);
        }
        drop(request);
        let plan = negotiated.map_err(|error| {
            self.emit(EventKind::SandboxNegotiated {
                level: aidex_events::SandboxLevel::Brokered,
                enforced: false,
            });
            launch_error(
                "the sandbox cannot run this MCP server on this host",
                &error,
            )
        })?;
        self.emit(EventKind::SandboxNegotiated {
            level: aidex_events::SandboxLevel::Brokered,
            enforced: true,
        });
        let program = plan.resolve_program(&launch.program).map_err(|error| match error {
            SandboxError::ProgramNotFound(_) => McpError::new(
                McpErrorKind::LaunchFailed,
                format!(
                    "`{}` was not found on the sandbox PATH; use an absolute path or allowlist the variables it needs",
                    bounded_redacted(command, 200)
                ),
            ),
            other => launch_error("the server command cannot be resolved", &other),
        })?;
        let process = self
            .sandbox
            .spawn_interactive(&plan, CommandSpec::new(program).args(launch.args))
            .map_err(|error| launch_error("the MCP server could not be launched", &error))?;
        logs.push(
            LogStream::Client,
            &format!(
                "launched {} in the sandbox (process {})",
                bounded_redacted(command, 200),
                process.id()
            ),
        );
        self.emit(EventKind::ProcessStarted {
            process_id: process.id().clone(),
            command_summary: bounded_redacted(&format!("mcp {}: {}", config.id, command), 256),
            sandbox_level: aidex_events::SandboxLevel::Brokered,
        });
        Ok(LaunchedServer { process, secrets })
    }

    fn http_endpoint(&self, config: &ServerConfig) -> Result<HttpEndpoint, McpError> {
        if config.transport != TransportKind::Http {
            return Err(McpError::invalid_config("the server is not an http server"));
        }
        let url = config
            .endpoint_url()
            .ok_or_else(|| McpError::invalid_config("the server URL is invalid"))?;
        // Refused before any secret is read.
        self.gate.check_egress(&config.id, &url)?;
        let mut headers = Vec::new();
        for mapping in &config.headers_from_secrets {
            let value = self.secret(config, &mapping.secret_name)?;
            if value.is_empty() || value.chars().any(|ch| ch.is_control()) {
                return Err(McpError::new(
                    McpErrorKind::NeedsAuth,
                    format!(
                        "secret `{}` cannot be sent as the `{}` header (empty or control characters)",
                        mapping.secret_name, mapping.header
                    ),
                ));
            }
            headers.push((mapping.header.clone(), value));
        }
        let gate = Arc::clone(&self.gate);
        let id = config.id.clone();
        let checked = url.clone();
        Ok(HttpEndpoint {
            url,
            headers,
            egress: Some(Arc::new(move || gate.check_egress(&id, &checked))),
        })
    }

    fn process_exited(&self, process: &ManagedProcess) {
        let status = process.wait_timeout(Duration::from_secs(1)).ok().flatten();
        self.emit(EventKind::ProcessExited {
            process_id: process.id().clone(),
            exit_code: status.as_ref().and_then(|status| status.code),
            signal: status.as_ref().and_then(|status| status.signal),
        });
    }

    /// Delete the server's scratch home (caches, files it wrote there). Best
    /// effort; only a real folder directly under the scratch root, and links
    /// inside it are removed, never followed.
    fn forget(&self, server_id: &str) {
        if validate_id(server_id).is_err() {
            return;
        }
        let scratch = self.scratch_root.join(server_id);
        let is_folder = fs::symlink_metadata(&scratch)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink());
        if !is_folder {
            return;
        }
        // Files of a process that just exited can stay locked for a moment.
        for _ in 0..3 {
            if fs::remove_dir_all(&scratch).is_ok() || !scratch.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn launch_error(context: &str, error: &SandboxError) -> McpError {
    McpError::new(
        McpErrorKind::LaunchFailed,
        format!("{context}: {}", bounded_redacted(&error.to_string(), 400)),
    )
}

/// Poison-tolerant lock helper shared by the supervisor.
pub(super) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
