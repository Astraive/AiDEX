//! Planning and evaluation: tool contexts, policy (with project rules), dry runs.

use std::{fs, io::ErrorKind, sync::Arc};

use aidex_core::{EntityId, WorkspacePath};
use aidex_events::{EventKind, EventScope, redact_text};
use aidex_policy::{ApprovalScope, PROJECT_RULES_FILE, PolicySet, ProjectRules, Verdict};
use aidex_tools::{ProcessLauncher, ToolContext, ToolPlan};
use aidex_workspace::{Access, ConfinedRoot};

use super::Broker;
use crate::{
    BrokerError, Explanation, ToolInvocation,
    approvals::{CardDiff, card},
    audit::events_verdict,
    intent::{self, PrepareContext, Prepared, targets},
    workspaces::ResolvedWorkspace,
};

pub(super) fn generate(prefix: &str) -> Result<EntityId, BrokerError> {
    EntityId::generate(prefix)
        .map_err(|error| BrokerError::internal("cannot generate an id", error))
}

pub(super) fn session_scope(invocation: &ToolInvocation) -> EventScope {
    EventScope::session(
        invocation.workspace_id.clone(),
        invocation.session_id.clone(),
    )
}

/// Load `<workspace>/.aidex/rules.toml` through the confined root. Repository content
/// is untrusted, so the rules can only tighten; a file that cannot be read safely or
/// parsed denies every call in the workspace (fail closed).
fn project_rules(root: &ConfinedRoot) -> Result<Option<ProjectRules>, String> {
    let path = WorkspacePath::new(PROJECT_RULES_FILE)
        .map_err(|_| format!("`{PROJECT_RULES_FILE}` is not a workspace path"))?;
    match root.resolve(&path, Access::Read) {
        Ok(absolute) => match fs::symlink_metadata(&absolute) {
            Ok(metadata) if metadata.is_file() => {
                ProjectRules::load(&absolute).map(Some).map_err(|error| {
                    format!("project rules `{PROJECT_RULES_FILE}` are invalid: {error}")
                })
            }
            Ok(_) => Err(format!("`{PROJECT_RULES_FILE}` is not a regular file")),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("`{PROJECT_RULES_FILE}` cannot be read: {error}")),
        },
        Err(error) if error.reason() == "path_not_found" => Ok(None),
        Err(error) => Err(format!(
            "`{PROJECT_RULES_FILE}` cannot be read safely: {error}"
        )),
    }
}

impl Broker {
    /// Dry run: plan the invocation and evaluate policy, without executing,
    /// creating an approval, or recording events.
    pub fn explain(&self, invocation: &ToolInvocation) -> Result<Explanation, BrokerError> {
        if self.registry.descriptor(&invocation.tool_id).is_none() {
            return Err(BrokerError::UnknownTool(invocation.tool_id.to_string()));
        }
        let workspace = self.workspaces.resolve(&invocation.workspace_id)?;
        let call_id = generate("call")?;
        let ctx = self.tool_context(invocation, &call_id, &workspace);
        let plan = self
            .registry
            .plan(&ctx, &invocation.tool_id, invocation.input.clone())
            .map_err(|error| BrokerError::Tool {
                code: error.code(),
                reason: error.reason(),
                message: redact_text(&error.to_string()).into_owned(),
            })?;
        let prepared = self.prepare(invocation, &workspace, plan)?;
        let session_grant = match &prepared.combined.verdict {
            Verdict::RequireApproval { scope_options, .. }
                if scope_options.contains(&ApprovalScope::Session) =>
            {
                self.store
                    .find_grant(
                        &invocation.session_id,
                        &invocation.workspace_id,
                        invocation.tool_id.as_str(),
                        &prepared.grant.key,
                        &self.clock.now(),
                    )?
                    .map(|grant| grant.approval_id)
            }
            _ => None,
        };
        let sandbox_unavailable = if prepared.combined.verdict.is_deny() {
            None
        } else {
            self.sandbox_unavailable(&prepared, &workspace)
        };
        Ok(Explanation {
            tool_id: prepared.plan.tool_id().to_string(),
            summary: redact_text(prepared.plan.summary()).into_owned(),
            effect: card::effect(&prepared, &workspace),
            risk: prepared.plan.risk(),
            decision: prepared.combined.clone(),
            intents: prepared.decisions.clone(),
            session_grant,
            sandbox_unavailable,
            targets: targets(&prepared.plan, prepared.remote.as_ref()),
            diff_preview: prepared.plan.preview().map(CardDiff::from_preview),
            warnings: prepared.warnings.clone(),
        })
    }

    pub(super) fn tool_context(
        &self,
        invocation: &ToolInvocation,
        call_id: &EntityId,
        workspace: &ResolvedWorkspace,
    ) -> ToolContext {
        let launcher: Arc<dyn ProcessLauncher> = self.launcher.clone();
        ToolContext::new(
            invocation.session_id.clone(),
            invocation.workspace_id.clone(),
            call_id.clone(),
            Arc::new(workspace.root.clone()),
            Arc::clone(self.audit.sink()),
        )
        .with_limits(self.config.tool_limits.clone())
        .with_launcher(launcher)
    }

    /// Enrich and evaluate a plan under the current policy, project rules included.
    pub(super) fn prepare(
        &self,
        invocation: &ToolInvocation,
        workspace: &ResolvedWorkspace,
        plan: ToolPlan,
    ) -> Result<Prepared, BrokerError> {
        let (documents, settings) = {
            let state = self.policy_state();
            (state.documents.clone(), state.settings)
        };
        let mut failure = None;
        let base = documents.unwrap_or_else(|error| {
            failure = Some((
                "broker:policy:documents",
                format!("policy documents failed to load, so every call is denied: {error}"),
            ));
            Arc::new(PolicySet::new())
        });
        let policies = match project_rules(&workspace.root) {
            Ok(None) => base,
            Ok(Some(rules)) => {
                let mut set = (*base).clone();
                set.set_project_rules(workspace.id.clone(), rules);
                Arc::new(set)
            }
            Err(message) => {
                failure.get_or_insert(("broker:project:rules", message));
                base
            }
        };
        let evaluation = settings.context();
        let mut prepared = intent::prepare(
            plan,
            &PrepareContext {
                invocation,
                workspace,
                protected: &self.config.protected_paths,
                policy_dirs: &self.config.policy_paths,
                secrets: &self.secrets,
                policies: &policies,
                evaluation: &evaluation,
            },
        )?;
        if let Some((rule, reason)) = failure {
            prepared.combined.verdict = Verdict::Deny;
            prepared.combined.rule = rule.to_owned();
            prepared.combined.reason = reason;
        }
        Ok(prepared)
    }

    pub(super) fn record_decisions(
        &self,
        scope: &EventScope,
        prepared: &Prepared,
    ) -> Result<(), BrokerError> {
        for entry in &prepared.decisions {
            self.audit.before(
                scope,
                EventKind::PolicyDecision {
                    intent_id: entry.intent_id.clone(),
                    decision: events_verdict(&entry.decision.verdict),
                    rule: entry.decision.rule.to_string(),
                },
            )?;
        }
        Ok(())
    }
}
