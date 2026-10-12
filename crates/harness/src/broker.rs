//! Native ownership of execution permits and the tool registry.
use crate::{extensions::Extensions, launcher::NativeLauncher};
use aidex_core::{EntityId, WorkspacePath};
use aidex_events::NullEventSink;
use aidex_native_sessions::{PermissionMode, SessionDetail};
use aidex_policy::{
    EvaluationContext, HostTrust, NetworkMode, PolicySet, PrivacyMode, Profile, ProjectRules,
    evaluate,
};
use aidex_sandbox::{Sandbox, SandboxLevel};
use aidex_tools::{ExecutionPermit, PermitIssuer, ToolContext, ToolOutput, ToolPlan, ToolRegistry};
use aidex_workspace::WorkspaceRegistry;
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
pub(crate) struct Prepared {
    pub context: ToolContext,
    pub plan: ToolPlan,
    pub needs_approval: bool,
    pub before: Vec<(String, Option<String>)>,
    policy_fingerprint: String,
    /// The drive settings a plan that reaches the network was evaluated
    /// under; `None` for other plans.
    network: Option<EvaluationContext>,
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
        let profile = if config.permission_mode == PermissionMode::FullAccess {
            Profile::Autonomous
        } else {
            Profile::Balanced
        };
        let context = EvaluationContext::new(
            profile,
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
    pub fn prepare(
        &self,
        s: &SessionDetail,
        call: &aidex_providers::ToolCall,
    ) -> Result<Prepared, String> {
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
        let tool = self
            .tools
            .resolve_model_name(&call.name)
            .ok_or("unknown native tool")?;
        let input: serde_json::Value =
            serde_json::from_str(&call.arguments).map_err(|_| "invalid tool arguments JSON")?;
        if tool.as_str() == "fs.delete" {
            if input
                .get("recursive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                return Err("native deletion never removes directories recursively".into());
            }
            let path = input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or("delete path is required")?;
            let absolute = handle
                .resolve(
                    &WorkspacePath::new(path).map_err(|e| e.to_string())?,
                    aidex_workspace::Access::Create,
                )
                .map_err(|e| e.to_string())?;
            if !std::fs::symlink_metadata(&absolute)
                .map_err(|e| e.to_string())?
                .is_file()
            {
                return Err(
                    "native deletion requires a regular file; directories and links are refused"
                        .into(),
                );
            }
        }
        let plan = self
            .tools
            .plan(&context, tool, input)
            .map_err(|e| e.to_string())?;
        let (policies, policy_fingerprint) =
            self.policy_state(Some(&handle.root().display_root()), Some(handle.id()))?;
        let profile = if s.configuration.permission_mode == PermissionMode::FullAccess {
            Profile::Autonomous
        } else {
            Profile::Balanced
        };
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
                return Err(format!("policy denied: {}", d.reason));
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
        let launcher = Arc::new(NativeLauncher::new(
            self.sandbox.clone(),
            self.scratch.clone(),
            plan.fingerprint(),
            level,
        ));
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
        })
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
    pub fn execute(&self, p: &Prepared, issuer: PermitIssuer) -> Result<ToolOutput, String> {
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
            .execute(
                &p.context,
                &p.plan,
                ExecutionPermit::for_plan(&p.plan, issuer),
            )
            .map_err(|e| e.to_string())
    }
}
