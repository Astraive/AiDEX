//! Building human-readable approval cards (spec §14) from prepared plans.

use aidex_core::{EntityId, Timestamp};
use aidex_events::redact_text;
use aidex_policy::{ApprovalScope, redact_command};
use aidex_tools::DiffKind;
use serde_json::Value;

use crate::{
    ToolInvocation,
    approvals::{ApprovalCard, CardDiff, Requester, SecretNotice},
    intent::{Prepared, targets},
    workspaces::ResolvedWorkspace,
};

/// What the action will do, in one sentence.
pub(crate) fn effect(prepared: &Prepared, workspace: &ResolvedWorkspace) -> String {
    let plan = &prepared.plan;
    let input = plan.input();
    let text = |field: &str| input.get(field).and_then(Value::as_str).unwrap_or_default();
    let name = &workspace.name;
    let effect = match plan.tool_id().as_str() {
        "git.push" => {
            let destination = prepared
                .remote
                .as_ref()
                .map(|remote| format!(" ({})", remote.display))
                .unwrap_or_default();
            format!(
                "Uploads local commits on branch {} to remote {}{destination} from workspace {name}",
                text("branch"),
                text("remote"),
            )
        }
        "git.commit" => {
            let paths = input
                .get("paths")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("Commits {paths} path(s) to the local repository of workspace {name}")
        }
        "shell.exec" => match plan.facts().command.as_ref() {
            Some(command) => format!(
                "Runs `{}` in workspace {name} (classified {} risk)",
                command.redacted_line(),
                command.classification().risk
            ),
            None => format!("Runs a command in workspace {name}"),
        },
        _ => match plan.preview() {
            Some(preview) if !preview.files.is_empty() => {
                let count = preview.files.len() as u64 + preview.files_omitted;
                let all = |kind: DiffKind| preview.files.iter().all(|file| file.kind == kind);
                let verb = if all(DiffKind::Created) {
                    "Creates"
                } else if all(DiffKind::Deleted) {
                    "Deletes"
                } else {
                    "Writes"
                };
                let noun = if count == 1 {
                    preview.files[0].path.as_str().to_owned()
                } else {
                    format!("{count} files")
                };
                format!(
                    "{verb} {noun} in workspace {name}: +{} −{}",
                    preview.additions(),
                    preview.deletions()
                )
            }
            _ => prepared.combined.effect_summary.clone(),
        },
    };
    redact_text(&effect).into_owned()
}

/// Assemble the card for a pending approval.
pub(crate) fn build(
    approval_id: &EntityId,
    invocation: &ToolInvocation,
    prepared: &Prepared,
    workspace: &ResolvedWorkspace,
    scope_options: &[ApprovalScope],
    created_at: Timestamp,
    expires_at: Timestamp,
) -> ApprovalCard {
    let plan = &prepared.plan;
    let session_grant = scope_options
        .contains(&ApprovalScope::Session)
        .then(|| prepared.grant.description.clone());
    ApprovalCard {
        approval_id: approval_id.clone(),
        requester: Requester {
            origin: invocation.origin.clone(),
            agent_id: invocation.agent_id.clone(),
        },
        tool_id: plan.tool_id().to_string(),
        action: plan.action(),
        summary: redact_text(plan.summary()).into_owned(),
        effect: effect(prepared, workspace),
        workspace_id: workspace.id.clone(),
        workspace_name: workspace.name.clone(),
        session_id: invocation.session_id.clone(),
        risk: plan.risk(),
        scope_options: scope_options.to_vec(),
        created_at,
        expires_at,
        rule: prepared.combined.rule.clone(),
        reason: redact_text(&prepared.combined.reason).into_owned(),
        sandbox: prepared.combined.sandbox,
        targets: targets(plan, prepared.remote.as_ref())
            .into_iter()
            .map(|target| redact_text(&target).into_owned())
            .collect(),
        command: plan
            .facts()
            .command
            .as_ref()
            .map(|command| redact_text(&redact_command(command.line())).into_owned()),
        secrets: prepared
            .credentials
            .iter()
            .map(|credential| SecretNotice {
                secret_name: credential.secret_name.as_str().to_owned(),
                consumer: credential.consumer.to_string(),
            })
            .collect(),
        diff_preview: plan.preview().map(CardDiff::from_preview),
        session_grant,
        warnings: prepared.warnings.clone(),
    }
}
