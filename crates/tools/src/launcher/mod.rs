//! The process-launching boundary. Tools never spawn processes themselves: they
//! describe a launch bound to an approved plan and hand it to a [`ProcessLauncher`],
//! which the broker implements over `aidex-sandbox`.

mod cmdline;
#[cfg(feature = "direct-launcher")]
mod direct;

use std::{fmt, path::PathBuf, time::Duration};

use aidex_core::EntityId;
use aidex_sandbox::{CapturedOutput, ExitStatusInfo, MountAccess, SandboxLevel};

pub(crate) use cmdline::{command_line, display_command};
#[cfg(feature = "direct-launcher")]
pub use direct::DirectLauncher;

use crate::{
    CancelToken, ExecutionPermit, LaunchError, PermitIssuer, PlanFingerprint, ToolContext,
    ToolError, ToolId, ToolPlan,
};

/// Runs one process for a tool call, blocking until it ends.
///
/// The production implementation (the execution broker) maps a request onto an
/// `aidex_sandbox::SandboxRequest` — the workspace mounted with
/// [`LaunchRequest::workspace_access`], network per [`LaunchRequest::network`] and
/// policy, the decision's minimum sandbox level, its environment allowlist, and any
/// secret handles it resolves — and should refuse requests whose
/// [`LaunchRequest::plan_fingerprint`] it did not permit. Implementations must
/// terminate the process tree when `cancel` fires or the timeout elapses and must
/// bound captured output to [`LaunchRequest::max_output_bytes`]. Launchers own the
/// `process.*` events.
pub trait ProcessLauncher: Send + Sync {
    fn launch(
        &self,
        request: &LaunchRequest,
        cancel: &CancelToken,
    ) -> Result<LaunchOutcome, LaunchError>;
}

/// How a launched process ended.
#[derive(Clone, Debug)]
pub struct LaunchOutcome {
    /// The managed process id (`proc_…`) when the launcher tracks one.
    pub process_id: Option<EntityId>,
    /// The isolation level actually enforced.
    pub sandbox_level: SandboxLevel,
    pub status: ExitStatusInfo,
    pub output: CapturedOutput,
}

/// A fixed program + argument vector (never a shell string unless the program is
/// itself a shell chosen by `shell.exec`), bound to one permitted plan.
#[derive(Clone)]
pub struct LaunchRequest {
    tool_id: ToolId,
    session_id: EntityId,
    workspace_id: EntityId,
    call_id: EntityId,
    plan_fingerprint: PlanFingerprint,
    issuer: PermitIssuer,
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
    workspace_root: PathBuf,
    workspace_access: MountAccess,
    env: Vec<(String, String)>,
    stdin: Option<Vec<u8>>,
    network: bool,
    timeout: Duration,
    max_output_bytes: usize,
    display: String,
}

impl LaunchRequest {
    /// A request for `plan`, authorized by `permit` (which must be the plan's).
    /// Defaults: cwd = workspace root, read-only workspace, no network, the
    /// context's process timeout and output bound, no extra environment, no stdin.
    pub fn new(
        ctx: &ToolContext,
        plan: &ToolPlan,
        permit: &ExecutionPermit,
        program: impl Into<String>,
        args: Vec<String>,
    ) -> Result<Self, ToolError> {
        if permit.fingerprint() != plan.fingerprint() {
            return Err(ToolError::PlanMismatch(
                "the permit was issued for a different plan".into(),
            ));
        }
        let program = program.into();
        let display = display_command(&program, &args);
        let root = ctx.root.display_root();
        Ok(Self {
            tool_id: plan.tool_id().clone(),
            session_id: plan.session_id().clone(),
            workspace_id: plan.workspace_id().clone(),
            call_id: plan.call_id().clone(),
            plan_fingerprint: plan.fingerprint(),
            issuer: permit.issuer().clone(),
            program,
            args,
            cwd: root.clone(),
            workspace_root: root,
            workspace_access: MountAccess::ReadOnly,
            env: Vec::new(),
            stdin: None,
            network: false,
            timeout: ctx.limits.process_timeout,
            max_output_bytes: ctx.limits.max_output_bytes,
            display,
        })
    }

    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = cwd;
        self
    }

    /// Add a fixed, non-secret environment variable (secrets are injected by the
    /// launcher from vault handles, never by tools).
    pub fn with_env(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((name.into(), value.into()));
        self
    }

    pub fn with_stdin(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
        self
    }

    pub fn with_workspace_access(mut self, access: MountAccess) -> Self {
        self.workspace_access = access;
        self
    }

    pub fn with_network(mut self, network: bool) -> Self {
        self.network = network;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn tool_id(&self) -> &ToolId {
        &self.tool_id
    }

    pub fn session_id(&self) -> &EntityId {
        &self.session_id
    }

    pub fn workspace_id(&self) -> &EntityId {
        &self.workspace_id
    }

    pub fn call_id(&self) -> &EntityId {
        &self.call_id
    }

    pub fn plan_fingerprint(&self) -> PlanFingerprint {
        self.plan_fingerprint
    }

    pub fn issuer(&self) -> &PermitIssuer {
        &self.issuer
    }

    /// Bare program name (resolved on the launcher's PATH) or absolute path.
    pub fn program(&self) -> &str {
        &self.program
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    /// The workspace root (display form, no Windows verbatim prefix).
    pub fn workspace_root(&self) -> &PathBuf {
        &self.workspace_root
    }

    pub fn workspace_access(&self) -> MountAccess {
        self.workspace_access
    }

    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    pub fn stdin(&self) -> Option<&[u8]> {
        self.stdin.as_deref()
    }

    /// Whether the process needs network access (policy still decides where).
    pub fn network(&self) -> bool {
        self.network
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    pub fn max_output_bytes(&self) -> usize {
        self.max_output_bytes
    }

    /// The command line with likely credentials redacted, for display and audit.
    pub fn display(&self) -> &str {
        &self.display
    }
}

impl fmt::Debug for LaunchRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let env: Vec<&str> = self.env.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("LaunchRequest")
            .field("tool_id", &self.tool_id)
            .field("call_id", &self.call_id)
            .field("plan_fingerprint", &self.plan_fingerprint)
            .field("command", &self.display)
            .field("cwd", &self.cwd)
            .field("workspace_access", &self.workspace_access)
            .field("env", &env)
            .field("stdin_bytes", &self.stdin.as_ref().map(Vec::len))
            .field("network", &self.network)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

/// Run `request` through the context's launcher, mapping cancellation.
pub(crate) fn launch(
    ctx: &ToolContext,
    request: &LaunchRequest,
) -> Result<LaunchOutcome, ToolError> {
    crate::context::check_cancel(&ctx.cancel)?;
    let outcome = ctx.launcher()?.launch(request, &ctx.cancel)?;
    if ctx.cancel.is_stopped() {
        return Err(ToolError::Cancelled);
    }
    Ok(outcome)
}
