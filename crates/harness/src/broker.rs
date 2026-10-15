//! Native ownership of execution permits and the tool registry.
use crate::{extensions::Extensions, launcher::NativeLauncher, telemetry::ToolEventSink};
use aidex_core::{EntityId, WorkspacePath};
use aidex_events::{EventScope, EventSink, NullEventSink};
use aidex_native_sessions::{PermissionMode, SessionDetail};
use aidex_policy::{
    EvaluationContext, HostTrust, NetworkMode, PolicySet, PrivacyMode, Profile, ProjectRules,
    evaluate,
};
use aidex_sandbox::{Sandbox, SandboxLevel};
use aidex_tools::{
    ExecutionPermit, PermitIssuer, PlanFingerprint, ToolContext, ToolError, ToolId, ToolOutput,
    ToolPlan, ToolRegistry,
};
use aidex_workspace::{WorkspaceHandle, WorkspaceRegistry};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
pub(crate) struct Broker {
    pub tools: ToolRegistry,
    pub sandbox: Sandbox,
    pub workspaces: Arc<WorkspaceRegistry>,
    scratch: PathBuf,
    /// The offline switch, emergency stop, and drive policy settings that
    /// planned network access is evaluated under.
    extensions: Arc<dyn Extensions>,
}
/// Why a model's tool call could not be planned.
#[derive(Debug)]
pub(crate) enum PrepareError {
    /// The call is malformed in a way the model can correct (an unknown
    /// tool, invalid arguments, a missing file, a patch that does not
    /// apply). Nothing was started; the model receives the code and message.
    Repairable { code: String, message: String },
    /// Anything else (a policy denial, a path outside the workspace, an
    /// unavailable workspace) fails the run.
    Fatal(String),
}
impl std::fmt::Display for PrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Repairable { message, .. } | Self::Fatal(message) => f.write_str(message),
        }
    }
}
impl From<String> for PrepareError {
    fn from(message: String) -> Self {
        Self::Fatal(message)
    }
}
impl From<&str> for PrepareError {
    fn from(message: &str) -> Self {
        Self::Fatal(message.into())
    }
}
impl From<PrepareError> for String {
    fn from(error: PrepareError) -> Self {
        error.to_string()
    }
}
impl PrepareError {
    fn repairable(code: &str, message: &str) -> Self {
        Self::Repairable {
            code: code.into(),
            message: message.into(),
        }
    }
}
/// A planning failure of the tool itself. Errors about the call's own input
/// or the state of the workspace are repairable; paths outside the
/// workspace, forbidden or linked targets, and malformed paths are not.
fn planning_error(error: ToolError) -> PrepareError {
    let repairable = match &error {
        ToolError::InvalidInput { reason, .. } => *reason != aidex_tools::InputError::InvalidPath,
        ToolError::UnknownTool(_)
        | ToolError::NotFound { .. }
        | ToolError::WrongKind { .. }
        | ToolError::TooLarge { .. }
        | ToolError::Conflict { .. }
        | ToolError::PatchFailed { .. } => true,
        _ => false,
    };
    if repairable {
        PrepareError::Repairable {
            code: error.reason().into(),
            message: error.to_string(),
        }
    } else {
        PrepareError::Fatal(error.to_string())
    }
}
/// The policy profile a permission mode is evaluated under.
fn profile_for(mode: PermissionMode) -> Profile {
    if mode == PermissionMode::FullAccess {
        Profile::Autonomous
    } else {
        Profile::Balanced
    }
}
pub(crate) struct Prepared {
    pub context: ToolContext,
    pub plan: ToolPlan,
    pub needs_approval: bool,
    pub before: Vec<(String, Option<String>)>,
    policy_fingerprint: String,
    /// The drive settings a plan that reaches the network was evaluated
    /// under; `None` for other plans.
    network: Option<EvaluationContext>,
    /// The isolation the plan's processes run at.
    level: SandboxLevel,
}
impl Broker {
    pub(crate) fn policy_state(
        &self,
        root: Option<&Path>,
        workspace: Option<&EntityId>,
    ) -> Result<(PolicySet, String), String> {
        let dir = self.workspaces.paths().policies();
        let mut policies = PolicySet::load(&dir).map_err(|e| e.to_string())?;
        let mut files = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        files.retain(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("toml"))
        });
        files.sort();
        let mut data = Vec::new();
        for file in files {
            data.extend(
                file.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_bytes(),
            );
            data.extend(std::fs::read(file).map_err(|e| e.to_string())?);
        }
        if let (Some(root), Some(workspace)) = (root, workspace) {
            let project = root.join(aidex_policy::PROJECT_RULES_FILE);
            if project.exists() {
                policies.set_project_rules(
                    workspace.clone(),
                    ProjectRules::load(&project).map_err(|e| e.to_string())?,
                );
                data.extend(std::fs::read(project).map_err(|e| e.to_string())?);
            }
        }
        Ok((policies, aidex_native_sessions::digest(&data)))
    }
    pub fn authorize_native(
        &self,
        session: &str,
        config: &aidex_native_sessions::SessionConfig,
        action: aidex_policy::Action,
        locality: Option<aidex_policy::ModelLocality>,
    ) -> Result<(), String> {
        let workspace = config
            .workspace_id
            .as_ref()
            .map(EntityId::new)
            .transpose()
            .map_err(|e| e.to_string())?;
        let root = workspace
            .as_ref()
            .map(|id| self.workspaces.handle(id).map(|h| h.root().display_root()))
            .transpose()
            .map_err(|e| e.to_string())?;
        let (policies, _) = self.policy_state(root.as_deref(), workspace.as_ref())?;
        let context = EvaluationContext::new(
            profile_for(config.permission_mode),
            PrivacyMode::Hybrid,
            HostTrust::Trusted,
            NetworkMode::Unrestricted,
        );
        let mut intent = aidex_policy::Intent::new(
            action,
            EntityId::new(session).map_err(|e| e.to_string())?,
            "native execution action",
        );
        intent.workspace_id = workspace;
        intent.attributes.model_locality = locality;
        let decision = evaluate(&policies, &context, &intent);
        if decision.verdict.is_deny() {
            return Err(format!("policy denied: {}", decision.reason));
        }
        if decision.verdict.requires_approval() || decision.rate_limit.is_some() {
            return Err(
                "native model/child action requires a policy gate unavailable in v1".into(),
            );
        }
        Ok(())
    }
    pub fn new(
        workspaces: Arc<WorkspaceRegistry>,
        scratch: PathBuf,
        extensions: Arc<dyn Extensions>,
    ) -> Self {
        Self {
            tools: ToolRegistry::with_builtin_tools(),
            sandbox: Sandbox::locate(),
            workspaces,
            scratch,
            extensions,
        }
    }
    /// A process launcher for one plan, publishing its `process.*` events to
    /// `events`.
    fn launcher(
        &self,
        fingerprint: PlanFingerprint,
        level: SandboxLevel,
        events: Arc<dyn EventSink>,
        scope: EventScope,
    ) -> Arc<NativeLauncher> {
        Arc::new(NativeLauncher::new(
            self.sandbox.clone(),
            self.scratch.clone(),
            fingerprint,
            level,
            events,
            scope,
        ))
    }
    /// Plan a model's tool call under the session's permission mode and the
    /// drive and project policies. The plan's context publishes no events;
    /// [`execute_observed`](Self::execute_observed) attaches a sink when it
    /// runs.
    pub fn prepare(
        &self,
        s: &SessionDetail,
        call: &aidex_providers::ToolCall,
    ) -> Result<Prepared, PrepareError> {
        let workspace = s
            .configuration
            .workspace_id
            .as_ref()
            .ok_or("agent tools require a selected workspace")?;
        let handle = self
            .workspaces
            .handle(&EntityId::new(workspace).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let context = ToolContext::new(
            EntityId::new(&s.id).map_err(|e| e.to_string())?,
            handle.id().clone(),
            EntityId::generate("call").map_err(|e| e.to_string())?,
            Arc::new(handle.root().clone()),
            Arc::new(NullEventSink),
        );
        let tool = self.tools.resolve_model_name(&call.name).ok_or_else(|| {
            PrepareError::repairable(
                "unknown_tool",
                "unknown native tool; call one of the offered tools",
            )
        })?;
        let input: serde_json::Value = serde_json::from_str(&call.arguments).map_err(|_| {
            PrepareError::repairable("invalid_input", "tool arguments are not valid JSON")
        })?;
        // A repairable error is first checked against policy for the path it
        // is about ([`Self::guard_repairable`]): its text may tell what is
        // (or is not) in a file the model may not read.
        let guard = |error: PrepareError| self.guard_repairable(s, &handle, tool, &input, error);
        if tool.as_str() == "fs.delete" {
            if input
                .get("recursive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Err("native deletion never removes directories recursively".into());
            }
            let path = input.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                PrepareError::repairable("invalid_input", "delete path is required")
            })?;
            let absolute = handle
                .resolve(
                    &WorkspacePath::new(path).map_err(|e| e.to_string())?,
                    aidex_workspace::Access::Create,
                )
                .map_err(|e| e.to_string())?;
            match std::fs::symlink_metadata(&absolute) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(metadata) if metadata.is_dir() => {
                    return Err(guard(PrepareError::repairable(
                        "wrong_kind",
                        "native deletion requires a regular file; directories are refused",
                    )));
                }
                Ok(_) => {
                    return Err(
                        "native deletion requires a regular file; links and special files are \
                         refused"
                            .into(),
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Err(guard(PrepareError::Repairable {
                        code: "not_found".into(),
                        message: format!("`{path}` does not exist"),
                    }));
                }
                Err(error) => return Err(error.to_string().into()),
            }
        }
        let plan = self
            .tools
            .plan(&context, tool, input.clone())
            .map_err(|error| guard(planning_error(error)))?;
        let (policies, policy_fingerprint) =
            self.policy_state(Some(&handle.root().display_root()), Some(handle.id()))?;
        let profile = profile_for(s.configuration.permission_mode);
        // A plan that reaches the network (a push, a networked command) is
        // refused while offline or stopped, and its intents meet the drive's
        // privacy, host-trust, and network-mode floors; other plans keep the
        // permission-mode context.
        let network = if plan.facts().network.is_some() {
            Some(self.network_context(profile)?)
        } else {
            None
        };
        let evaluation = network.clone().unwrap_or_else(|| {
            EvaluationContext::new(
                profile,
                PrivacyMode::Hybrid,
                HostTrust::Trusted,
                NetworkMode::Unrestricted,
            )
        });
        let mut needs_approval = tool.as_str() == "fs.delete";
        let mut level = SandboxLevel::Brokered;
        for intent in plan.intents() {
            let d = evaluate(&policies, &evaluation, &intent);
            if d.verdict.is_deny() {
                return Err(PrepareError::Fatal(format!("policy denied: {}", d.reason)));
            }
            if d.rate_limit.is_some() {
                return Err("rate-limited tools unavailable in native v1".into());
            }
            needs_approval |= d.verdict.requires_approval();
            let candidate = match d.sandbox {
                aidex_policy::SandboxLevel::Direct | aidex_policy::SandboxLevel::Brokered => {
                    SandboxLevel::Brokered
                }
                aidex_policy::SandboxLevel::Restricted => SandboxLevel::Restricted,
                aidex_policy::SandboxLevel::Ephemeral => SandboxLevel::Ephemeral,
            };
            level = level.max(candidate);
        }
        let read_only = self
            .tools
            .descriptor(tool)
            .ok_or("unknown native tool")?
            .risk
            .is_read_only();
        let edits = matches!(tool.as_str(), "fs.write" | "fs.patch");
        needs_approval |= match s.configuration.permission_mode {
            PermissionMode::Supervised | PermissionMode::Auto => !read_only,
            PermissionMode::AutoAcceptEdits => !read_only && !edits,
            PermissionMode::FullAccess => false,
        };
        // Full access explicitly opts into brokered, same-user shell. Other modes
        // request filesystem confinement and fail closed on unsupported hosts.
        if tool.as_str() == "shell.exec"
            && s.configuration.permission_mode != PermissionMode::FullAccess
        {
            level = level.max(SandboxLevel::Restricted);
        }
        let launcher = self.launcher(
            plan.fingerprint(),
            level,
            Arc::new(NullEventSink),
            context.scope(),
        );
        let context = context.with_launcher(launcher);
        let mut before = vec![];
        for p in &plan.facts().paths {
            if p.access.is_write() {
                let path = WorkspacePath::new(&p.path).map_err(|e| e.to_string())?;
                let absolute = handle
                    .resolve(&path, aidex_workspace::Access::Create)
                    .map_err(|e| e.to_string())?;
                let prior = if absolute.exists() {
                    let bytes = std::fs::read(&absolute).map_err(|e| e.to_string())?;
                    if bytes.len() > 4 * 1024 * 1024 {
                        return Err("checkpoint file exceeds size bound".into());
                    }
                    Some(
                        String::from_utf8(bytes)
                            .map_err(|_| "checkpoint supports UTF-8 files only")?,
                    )
                } else {
                    None
                };
                before.push((p.path.clone(), prior));
            }
        }
        Ok(Prepared {
            context,
            plan,
            needs_approval,
            before,
            policy_fingerprint,
            network,
            level,
        })
    }
    /// `error`, unless it is repairable and about a path the model may not
    /// read. A planning error's text (a patch that does not apply, how often
    /// a string occurs, a missing file, the wrong kind of entry) tells what
    /// is, or is not, in the file, so it reaches the model only when policy
    /// lets the session read the call's `path` without an approval and does
    /// not deny the tool's own action there. Otherwise the call fails the
    /// run: with the policy's denial, or with a fixed text when reading needs
    /// an approval or is rate limited. Errors of calls without a `path`
    /// argument are kept.
    fn guard_repairable(
        &self,
        s: &SessionDetail,
        handle: &WorkspaceHandle,
        tool: &ToolId,
        input: &serde_json::Value,
        error: PrepareError,
    ) -> PrepareError {
        if !matches!(error, PrepareError::Repairable { .. }) {
            return error;
        }
        let Some(path) = input.get("path").and_then(serde_json::Value::as_str) else {
            return error;
        };
        match self.readable_path(s, handle, tool, path) {
            Ok(()) => error,
            Err(refusal) => PrepareError::Fatal(refusal),
        }
    }
    /// Whether policy lets the session read `path` without an approval, and
    /// does not deny `tool`'s own action there.
    fn readable_path(
        &self,
        s: &SessionDetail,
        handle: &WorkspaceHandle,
        tool: &ToolId,
        path: &str,
    ) -> Result<(), String> {
        const PROTECTED: &str = "the call could not be planned for a protected path";
        let scope = aidex_tools::path_scope(handle.root(), path).map_err(|_| PROTECTED)?;
        let action = self
            .tools
            .descriptor(tool)
            .map(|descriptor| descriptor.risk.action)
            .ok_or(PROTECTED)?;
        let (policies, _) =
            self.policy_state(Some(&handle.root().display_root()), Some(handle.id()))?;
        let context = EvaluationContext::new(
            profile_for(s.configuration.permission_mode),
            PrivacyMode::Hybrid,
            HostTrust::Trusted,
            NetworkMode::Unrestricted,
        );
        let session = EntityId::new(&s.id).map_err(|e| e.to_string())?;
        let decide = |action| {
            let mut intent =
                aidex_policy::Intent::new(action, session.clone(), "check a planning error")
                    .with_workspace(handle.id().clone())
                    .with_tool(tool.entity_id().clone());
            intent.attributes.paths = vec![scope.clone()];
            evaluate(&policies, &context, &intent)
        };
        let read = decide(aidex_policy::Action::FsRead);
        let own = decide(action);
        for decision in [&read, &own] {
            if decision.verdict.is_deny() {
                return Err(format!("policy denied: {}", decision.reason));
            }
        }
        if read.verdict.requires_approval() || read.rate_limit.is_some() {
            return Err(PROTECTED.into());
        }
        Ok(())
    }
    /// The context network-bearing plans are evaluated under: refused while
    /// the offline switch or an emergency stop is on, otherwise the drive's
    /// privacy, host-trust, and network-mode settings.
    fn network_context(&self, profile: Profile) -> Result<EvaluationContext, String> {
        let network = self.extensions.network();
        if let Some(reason) = network.blocked {
            return Err(format!(
                "policy denied: network access is unavailable ({reason})"
            ));
        }
        Ok(EvaluationContext::new(
            profile,
            network.context.privacy,
            network.context.host_trust,
            network.context.network_mode,
        ))
    }
    /// Run a prepared plan without publishing events (checkpoint restores,
    /// automatic prompt context).
    pub fn execute(&self, p: &Prepared, issuer: PermitIssuer) -> Result<ToolOutput, String> {
        self.execute_in(p, &p.context, issuer)
    }
    /// Run a prepared plan, publishing its `tool.*`, `file.changed`,
    /// `diff.created`, and `process.*` events to `events` as metadata only
    /// ([`ToolEventSink`]).
    pub fn execute_observed(
        &self,
        p: &Prepared,
        issuer: PermitIssuer,
        events: Arc<dyn EventSink>,
    ) -> Result<ToolOutput, String> {
        let events: Arc<dyn EventSink> = Arc::new(ToolEventSink::new(events));
        let mut context = p.context.clone();
        context.events = Arc::clone(&events);
        let scope = context.scope();
        let context =
            context.with_launcher(self.launcher(p.plan.fingerprint(), p.level, events, scope));
        self.execute_in(p, &context, issuer)
    }
    fn execute_in(
        &self,
        p: &Prepared,
        context: &ToolContext,
        issuer: PermitIssuer,
    ) -> Result<ToolOutput, String> {
        let (_, current) = self.policy_state(
            Some(&p.context.root.display_root()),
            Some(&p.context.workspace_id),
        )?;
        if current != p.policy_fingerprint {
            return Err("policy changed since tool planning; request a new plan".into());
        }
        // The offline switch, an emergency stop, or the drive's settings may
        // have changed while the plan waited for approval.
        if let Some(planned) = &p.network
            && self.network_context(planned.profile)? != *planned
        {
            return Err("policy changed since tool planning; request a new plan".into());
        }
        self.tools
            .execute(context, &p.plan, ExecutionPermit::for_plan(&p.plan, issuer))
            .map_err(|e| e.to_string())
    }
}
