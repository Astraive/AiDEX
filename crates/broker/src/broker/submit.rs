//! Submission and continuation: plan → decide → (approve) → execute.

use std::time::Duration;

use aidex_core::{EntityId, Timestamp};
use aidex_events::{EventKind, EventScope};
use aidex_policy::{ApprovalScope, Verdict};
use aidex_tools::PermitIssuer;

use super::{
    Broker, PendingCall,
    prepare::{generate, session_scope},
};
use crate::{
    ApprovalRefusal, BrokerError, BrokerOutcome, ToolInvocation,
    approvals::{ApprovalState, NewApproval, card},
    audit::events_scope,
    intent::Prepared,
    workspaces::ResolvedWorkspace,
};

/// What the policy offered for an approval.
struct ApprovalRequest {
    scope_options: Vec<ApprovalScope>,
    expires_in: Duration,
}

impl Broker {
    /// Submit a tool invocation (spec §13).
    ///
    /// Plans it (validated input, confined paths, computed facts, no side effects),
    /// builds policy intents from the plan's facts, evaluates them, and records
    /// `tool.requested` plus one `policy.decision` per intent. Then:
    /// allowed (or sandboxed) calls execute under a permit bound to the plan's
    /// fingerprint, at the decision's sandbox level or not at all; calls needing
    /// approval are matched against session grants or become a pending approval
    /// with a card; denied or rate-limited calls are recorded as `tool.denied` and
    /// never run.
    pub fn submit(&self, invocation: ToolInvocation) -> Result<BrokerOutcome, BrokerError> {
        let call_id = match &invocation.call_id {
            Some(id) => id.clone(),
            None => generate("call")?,
        };
        let scope = session_scope(&invocation);
        if let Err(stopped) = self.emergency.check() {
            self.audit.note(
                &scope,
                EventKind::ToolDenied {
                    call_id,
                    reason: "an emergency stop is in effect".into(),
                },
            );
            return Err(stopped);
        }
        let descriptor = self
            .registry
            .descriptor(&invocation.tool_id)
            .ok_or_else(|| BrokerError::UnknownTool(invocation.tool_id.to_string()))?;
        let base_risk = descriptor.risk.base_risk;
        let workspace = self.workspaces.resolve(&invocation.workspace_id)?;
        let ctx = self.tool_context(&invocation, &call_id, &workspace);
        let plan = match self
            .registry
            .plan(&ctx, &invocation.tool_id, invocation.input.clone())
        {
            Ok(plan) => plan,
            Err(error) => {
                return self.plan_failed(&scope, &invocation, call_id, base_risk, &error, true);
            }
        };
        let prepared = self.prepare(&invocation, &workspace, plan)?;
        self.audit.before(
            &scope,
            EventKind::ToolRequested {
                call_id: call_id.clone(),
                tool_id: invocation.tool_id.entity_id().clone(),
                summary: prepared.plan.summary().to_owned(),
                risk: prepared.plan.risk(),
            },
        )?;
        self.record_decisions(&scope, &prepared)?;
        match prepared.combined.verdict.clone() {
            Verdict::Deny => {
                let reason = prepared.combined.reason.clone();
                let rule = prepared.combined.rule.clone();
                self.deny(&scope, call_id, reason, rule)
            }
            Verdict::RequireApproval {
                scope_options,
                expires_in,
            } => {
                if scope_options.contains(&ApprovalScope::Session) {
                    let now = self.clock.now();
                    if let Some(grant) = self.store.find_grant(
                        &invocation.session_id,
                        &invocation.workspace_id,
                        invocation.tool_id.as_str(),
                        &prepared.grant.key,
                        &now,
                    )? {
                        self.audit.before(
                            &scope,
                            EventKind::PolicyDecision {
                                intent_id: prepared.primary().intent_id.clone(),
                                decision: aidex_events::Verdict::Allow,
                                rule: format!("session_grant:{}", grant.approval_id),
                            },
                        )?;
                        let issuer = PermitIssuer::Approval {
                            approval_id: grant.approval_id,
                        };
                        return self.execute(&ctx, &workspace, prepared, issuer, &scope);
                    }
                }
                // Never ask a human to approve something this host cannot contain.
                if let Some(reason) = self.sandbox_unavailable(&prepared, &workspace) {
                    return self.sandbox_denied(&scope, call_id, &prepared, reason);
                }
                let request = ApprovalRequest {
                    scope_options,
                    expires_in,
                };
                self.request_approval(invocation, call_id, &workspace, prepared, request, &scope)
            }
            Verdict::Allow | Verdict::Sandbox { .. } | Verdict::RateLimit { .. } => {
                let issuer = PermitIssuer::Policy {
                    rule: prepared.combined.rule.clone(),
                };
                self.execute(&ctx, &workspace, prepared, issuer, &scope)
            }
        }
    }

    /// Execute a call whose approval was granted.
    ///
    /// Re-plans the stored invocation and refuses unless the result reproduces the
    /// approved intent fingerprint (the target files, command, destination, and
    /// input are unchanged), the approval is `approved` and unused, and no
    /// emergency stop is in effect. Policy is evaluated again: a call the current
    /// policy denies is denied. Approvals are single-use: a second continuation
    /// fails with [`ApprovalRefusal::AlreadyConsumed`].
    pub fn continue_after_approval(
        &self,
        approval_id: &EntityId,
    ) -> Result<BrokerOutcome, BrokerError> {
        self.emergency.check()?;
        let now = self.clock.now();
        let record = self.store.get(approval_id, &now)?.ok_or_else(|| {
            BrokerError::approval(approval_id, ApprovalRefusal::NotFound, "no such approval")
        })?;
        let refusal = match &record.state {
            ApprovalState::Approved { .. } => None,
            ApprovalState::Pending => Some(ApprovalRefusal::Pending),
            ApprovalState::Denied { .. } => Some(ApprovalRefusal::Denied),
            ApprovalState::Expired { .. } => Some(ApprovalRefusal::Expired),
            ApprovalState::Revoked { .. } => Some(ApprovalRefusal::Revoked),
            ApprovalState::Cancelled { .. } => Some(ApprovalRefusal::Cancelled),
        };
        if let Some(refusal) = refusal {
            return Err(BrokerError::approval(
                approval_id,
                refusal,
                format!("the approval is {}, not approved", record.status()),
            ));
        }
        if record.consumed_at.is_some() {
            return Err(BrokerError::approval(
                approval_id,
                ApprovalRefusal::AlreadyConsumed,
                "the approved call was already executed",
            ));
        }
        let pending = self
            .pending()
            .get(approval_id)
            .map(|call| (call.invocation.clone(), call.call_id.clone()));
        let Some((invocation, call_id)) = pending else {
            return Err(BrokerError::approval(
                approval_id,
                ApprovalRefusal::InvocationLost,
                "this broker no longer holds the approved invocation (it restarted); submit it again",
            ));
        };
        let scope = session_scope(&invocation);
        let workspace = self.workspaces.resolve(&invocation.workspace_id)?;
        let ctx = self.tool_context(&invocation, &call_id, &workspace);
        let plan = match self
            .registry
            .plan(&ctx, &invocation.tool_id, invocation.input.clone())
        {
            Ok(plan) => plan,
            Err(error) => {
                self.pending().remove(approval_id);
                let risk = record.card.risk;
                return self.plan_failed(&scope, &invocation, call_id, risk, &error, false);
            }
        };
        let prepared = self.prepare(&invocation, &workspace, plan)?;
        self.record_decisions(&scope, &prepared)?;
        if prepared.fingerprint != record.intent_fingerprint {
            self.audit.note(
                &scope,
                EventKind::ToolDenied {
                    call_id,
                    reason: format!("the intent changed after approval {approval_id}"),
                },
            );
            return Err(BrokerError::approval(
                approval_id,
                ApprovalRefusal::IntentChanged,
                "re-planning produced a different intent than the one approved (the targets, \
                 command, destination, or workspace state changed); submit it again for a new approval",
            ));
        }
        if prepared.combined.verdict.is_deny() {
            self.pending().remove(approval_id);
            let reason = prepared.combined.reason.clone();
            let rule = prepared.combined.rule.clone();
            return self.deny(&scope, call_id, reason, rule);
        }
        if !self.store.consume(approval_id, &now)? {
            return Err(BrokerError::approval(
                approval_id,
                ApprovalRefusal::AlreadyConsumed,
                "the approved call was already executed (or the approval was revoked)",
            ));
        }
        self.pending().remove(approval_id);
        let issuer = PermitIssuer::Approval {
            approval_id: approval_id.clone(),
        };
        self.execute(&ctx, &workspace, prepared, issuer, &scope)
    }

    fn request_approval(
        &self,
        invocation: ToolInvocation,
        call_id: EntityId,
        workspace: &ResolvedWorkspace,
        prepared: Prepared,
        request: ApprovalRequest,
        scope: &EventScope,
    ) -> Result<BrokerOutcome, BrokerError> {
        let ApprovalRequest {
            scope_options,
            expires_in,
        } = request;
        let approval_id = generate("apr")?;
        let created_at = self.clock.now();
        let expires_ms = created_at
            .unix_millis()
            .saturating_add(i64::try_from(expires_in.as_millis()).unwrap_or(i64::MAX));
        let expires_at = Timestamp::from_unix_millis(expires_ms)
            .map_err(|error| BrokerError::internal("approval expiry out of range", error))?;
        let card = card::build(
            &approval_id,
            &invocation,
            &prepared,
            workspace,
            &scope_options,
            created_at.clone(),
            expires_at.clone(),
        );
        let grant_key = scope_options
            .contains(&ApprovalScope::Session)
            .then(|| prepared.grant.key.clone());
        let summary = card.summary.clone();
        // Held before the record exists so a fast approve + continue finds it.
        self.pending().insert(
            approval_id.clone(),
            PendingCall {
                invocation,
                call_id: call_id.clone(),
            },
        );
        let record = match self.store.insert(
            &NewApproval {
                card,
                call_id: call_id.clone(),
                fingerprint: prepared.fingerprint.clone(),
                grant_key,
            },
            &created_at,
        ) {
            Ok(record) => record,
            Err(error) => {
                self.pending().remove(&approval_id);
                return Err(error);
            }
        };
        let widest = if scope_options.contains(&ApprovalScope::Session) {
            ApprovalScope::Session
        } else {
            ApprovalScope::Once
        };
        if let Err(error) = self.audit.before(
            scope,
            EventKind::ApprovalCreated {
                approval_id: approval_id.clone(),
                intent_id: prepared.primary().intent_id.clone(),
                summary,
                scope: events_scope(widest),
                expires_at: Some(expires_at),
            },
        ) {
            // An unrecorded approval must not be actionable.
            let _ = self
                .store
                .cancel(&approval_id, &self.clock.now(), "audit failure");
            self.pending().remove(&approval_id);
            return Err(error);
        }
        Ok(BrokerOutcome::PendingApproval {
            call_id,
            approval_id,
            card: Box::new(record.card),
        })
    }
}
