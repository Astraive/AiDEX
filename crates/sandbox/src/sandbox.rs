//! The sandbox service: probe once, negotiate plans, and launch managed processes.

use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, mpsc},
    thread,
    time::Duration,
};

use aidex_core::EntityId;

use crate::{
    BackendCapabilities, CommandSpec, EnforcementPlan, HelperStatus, ManagedProcess,
    ProcessRegistry, SandboxError, SandboxRequest, backend, env as sandbox_env,
    helper::protocol,
    negotiate::negotiate_with,
    paths,
    process::{
        KillReason, ProcessShared,
        monitor::{self, LaunchedPipes},
    },
};

/// File name of the helper binary that must ship next to `aidexd`.
pub const HELPER_BINARY_NAME: &str = "aidex-sandbox-exec";

/// How long the helper may take to confirm that its restrictions are in place.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

struct SandboxInner {
    helper: PathBuf,
    capabilities: OnceLock<BackendCapabilities>,
    registry: ProcessRegistry,
}

/// Entry point for sandboxed execution. Cloning shares the probe cache and registry.
#[derive(Clone)]
pub struct Sandbox {
    inner: Arc<SandboxInner>,
}

impl Sandbox {
    /// A sandbox that launches through the helper at `helper`.
    pub fn new(helper: impl Into<PathBuf>) -> Self {
        Self::with_registry(helper, ProcessRegistry::new())
    }

    /// A sandbox that records its processes in an existing registry.
    pub fn with_registry(helper: impl Into<PathBuf>, registry: ProcessRegistry) -> Self {
        Self {
            inner: Arc::new(SandboxInner {
                helper: helper.into(),
                capabilities: OnceLock::new(),
                registry,
            }),
        }
    }

    /// A sandbox using the helper next to the current executable (see
    /// [`default_helper_path`]).
    pub fn locate() -> Self {
        Self::new(default_helper_path())
    }

    pub fn helper_path(&self) -> &Path {
        &self.inner.helper
    }

    pub fn registry(&self) -> &ProcessRegistry {
        &self.inner.registry
    }

    /// The host's capabilities, probed on first use and cached.
    pub fn capabilities(&self) -> &BackendCapabilities {
        self.inner
            .capabilities
            .get_or_init(|| backend::probe(helper_status(&self.inner.helper)))
    }

    /// Produce a plan stating exactly what will be enforced, or fail closed.
    pub fn negotiate(&self, request: &SandboxRequest) -> Result<EnforcementPlan, SandboxError> {
        negotiate_with(
            self.capabilities(),
            request,
            sandbox_env::parent_environment(),
        )
    }

    /// Launch `command` under `plan`. Returns only after the helper confirmed that every
    /// planned restriction is in place; otherwise nothing ran and an error is returned.
    pub fn spawn(
        &self,
        plan: &EnforcementPlan,
        command: CommandSpec,
    ) -> Result<ManagedProcess, SandboxError> {
        self.spawn_inner(plan, command, false)
    }

    /// Launch with stdin retained for a bidirectional protocol. The helper consumes
    /// only its exact framed plan, and the tracked target inherits the remaining pipe.
    pub fn spawn_interactive(
        &self,
        plan: &EnforcementPlan,
        command: CommandSpec,
    ) -> Result<ManagedProcess, SandboxError> {
        self.spawn_inner(plan, command, true)
    }

    fn spawn_inner(
        &self,
        plan: &EnforcementPlan,
        command: CommandSpec,
        interactive: bool,
    ) -> Result<ManagedProcess, SandboxError> {
        self.check_plan(plan)?;
        if !command.program.is_absolute() {
            return Err(SandboxError::ProgramNotFound(format!(
                "{} is not absolute; resolve it with EnforcementPlan::resolve_program",
                command.program.display()
            )));
        }
        let program = paths::canonical_file(&command.program)?;
        if command.args.iter().any(|arg| arg.contains('\0')) {
            return Err(SandboxError::invalid(
                "arguments must not contain NUL bytes",
            ));
        }
        backend::check_program(plan, &program)?;
        let command = CommandSpec { program, ..command };
        let frame = protocol::encode_plan(&backend::helper_plan(plan, &command)?)
            .map_err(|error| SandboxError::io("encoding helper plan", &error))?;

        let id = EntityId::generate("proc").map_err(|error| SandboxError::Io(error.to_string()))?;
        let shared = Arc::new(ProcessShared::new(
            id,
            Arc::new(plan.clone()),
            self.inner.registry.clone(),
        ));
        self.inner.registry.register(&shared)?;
        match start(&shared, frame, command.stdin, interactive) {
            Ok(()) => Ok(ManagedProcess { shared }),
            Err(error) => {
                self.inner.registry.remove(&shared.id);
                Err(error)
            }
        }
    }

    fn check_plan(&self, plan: &EnforcementPlan) -> Result<(), SandboxError> {
        let capabilities = self.capabilities();
        if plan.backend != capabilities.backend {
            return Err(SandboxError::PlanMismatch(format!(
                "plan targets {} but this host uses {}",
                plan.backend.as_str(),
                capabilities.backend.as_str()
            )));
        }
        if plan.helper != self.inner.helper {
            return Err(SandboxError::PlanMismatch(
                "plan was negotiated against a different helper".into(),
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox")
            .field("helper", &self.inner.helper)
            .field("registry", &self.inner.registry)
            .finish()
    }
}

fn spawn_thread(name: String, body: impl FnOnce() + Send + 'static) -> Result<(), SandboxError> {
    thread::Builder::new()
        .name(name)
        .spawn(body)
        .map(drop)
        .map_err(|error| SandboxError::io("starting sandbox thread", &error))
}

/// Launch on the monitor thread, wire the pipes, and wait for the helper's handshake.
fn start(
    shared: &Arc<ProcessShared>,
    frame: Vec<u8>,
    stdin: Option<Vec<u8>>,
    interactive: bool,
) -> Result<(), SandboxError> {
    let (launched_tx, launched_rx) = mpsc::sync_channel::<Result<LaunchedPipes, SandboxError>>(1);
    let monitor_shared = Arc::clone(shared);
    spawn_thread(format!("aidex-sandbox-monitor-{}", shared.id), move || {
        monitor::run_monitor(monitor_shared, launched_tx);
    })?;
    let pipes = match launched_rx.recv() {
        Ok(Ok(pipes)) => pipes,
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            return Err(SandboxError::Spawn(
                "monitor thread exited during launch".into(),
            ));
        }
    };
    let (Some(stdin_pipe), Some(stdout), Some(stderr)) = (pipes.stdin, pipes.stdout, pipes.stderr)
    else {
        let _ = shared.kill(KillReason::HandshakeFailed);
        return Err(SandboxError::Spawn("helper pipes were not created".into()));
    };

    let (ready_tx, ready_rx) = mpsc::sync_channel::<bool>(1);
    let started = [
        {
            let shared = Arc::clone(shared);
            spawn_thread(format!("aidex-sandbox-stdout-{}", shared.id), move || {
                monitor::run_stdout_reader(shared, stdout, ready_tx);
            })
        },
        {
            let shared = Arc::clone(shared);
            spawn_thread(format!("aidex-sandbox-stderr-{}", shared.id), move || {
                monitor::run_stderr_reader(shared, stderr);
            })
        },
        {
            let shared = Arc::clone(shared);
            spawn_thread(format!("aidex-sandbox-stdin-{}", shared.id), move || {
                if interactive {
                    use std::io::Write;
                    // Hand the pipe over before the plan can reach the helper and keep it
                    // locked until the plan is written. The helper confirms only after
                    // reading the plan, so once `spawn_interactive` returns the caller's
                    // first write always finds the pipe (it used to race this thread and
                    // fail as closed input) and can never land ahead of the plan.
                    let mut slot = shared
                        .stdin
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let pipe = slot.insert(stdin_pipe);
                    if pipe
                        .write_all(&frame)
                        .and_then(|()| pipe.write_all(stdin.as_deref().unwrap_or_default()))
                        .and_then(|()| pipe.flush())
                        .is_err()
                    {
                        slot.take();
                        drop(slot);
                        let _ = shared.kill(KillReason::HandshakeFailed);
                    }
                } else {
                    monitor::run_stdin_writer(stdin_pipe, frame, stdin);
                }
            })
        },
    ];
    if let Some(Err(error)) = started.into_iter().find(Result::is_err) {
        let _ = shared.kill(KillReason::HandshakeFailed);
        return Err(error);
    }

    if ready_rx.recv_timeout(HANDSHAKE_TIMEOUT) != Ok(true) {
        let _ = shared.kill(KillReason::HandshakeFailed);
        let result = shared.wait_finished(Some(HANDSHAKE_TIMEOUT));
        return Err(match (result, shared.setup_failure()) {
            (Some(Err(error @ SandboxError::EnforcementFailed { .. })), _) => error,
            (_, Some((stage, message))) => SandboxError::EnforcementFailed { stage, message },
            _ => SandboxError::EnforcementFailed {
                stage: "handshake".into(),
                message: "the helper exited before confirming the sandbox".into(),
            },
        });
    }

    let deadline = shared
        .plan
        .limits
        .wall_timeout
        .filter(|_| shared.plan.applies(crate::Requirement::WallTimeout))
        .map(|timeout| shared.started + timeout);
    let watchdog = backend::Watchdog::for_plan(&shared.plan);
    if deadline.is_some() || watchdog.is_some() {
        let supervisor_shared = Arc::clone(shared);
        if let Err(error) = spawn_thread(
            format!("aidex-sandbox-supervisor-{}", shared.id),
            move || {
                monitor::run_supervisor(supervisor_shared, deadline, watchdog);
            },
        ) {
            let _ = shared.kill(KillReason::HandshakeFailed);
            return Err(error);
        }
    }
    Ok(())
}

fn helper_status(path: &Path) -> HelperStatus {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => HelperStatus::Available {
            path: path.to_path_buf(),
        },
        Ok(_) => HelperStatus::Missing {
            path: path.to_path_buf(),
            reason: "not a file".into(),
        },
        Err(error) => HelperStatus::Missing {
            path: path.to_path_buf(),
            reason: error.to_string(),
        },
    }
}

/// `aidex-sandbox-exec` next to the current executable. When the executable lives in a
/// Cargo `deps/` directory (tests), the parent directory is checked as well.
pub fn default_helper_path() -> PathBuf {
    let name = format!("{HELPER_BINARY_NAME}{}", env::consts::EXE_SUFFIX);
    let Some(directory) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return PathBuf::from(name);
    };
    let beside = directory.join(&name);
    if !beside.is_file()
        && directory.file_name().is_some_and(|dir| dir == "deps")
        && let Some(parent) = directory.parent()
    {
        let candidate = parent.join(&name);
        if candidate.is_file() {
            return candidate;
        }
    }
    beside
}

/// Probe this host with the default helper location (doctor / `aidex-host` hook).
pub fn probe() -> BackendCapabilities {
    backend::probe(helper_status(&default_helper_path()))
}

/// Negotiate `request` with the default helper location.
pub fn negotiate(request: &SandboxRequest) -> Result<EnforcementPlan, SandboxError> {
    negotiate_with(&probe(), request, sandbox_env::parent_environment())
}
