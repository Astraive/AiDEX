//! The broker's [`ProcessLauncher`]: every tool process runs through
//! `aidex-sandbox` at the level the policy decision demands, and only for plans the
//! broker permitted.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use aidex_core::EntityId;
use aidex_events::{EventKind, EventScope, EventSink, ProcessStream};
use aidex_sandbox::{
    CapturedOutput, CommandSpec, EnforcementPlan, Mount, MountAccess, NetworkMode, ResourceLimits,
    Sandbox, SandboxError, SandboxLevel, SandboxRequest,
};
use aidex_tools::{
    CancelToken, LaunchError, LaunchOutcome, LaunchRequest, PlanFingerprint, ProcessLauncher,
};

use crate::{
    audit::events_level,
    secrets::{IssuedSecret, Secrets},
};

/// How often a running process is polled for exit and cancellation.
const POLL: Duration = Duration::from_millis(20);
/// How often captured output is forwarded as live `process.output` events.
const OUTPUT_INTERVAL: Duration = Duration::from_millis(100);
/// Largest `process.output` chunk.
const MAX_CHUNK: usize = 8 * 1024;

/// What the broker permitted for one plan.
pub(crate) struct LaunchGrant {
    pub session_id: EntityId,
    pub workspace_id: EntityId,
    /// Minimum isolation; launches never run weaker.
    pub level: SandboxLevel,
    /// Secret handles resolved into the environment at launch.
    pub secrets: Vec<IssuedSecret>,
}

/// Network mode for a launch: `normal` (governed by broker policy) when the tool
/// needs the network, or at `brokered` level where the OS applies no network
/// restriction by definition; `offline` (OS-enforced denial, fail closed where the
/// host cannot deny) at `restricted` and above.
pub(crate) fn network_mode(level: SandboxLevel, needs_network: bool) -> NetworkMode {
    if needs_network || level < SandboxLevel::Restricted {
        NetworkMode::Normal
    } else {
        NetworkMode::Offline
    }
}

/// Launches tool processes through `aidex-sandbox`. Refuses any launch whose plan
/// fingerprint the broker has not registered (a permit it issued).
pub struct SandboxLauncher {
    sandbox: Sandbox,
    scratch_root: PathBuf,
    events: Arc<dyn EventSink>,
    secrets: Secrets,
    grants: Mutex<HashMap<PlanFingerprint, LaunchGrant>>,
    sequence: AtomicU64,
}

impl std::fmt::Debug for SandboxLauncher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxLauncher")
            .field("helper", &self.sandbox.helper_path())
            .field("scratch_root", &self.scratch_root)
            .field("permits", &self.lock().len())
            .finish_non_exhaustive()
    }
}

/// Removes a launch permit when the call ends.
pub(crate) struct PermitGuard<'a> {
    launcher: &'a SandboxLauncher,
    fingerprint: PlanFingerprint,
}

impl Drop for PermitGuard<'_> {
    fn drop(&mut self) {
        self.launcher.lock().remove(&self.fingerprint);
    }
}

impl SandboxLauncher {
    pub(crate) fn new(
        sandbox: Sandbox,
        scratch_root: PathBuf,
        events: Arc<dyn EventSink>,
        secrets: Secrets,
    ) -> Self {
        Self {
            sandbox,
            scratch_root,
            events,
            secrets,
            grants: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(0),
        }
    }

    pub fn sandbox(&self) -> &Sandbox {
        &self.sandbox
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<PlanFingerprint, LaunchGrant>> {
        self.grants.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Allow launches for `fingerprint` until the guard drops.
    pub(crate) fn permit(
        &self,
        fingerprint: PlanFingerprint,
        grant: LaunchGrant,
    ) -> PermitGuard<'_> {
        self.lock().insert(fingerprint, grant);
        PermitGuard {
            launcher: self,
            fingerprint,
        }
    }

    /// Withdraw every launch permit (emergency stop). Returns how many were live.
    pub(crate) fn withdraw_all(&self) -> usize {
        let mut grants = self.lock();
        let count = grants.len();
        grants.clear();
        count
    }

    /// A fresh scratch directory for one launch or check.
    fn scratch(&self, label: &str) -> Result<PathBuf, String> {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let dir = self
            .scratch_root
            .join(format!("{label}-{}-{sequence}", std::process::id()));
        fs::create_dir_all(&dir)
            .map_err(|error| format!("cannot create scratch directory: {error}"))?;
        Ok(dir)
    }

    /// The sandbox request for running in `root` at `level`.
    fn request(
        level: SandboxLevel,
        root: &Path,
        cwd: &Path,
        scratch: &Path,
        access: MountAccess,
        needs_network: bool,
        limits: ResourceLimits,
    ) -> SandboxRequest {
        SandboxRequest::new(level, cwd, scratch)
            .with_mount(Mount {
                host_path: root.to_path_buf(),
                access,
            })
            .with_network(network_mode(level, needs_network))
            .with_limits(limits)
    }

    /// Negotiate (without launching) what a tool in `root` would run under, so a
    /// sandbox the host cannot provide denies the call before any approval or effect.
    pub(crate) fn check(
        &self,
        level: SandboxLevel,
        root: &Path,
        access: MountAccess,
        needs_network: bool,
        wall_timeout: Duration,
    ) -> Result<EnforcementPlan, String> {
        let scratch = self.scratch("check")?;
        let limits = ResourceLimits {
            wall_timeout: Some(wall_timeout),
            ..ResourceLimits::default()
        };
        let request = Self::request(level, root, root, &scratch, access, needs_network, limits);
        let result = self
            .sandbox
            .negotiate(&request)
            .map_err(|error| error.to_string());
        let _ = fs::remove_dir_all(&scratch);
        result
    }

    fn emit(&self, scope: &EventScope, event: EventKind) -> bool {
        self.events.emit(scope.clone(), event).is_ok()
    }

    fn run(
        &self,
        request: &LaunchRequest,
        cancel: &CancelToken,
        level: SandboxLevel,
        secrets: &[IssuedSecret],
        scratch: &Path,
    ) -> Result<LaunchOutcome, LaunchError> {
        let scope =
            EventScope::session(request.workspace_id().clone(), request.session_id().clone());
        let limits = ResourceLimits {
            wall_timeout: Some(request.timeout()),
            max_output_bytes: request.max_output_bytes(),
            ..ResourceLimits::default()
        };
        let mut sandbox_request = Self::request(
            level,
            request.workspace_root(),
            request.cwd(),
            scratch,
            request.workspace_access(),
            request.network(),
            limits,
        );
        for (name, value) in request.env() {
            sandbox_request = sandbox_request.with_env(name, value);
        }
        // Secret values exist only here, in the request handed to the sandbox.
        let secret_env = self
            .secrets
            .environment(secrets)
            .map_err(LaunchError::Denied)?;
        for (name, value) in secret_env.0 {
            sandbox_request = sandbox_request.with_env(name, value);
        }
        let plan = match self.sandbox.negotiate(&sandbox_request) {
            Ok(plan) => plan,
            Err(error) => {
                self.emit(
                    &scope,
                    EventKind::SandboxNegotiated {
                        level: events_level(level),
                        enforced: false,
                    },
                );
                return Err(LaunchError::Sandbox(error));
            }
        };
        drop(sandbox_request);
        if !self.emit(
            &scope,
            EventKind::SandboxNegotiated {
                level: events_level(plan.level),
                enforced: true,
            },
        ) {
            return Err(LaunchError::Denied(
                "sandbox.negotiated could not be recorded".into(),
            ));
        }
        let program = plan
            .resolve_program(request.program())
            .map_err(|error| match error {
                SandboxError::ProgramNotFound(_) => {
                    LaunchError::ProgramNotFound(request.program().to_owned())
                }
                other => LaunchError::Sandbox(other),
            })?;
        let mut command = CommandSpec::new(program).args(request.args().iter().cloned());
        if let Some(stdin) = request.stdin() {
            command = command.stdin(stdin.to_vec());
        }
        let process = self
            .sandbox
            .spawn(&plan, command)
            .map_err(LaunchError::Sandbox)?;
        let process_id = process.id().clone();
        if !self.emit(
            &scope,
            EventKind::ProcessStarted {
                process_id: process_id.clone(),
                command_summary: request.display().to_owned(),
                sandbox_level: events_level(plan.level),
            },
        ) {
            let _ = process.kill_tree();
            let _ = process.wait();
            return Err(LaunchError::Denied(
                "process.started could not be recorded; the process was stopped".into(),
            ));
        }
        let mut streamed = [0usize; 2];
        let mut last_output = Instant::now();
        let mut kill_requested = false;
        let status = loop {
            match process.wait_timeout(POLL) {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => {
                    let _ = process.kill_tree();
                    return Err(LaunchError::Sandbox(error));
                }
            }
            if cancel.is_stopped() && !kill_requested {
                kill_requested = true;
                let _ = process.kill_tree();
            }
            if last_output.elapsed() >= OUTPUT_INTERVAL {
                last_output = Instant::now();
                self.stream(&scope, &process_id, &process.output_so_far(), &mut streamed);
            }
        };
        let output = process.take_output();
        self.stream(&scope, &process_id, &output, &mut streamed);
        self.emit(
            &scope,
            EventKind::ProcessExited {
                process_id: process_id.clone(),
                exit_code: status.code,
                signal: status.signal,
            },
        );
        Ok(LaunchOutcome {
            process_id: Some(process_id),
            sandbox_level: plan.level,
            status,
            output,
        })
    }

    /// Forward output captured since the last call as live `process.output` events
    /// (redacted and bounded by the event sink).
    fn stream(
        &self,
        scope: &EventScope,
        process_id: &EntityId,
        output: &CapturedOutput,
        streamed: &mut [usize; 2],
    ) {
        for (index, (stream, capture)) in [
            (ProcessStream::Stdout, &output.stdout),
            (ProcessStream::Stderr, &output.stderr),
        ]
        .into_iter()
        .enumerate()
        {
            let bytes = &capture.bytes;
            while streamed[index] < bytes.len() {
                let end = (streamed[index] + MAX_CHUNK).min(bytes.len());
                let chunk = String::from_utf8_lossy(&bytes[streamed[index]..end]).into_owned();
                streamed[index] = end;
                let _ = self.events.emit(
                    scope.clone(),
                    EventKind::ProcessOutput {
                        process_id: process_id.clone(),
                        stream,
                        chunk,
                    },
                );
            }
        }
    }
}

impl ProcessLauncher for SandboxLauncher {
    fn launch(
        &self,
        request: &LaunchRequest,
        cancel: &CancelToken,
    ) -> Result<LaunchOutcome, LaunchError> {
        let (level, secrets) = {
            let grants = self.lock();
            let grant = grants.get(&request.plan_fingerprint()).ok_or_else(|| {
                LaunchError::Denied("the broker issued no permit for this plan".into())
            })?;
            if &grant.session_id != request.session_id()
                || &grant.workspace_id != request.workspace_id()
            {
                return Err(LaunchError::Denied(
                    "the permit was issued for another session or workspace".into(),
                ));
            }
            (grant.level, grant.secrets.clone())
        };
        let scratch = self.scratch("run").map_err(LaunchError::Io)?;
        let result = self.run(request, cancel, level, &secrets, &scratch);
        let _ = fs::remove_dir_all(&scratch);
        result
    }
}
