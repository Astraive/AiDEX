//! From a sealed tool plan to evaluated policy intents.
//!
//! Intents are built only from facts the tool computed and the broker verified
//! (confined paths, classified commands, the repository's configured remote), never
//! from agent claims. The broker adds what the tool cannot know: protected path
//! classes, policy-directory writes (`policy.change`), remote origins
//! (`remote.action`), the push destination, and stored credentials (`secret.use`).

use std::path::{Path, PathBuf};

use aidex_core::EntityId;
use aidex_policy::{
    Action, Decision, EvaluationContext, Intent, PathClass, PolicySet, RateLimit, RuleOrigin,
    RuleRef, SandboxLevel, SecretUse, Verdict, evaluate,
};
use aidex_tools::ToolPlan;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    BrokerError, Origin, ToolInvocation,
    digest::digest,
    git_remote::{self, GitRemote},
    secrets::{CredentialUse, Secrets},
    workspaces::ResolvedWorkspace,
};

/// Actions an agent-side origin may never perform, whatever the policy says.
const LOCAL_USER_ONLY: [Action; 5] = [
    Action::PolicyChange,
    Action::BrokerDisable,
    Action::VaultModify,
    Action::CredentialExport,
    Action::SecretDump,
];

/// The policy decision for one evaluated intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntentDecision {
    /// Audit id of this intent (`int_…`), referenced by `policy.decision` and
    /// `approval.created` events.
    pub intent_id: EntityId,
    pub action: Action,
    pub summary: String,
    /// `aidex_policy::Intent::fingerprint_hex` of the evaluated intent.
    pub fingerprint: String,
    pub decision: Decision,
}

/// Every intent of a call combined: the strictest verdict wins, the deepest sandbox
/// applies, approval scopes intersect, and the shortest expiry holds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CombinedDecision {
    pub verdict: Verdict,
    /// Minimum isolation for the call, including after approval.
    pub sandbox: SandboxLevel,
    pub rate_limit: Option<RateLimit>,
    /// The deciding rule (`origin:scope:id`).
    pub rule: String,
    pub reason: String,
    pub effect_summary: String,
}

/// What "approve for session" would allow, keyed for exact matching.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GrantPattern {
    pub key: String,
    pub description: String,
}

/// A plan with everything the broker derived and decided for it.
pub(crate) struct Prepared {
    pub plan: ToolPlan,
    pub decisions: Vec<IntentDecision>,
    pub combined: CombinedDecision,
    /// Binding fingerprint an approval must reproduce on continuation.
    pub fingerprint: String,
    pub grant: GrantPattern,
    pub credentials: Vec<CredentialUse>,
    pub remote: Option<GitRemote>,
    pub warnings: Vec<String>,
}

impl Prepared {
    pub fn primary(&self) -> &IntentDecision {
        &self.decisions[0]
    }
}

/// Inputs to [`prepare`] besides the plan.
pub(crate) struct PrepareContext<'a> {
    pub invocation: &'a ToolInvocation,
    pub workspace: &'a ResolvedWorkspace,
    pub protected: &'a [PathBuf],
    pub policy_dirs: &'a [PathBuf],
    pub secrets: &'a Secrets,
    pub policies: &'a PolicySet,
    pub evaluation: &'a EvaluationContext,
}

/// Normalize a path for prefix comparison: no Windows verbatim prefix, `/`
/// separators, case-folded on Windows.
pub(crate) fn normalized(path: &Path) -> String {
    let text = path.display().to_string();
    let text = text
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned());
    let text = text.replace('\\', "/");
    let text = text.trim_end_matches('/').to_owned();
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

fn within_any(path: &str, roots: &[PathBuf]) -> bool {
    let path = normalized(Path::new(path));
    roots.iter().any(|root| {
        let root = normalized(root);
        !root.is_empty()
            && (path == root
                || path
                    .strip_prefix(&root)
                    .is_some_and(|rest| rest.starts_with('/')))
    })
}

/// Input fields that carry content rather than targets; a session grant covers any
/// value of them (for example new file content for the same path).
fn content_fields(tool: &str) -> &'static [&'static str] {
    match tool {
        "fs.write" => &["content", "expected_sha256"],
        "fs.patch" => &["edits", "unified_diff", "expected_sha256"],
        "git.commit" => &["message"],
        _ => &[],
    }
}

fn with_origin(mut intent: Intent, agent: Option<&EntityId>, origin: &Origin) -> Intent {
    if let Some(agent) = agent {
        intent.agent_id = Some(agent.clone());
    }
    if let Origin::Skill { skill_id } = origin {
        intent.skill_id = Some(skill_id.clone());
    }
    intent
}

/// Build, enrich, and evaluate every intent of `plan`.
pub(crate) fn prepare(plan: ToolPlan, ctx: &PrepareContext<'_>) -> Result<Prepared, BrokerError> {
    let invocation = ctx.invocation;
    let origin = &invocation.origin;
    let agent = invocation.agent_id.clone().or_else(|| match origin {
        Origin::Adapter { adapter_id } => Some(adapter_id.clone()),
        _ => None,
    });
    let mut intents: Vec<Intent> = plan
        .intents()
        .into_iter()
        .map(|intent| with_origin(intent, agent.as_ref(), origin))
        .collect();
    for intent in &mut intents {
        for path in &mut intent.attributes.paths {
            if within_any(&path.path, ctx.protected) {
                path.class = PathClass::Protected;
            }
        }
    }
    let primary = intents[0].clone();
    let writes_policy = plan
        .facts()
        .write_paths()
        .any(|path| within_any(&path.display, ctx.policy_dirs));
    if writes_policy
        && !intents
            .iter()
            .any(|intent| intent.action == Action::PolicyChange)
    {
        let mut implied = primary.clone();
        implied.action = Action::PolicyChange;
        intents.push(implied);
    }
    if matches!(origin, Origin::Remote { .. }) {
        let mut implied = primary.clone();
        implied.action = Action::RemoteAction;
        intents.push(implied);
    }

    let mut warnings = Vec::new();
    let mut credentials = Vec::new();
    let mut remote = None;
    if plan.action() == Action::GitPush {
        let name = plan
            .input()
            .get("remote")
            .and_then(Value::as_str)
            .unwrap_or("origin");
        remote = git_remote::resolve(&ctx.workspace.root, name);
        match &remote {
            Some(found) => {
                if let Some(target) = found.network_target() {
                    for intent in intents
                        .iter_mut()
                        .filter(|intent| intent.action == Action::GitPush)
                    {
                        intent.attributes.network = Some(target.clone());
                    }
                }
                if !found.exact {
                    warnings.push(
                        "git configuration (insteadOf, includes, or several URLs) may change where \
                         this pushes; the destination is treated as unknown"
                            .to_owned(),
                    );
                }
                if found.embeds_credentials {
                    warnings.push(
                        "the remote URL embeds user info that may be a credential".to_owned(),
                    );
                }
                if let Some(credential) = ctx.secrets.git_credential(found) {
                    let summary = format!(
                        "use secret `{}` for {}",
                        credential.secret_name.as_str(),
                        credential.consumer
                    );
                    let mut secret = Intent::new(
                        Action::SecretUse,
                        invocation.session_id.clone(),
                        summary,
                    )
                    .with_workspace(invocation.workspace_id.clone())
                    .with_tool(plan.tool_id().entity_id().clone())
                    .with_secret(SecretUse::new(
                        credential.secret_name.as_str(),
                        credential.consumer.to_string(),
                    ));
                    secret = with_origin(secret, agent.as_ref(), origin);
                    intents.push(secret);
                    credentials.push(credential);
                }
            }
            None => warnings.push(format!(
                "remote `{name}` is not configured in this repository's .git/config; the destination is unknown"
            )),
        }
    }

    let mut decisions = Vec::with_capacity(intents.len());
    for intent in &intents {
        let mut decision = evaluate(ctx.policies, ctx.evaluation, intent);
        if !origin.is_local_user() && LOCAL_USER_ONLY.contains(&intent.action) {
            decision = local_user_only(intent.action, origin, decision);
        }
        decisions.push(IntentDecision {
            intent_id: EntityId::generate("int")
                .map_err(|error| BrokerError::internal("cannot generate an intent id", error))?,
            action: intent.action,
            summary: intent.summary.clone(),
            fingerprint: intent.fingerprint_hex(),
            decision,
        });
    }
    let combined = combine(&decisions);
    let fingerprint = binding(&plan, &decisions, origin, agent.as_ref())?;
    let grant = grant_pattern(&plan, &intents, origin, ctx.workspace)?;
    Ok(Prepared {
        plan,
        decisions,
        combined,
        fingerprint,
        grant,
        credentials,
        remote,
        warnings,
    })
}

fn local_user_only(action: Action, origin: &Origin, decision: Decision) -> Decision {
    Decision {
        verdict: Verdict::Deny,
        reason: format!(
            "{action} may only be requested by the local user, not by {origin} (broker hard rule)"
        ),
        rule: RuleRef {
            origin: RuleOrigin::Hard,
            scope: "broker".into(),
            id: "broker.local-user-only".into(),
            source: None,
        },
        ..decision
    }
}

/// The approval binding: the plan minus its random ids, every evaluated intent's
/// fingerprint, and who asked.
fn binding(
    plan: &ToolPlan,
    decisions: &[IntentDecision],
    origin: &Origin,
    agent: Option<&EntityId>,
) -> Result<String, BrokerError> {
    let mut plan_value = serde_json::to_value(plan)
        .map_err(|error| BrokerError::internal("the plan is not serializable", error))?;
    if let Value::Object(map) = &mut plan_value {
        for random in ["plan_id", "call_id", "fingerprint"] {
            map.remove(random);
        }
    }
    let intents: Vec<&str> = decisions
        .iter()
        .map(|decision| decision.fingerprint.as_str())
        .collect();
    Ok(digest(
        "aidex-broker.intent.v1",
        &json!({ "plan": plan_value, "intents": intents, "origin": origin, "agent": agent }),
    ))
}

/// The session-grant key: tool, workspace, session, origin, the input without
/// content fields, and every intent's action and attributes (paths, command,
/// destination, secret). Free-text summaries are excluded.
fn grant_pattern(
    plan: &ToolPlan,
    intents: &[Intent],
    origin: &Origin,
    workspace: &ResolvedWorkspace,
) -> Result<GrantPattern, BrokerError> {
    let tool = plan.tool_id().as_str();
    let mut input = plan.input().clone();
    if let Value::Object(map) = &mut input {
        for field in content_fields(tool) {
            map.remove(*field);
        }
    }
    let mut units = Vec::with_capacity(intents.len());
    for intent in intents {
        let attributes = serde_json::to_value(&intent.attributes)
            .map_err(|error| BrokerError::internal("intent is not serializable", error))?;
        units.push(json!({
            "action": intent.action,
            "agent": intent.agent_id,
            "skill": intent.skill_id,
            "attributes": attributes,
        }));
    }
    let key = digest(
        "aidex-broker.grant.v1",
        &json!({
            "tool": tool,
            "version": plan.tool_version(),
            "workspace": plan.workspace_id(),
            "session": plan.session_id(),
            "origin": origin,
            "input": input,
            "units": units,
        }),
    );
    let targets = targets(plan, None);
    let what = if targets.is_empty() {
        "the same input".to_owned()
    } else {
        targets.join(", ")
    };
    let content = if content_fields(tool).is_empty() {
        ""
    } else {
        " (any content)"
    };
    Ok(GrantPattern {
        key,
        description: format!(
            "later `{tool}` calls in this session on {what}{content} in workspace {}",
            workspace.name
        ),
    })
}

/// Files, destinations, and remotes a plan touches, for cards and grants.
pub(crate) fn targets(plan: &ToolPlan, remote: Option<&GitRemote>) -> Vec<String> {
    let mut targets: Vec<String> = Vec::new();
    let mut push = |target: String| {
        if !targets.contains(&target) {
            targets.push(target);
        }
    };
    for path in &plan.facts().paths {
        if path.path.is_empty() {
            push("workspace root".to_owned());
        } else {
            push(path.path.clone());
        }
    }
    if let Some(network) = &plan.facts().network {
        for target in &network.targets {
            push(format!("{}:{}", target.host(), target.port()));
        }
        if remote.is_none() {
            for unresolved in &network.unresolved {
                push(unresolved.clone());
            }
        }
    }
    if let Some(remote) = remote {
        push(format!("remote `{}` ({})", remote.name, remote.display));
    }
    targets
}

/// Combine per-intent decisions by strictness.
pub(crate) fn combine(decisions: &[IntentDecision]) -> CombinedDecision {
    let mut strictest = &decisions[0].decision;
    for entry in &decisions[1..] {
        if entry.decision.verdict.rank() > strictest.verdict.rank() {
            strictest = &entry.decision;
        }
    }
    let sandbox = decisions
        .iter()
        .map(|entry| entry.decision.sandbox)
        .max()
        .unwrap_or(SandboxLevel::Brokered);
    let verdict = match &strictest.verdict {
        Verdict::RequireApproval { .. } => {
            let mut scopes: Option<Vec<_>> = None;
            let mut expiry = None;
            for entry in decisions {
                if let Verdict::RequireApproval {
                    scope_options,
                    expires_in,
                } = &entry.decision.verdict
                {
                    scopes = Some(match scopes {
                        None => scope_options.clone(),
                        Some(current) => current
                            .into_iter()
                            .filter(|scope| scope_options.contains(scope))
                            .collect(),
                    });
                    expiry = Some(expiry.map_or(*expires_in, |current: std::time::Duration| {
                        current.min(*expires_in)
                    }));
                }
            }
            let mut scope_options = scopes.unwrap_or_default();
            if scope_options.is_empty() {
                scope_options.push(aidex_policy::ApprovalScope::Once);
            }
            Verdict::RequireApproval {
                scope_options,
                expires_in: expiry.unwrap_or(aidex_policy::DEFAULT_APPROVAL_EXPIRY),
            }
        }
        Verdict::Sandbox { .. } => Verdict::Sandbox { level: sandbox },
        other => other.clone(),
    };
    let rate_limit = decisions
        .iter()
        .filter_map(|entry| entry.decision.rate_limit)
        .reduce(|a, b| {
            if u128::from(b.max) * a.per.as_millis() < u128::from(a.max) * b.per.as_millis() {
                b
            } else {
                a
            }
        });
    CombinedDecision {
        verdict,
        sandbox,
        rate_limit,
        rule: strictest.rule.to_string(),
        reason: strictest.reason.clone(),
        effect_summary: decisions[0].decision.effect_summary.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_roots_match_whole_components_only() {
        let roots = [PathBuf::from("/drive/private_vault")];
        assert!(within_any("/drive/private_vault", &roots));
        assert!(within_any("/drive/private_vault/vault.json", &roots));
        assert!(!within_any("/drive/private_vault2/x", &roots));
        assert!(!within_any("/drive/other", &roots));
        #[cfg(windows)]
        assert_eq!(normalized(Path::new(r"\\?\C:\Drive\x\")), "c:/drive/x");
    }
}
