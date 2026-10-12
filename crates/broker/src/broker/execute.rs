//! Execution under a permit, denials, and sandbox availability checks.

use std::time::Duration;

use aidex_core::EntityId;
use aidex_events::{EventKind, EventScope, RiskLevel, redact_text};
use aidex_policy::SandboxLevel;
use aidex_sandbox::MountAccess;
use aidex_tools::{ExecutionPermit, PermitIssuer, ToolContext, ToolError};

use super::{ActiveCall, Broker};
use crate::{
    BrokerError, BrokerOutcome, PermitInfo, ToolFailure, ToolInvocation,
    audit::{events_level, sandbox_level},
    intent::Prepared,
    launcher::LaunchGrant,
    workspaces::ResolvedWorkspace,
};

/// Extra lifetime of a secret handle beyond the tool's process timeout.
const HANDLE_GRACE: Duration = Duration::from_secs(30);

pub(super) fn failure(error: &ToolError) -> ToolFailure {
    ToolFailure {
        reason: error.reason().to_owned(),
        code: error.code(),
        message: redact_text(&error.to_string()).into_owned(),
    }
}

impl Broker {
    pub(super) fn plan_failed(
        &self,
        scope: &EventScope,
        invocation: &ToolInvocation,
        call_id: EntityId,
        risk: RiskLevel,
        error: &ToolError,
        requested: bool,
    ) -> Result<BrokerOutcome, BrokerError> {
        let failure = failure(error);
        if requested {
            self.audit.before(
                scope,
                EventKind::ToolRequested {
                    call_id: call_id.clone(),
                    tool_id: invocation.tool_id.entity_id().clone(),
                    summary: format!("{} (rejected before planning)", invocation.tool_id),
                    risk,
                },
            )?;
        }
        self.audit.after(
            scope,
            EventKind::ToolFailed {
                call_id: call_id.clone(),
                code: failure.reason.clone(),
                message: failure.message.clone(),
            },
        )?;
        Ok(BrokerOutcome::Failed { call_id, failure })
    }

    pub(super) fn deny(
        &self,
        scope: &EventScope,
        call_id: EntityId,
        reason: String,
        rule: String,
    ) -> Result<BrokerOutcome, BrokerError> {
        let reason = redact_text(&reason).into_owned();
        self.audit.before(
            scope,
            EventKind::ToolDenied {
                call_id: call_id.clone(),
                reason: format!("{reason} [{rule}]"),
            },
        )?;
        Ok(BrokerOutcome::Denied {
            call_id,
            reason,
            rule,
        })
    }

    pub(super) fn sandbox_denied(
        &self,
        scope: &EventScope,
        call_id: EntityId,
        prepared: &Prepared,
        reason: String,
    ) -> Result<BrokerOutcome, BrokerError> {
        self.audit.note(
            scope,
            EventKind::SandboxNegotiated {
                level: events_level(required_level(prepared)),
                enforced: false,
            },
        );
        self.deny(
            scope,
            call_id,
            format!(
                "the required `{}` sandbox cannot be enforced on this host: {reason}",
                prepared.combined.sandbox.max(SandboxLevel::Brokered)
            ),
            "broker:sandbox:unavailable".into(),
        )
    }

    /// Why the call's processes cannot run at the required level here, if so.
    pub(super) fn sandbox_unavailable(
        &self,
        prepared: &Prepared,
        workspace: &ResolvedWorkspace,
    ) -> Option<String> {
        let descriptor = self.registry.descriptor(prepared.plan.tool_id())?;
        let level = required_level(prepared);
        if !descriptor.risk.spawns_process {
            // In-process tools run inside the broker on their fixed, confined plan;
            // that satisfies process-isolation levels up to `restricted`, but not an
            // ephemeral environment.
            return (level >= aidex_sandbox::SandboxLevel::Ephemeral)
                .then(|| "in-process tools cannot run in an ephemeral environment".to_owned());
        }
        let needs_network = prepared.plan.facts().network.is_some();
        self.launcher
            .check(
                level,
                &workspace.root.display_root(),
                MountAccess::ReadWrite,
                needs_network,
                self.config.tool_limits.process_timeout,
            )
            .err()
    }

    /// Run `prepared` under a permit from `issuer`. The only path to tool execution.
    pub(super) fn execute(
        &self,
        ctx: &ToolContext,
        workspace: &ResolvedWorkspace,
        prepared: Prepared,
        issuer: PermitIssuer,
        scope: &EventScope,
    ) -> Result<BrokerOutcome, BrokerError> {
        let call_id = ctx.call_id.clone();
        let plan = &prepared.plan;
        if let Some(limit) = prepared.combined.rate_limit {
            let key = format!(
                "{}|{}|{}",
                plan.session_id(),
                plan.tool_id(),
                prepared.combined.rule
            );
            if !self.rate.try_acquire(&key, limit) {
                return self.deny(
                    scope,
                    call_id,
                    format!(
                        "rate limit reached: at most {} calls per {} s",
                        limit.max,
                        limit.per.as_secs()
                    ),
                    prepared.combined.rule.clone(),
                );
            }
        }
        if let Some(reason) = self.sandbox_unavailable(&prepared, workspace) {
            return self.sandbox_denied(scope, call_id, &prepared, reason);
        }
        let issued = match self.secrets.issue(
            &prepared.credentials,
            self.config.tool_limits.process_timeout + HANDLE_GRACE,
        ) {
            Ok(issued) => issued,
            Err(error) => {
                let failure = ToolFailure {
                    reason: "credential_unavailable".into(),
                    code: error.code(),
                    message: error.to_string(),
                };
                self.audit.after(
                    scope,
                    EventKind::ToolFailed {
                        call_id: call_id.clone(),
                        code: failure.reason.clone(),
                        message: failure.message.clone(),
                    },
                )?;
                return Ok(BrokerOutcome::Failed { call_id, failure });
            }
        };
        for secret in &issued {
            let consumer = EntityId::new(secret.handle.consumer.to_string())
                .map_err(|error| BrokerError::internal("consumer id", error));
            let granted = consumer.and_then(|consumer| {
                self.audit.before(
                    scope,
                    EventKind::SecretGranted {
                        handle_id: secret.handle.handle_id.clone(),
                        secret_name: secret.secret_name.as_str().to_owned(),
                        consumer,
                    },
                )
            });
            if let Err(error) = granted {
                self.secrets.revoke(&issued);
                return Err(error);
            }
        }
        let level = required_level(&prepared);
        let permit_guard = self.launcher.permit(
            plan.fingerprint(),
            LaunchGrant {
                session_id: plan.session_id().clone(),
                workspace_id: plan.workspace_id().clone(),
                level,
                secrets: issued.clone(),
            },
        );
        self.active().insert(
            call_id.clone(),
            ActiveCall {
                session_id: plan.session_id().clone(),
                cancel: ctx.cancel.clone(),
            },
        );
        // An emergency stop that began after the submission check still wins.
        let result = match self.emergency.check() {
            Ok(()) => {
                self.registry
                    .execute(ctx, plan, ExecutionPermit::for_plan(plan, issuer.clone()))
            }
            Err(_stopped) => Err(ToolError::Cancelled),
        };
        self.active().remove(&call_id);
        drop(permit_guard);
        self.secrets.revoke(&issued);
        for secret in &issued {
            self.audit.note(
                scope,
                EventKind::SecretRevoked {
                    handle_id: secret.handle.handle_id.clone(),
                    reason: Some("the tool call finished".into()),
                },
            );
        }
        Ok(match result {
            Ok(output) => BrokerOutcome::Completed {
                call_id,
                output,
                permit: PermitInfo {
                    issuer,
                    sandbox: prepared.combined.sandbox.max(SandboxLevel::Brokered),
                },
            },
            Err(error) => BrokerOutcome::Failed {
                call_id,
                failure: failure(&error),
            },
        })
    }
}

/// The isolation a call's processes need: the decision's level, at least brokered.
fn required_level(prepared: &Prepared) -> aidex_sandbox::SandboxLevel {
    sandbox_level(prepared.combined.sandbox.max(SandboxLevel::Brokered))
}
