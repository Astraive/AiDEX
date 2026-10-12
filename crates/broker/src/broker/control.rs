//! Policy changes, call/session cancellation, and emergency stop (spec §48).

use aidex_core::EntityId;
use aidex_events::{EventKind, EventScope};
use aidex_sandbox::StopReason;
use serde::Serialize;

use super::{Broker, PolicyStatus, load_documents};
use crate::{
    BrokerError, EmergencyReport, PolicyChange, Principal, approvals::ApprovalRecord, settings,
};

/// What [`Broker::cancel_session`] did.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SessionCancelReport {
    pub cancelled_calls: usize,
    pub cancelled_approvals: Vec<EntityId>,
    pub revoked_approvals: Vec<EntityId>,
}

fn ids(records: &[ApprovalRecord]) -> Vec<EntityId> {
    records
        .iter()
        .map(|record| record.approval_id.clone())
        .collect()
}

fn count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

impl Broker {
    /// Change the active policy settings or reload policy documents, as the local
    /// user. Agents cannot reach this method (their `policy.change` intents are
    /// denied), and remote devices are refused until remote authorization exists.
    /// Persists the settings and records `policy.changed`.
    pub fn change_policy(
        &self,
        change: PolicyChange,
        by: &Principal,
    ) -> Result<PolicyStatus, BrokerError> {
        if !matches!(by, Principal::LocalUser) {
            return Err(BrokerError::Unauthorized(format!(
                "{by} may not change policy; use the local CLI or desktop"
            )));
        }
        let profile = {
            let mut state = self
                .policy
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut next = state.settings;
            change.apply(&mut next);
            if next != state.settings {
                if let Some(path) = &self.config.settings_path {
                    settings::store(path, &next)?;
                }
                state.settings = next;
            }
            if change == PolicyChange::ReloadDocuments {
                state.documents = load_documents(self.config.policies_dir.as_deref());
            }
            state.settings.profile
        };
        let profile = EntityId::new(profile.as_str())
            .map_err(|error| BrokerError::internal("profile id", error))?;
        self.audit.after(
            &EventScope::global(),
            EventKind::PolicyChanged {
                profile: Some(profile),
                summary: format!("{} by {by}", change.describe()),
            },
        )?;
        Ok(self.policy_status())
    }

    /// Request cancellation of one running call; `false` when it is not running.
    pub fn cancel_call(&self, call_id: &EntityId) -> bool {
        match self.active().get(call_id) {
            Some(call) => {
                call.cancel.cancel();
                true
            }
            None => false,
        }
    }

    /// End a session's broker state: cancel its running calls and pending
    /// approvals and revoke its unused approvals and session grants.
    pub fn cancel_session(
        &self,
        session_id: &EntityId,
        by: &Principal,
    ) -> Result<SessionCancelReport, BrokerError> {
        let mut cancelled_calls = 0;
        for call in self
            .active()
            .values()
            .filter(|call| &call.session_id == session_id)
        {
            call.cancel.cancel();
            cancelled_calls += 1;
        }
        let now = self.clock.now();
        let cancelled =
            self.store
                .cancel_pending(Some(session_id), &now, "the session was cancelled")?;
        let revoked =
            self.store
                .revoke_active(Some(session_id), by, &now, "the session was cancelled")?;
        self.record_stopped(&cancelled, &revoked, "the session was cancelled")?;
        Ok(SessionCancelReport {
            cancelled_calls,
            cancelled_approvals: ids(&cancelled),
            revoked_approvals: ids(&revoked),
        })
    }

    fn record_stopped(
        &self,
        cancelled: &[ApprovalRecord],
        revoked: &[ApprovalRecord],
        reason: &str,
    ) -> Result<(), BrokerError> {
        let mut pending = self.pending();
        for record in cancelled.iter().chain(revoked) {
            pending.remove(&record.approval_id);
        }
        drop(pending);
        for record in cancelled {
            self.audit.after(
                &EventScope::session(record.workspace_id.clone(), record.session_id.clone()),
                EventKind::ApprovalCancelled {
                    approval_id: record.approval_id.clone(),
                },
            )?;
        }
        for record in revoked {
            self.audit.after(
                &EventScope::session(record.workspace_id.clone(), record.session_id.clone()),
                EventKind::ApprovalRevoked {
                    approval_id: record.approval_id.clone(),
                    reason: Some(reason.to_owned()),
                },
            )?;
        }
        Ok(())
    }

    /// Emergency stop (spec §48): freeze new tool execution until
    /// [`Broker::resume_operations`], cancel every running call and pending
    /// approval, revoke every unused approval and session grant, terminate every
    /// managed process tree, withdraw launch permits, and revoke every vault handle.
    /// Locking the vault is the caller's choice (the daemon does it on request).
    /// Records `emergency.stopped`.
    pub fn emergency_stop(
        &self,
        reason: &str,
        by: &Principal,
    ) -> Result<EmergencyReport, BrokerError> {
        self.emergency.freeze(reason);
        let mut cancelled_calls = 0;
        for call in self.active().values() {
            call.cancel.cancel();
            cancelled_calls += 1;
        }
        let termination = self
            .sandbox()
            .registry()
            .shutdown(StopReason::EmergencyStop);
        self.launcher.withdraw_all();
        let revoked_handles = self
            .secrets
            .vault()
            .map_or(0, |vault| vault.revoke_all_handles());
        let now = self.clock.now();
        let cancelled = self.store.cancel_pending(None, &now, "emergency stop")?;
        let revoked = self.store.revoke_active(None, by, &now, "emergency stop")?;
        self.pending().clear();
        let report = EmergencyReport {
            stopped_at: now,
            reason: reason.to_owned(),
            cancelled_approvals: ids(&cancelled),
            revoked_approvals: ids(&revoked),
            cancelled_calls,
            terminated_processes: termination.terminated.len(),
            failed_terminations: termination.failed.len(),
            revoked_handles,
            vault_locked: false,
        };
        self.record_stopped(&cancelled, &revoked, "emergency stop")?;
        self.audit.after(
            &EventScope::global(),
            EventKind::EmergencyStopped {
                reason: reason.to_owned(),
                cancelled_approvals: count(report.cancelled_approvals.len()),
                revoked_approvals: count(report.revoked_approvals.len()),
                terminated_processes: count(report.terminated_processes),
                revoked_handles: count(report.revoked_handles),
            },
        )?;
        Ok(report)
    }

    /// Terminate every managed process tree and refuse new launches (daemon
    /// shutdown or drive removal). Returns how many trees were terminated.
    pub fn shutdown_processes(&self, drive_removed: bool) -> usize {
        let reason = if drive_removed {
            StopReason::DriveRemoved
        } else {
            StopReason::DaemonShutdown
        };
        for call in self.active().values() {
            call.cancel.cancel();
        }
        self.launcher.withdraw_all();
        self.sandbox().registry().shutdown(reason).terminated.len()
    }

    /// Lift an emergency stop: new submissions are accepted and the process
    /// registry accepts launches again. Cancelled approvals stay cancelled.
    /// Returns `false` when no stop was in effect.
    pub fn resume_operations(&self, by: &Principal) -> Result<bool, BrokerError> {
        if !matches!(by, Principal::LocalUser) {
            return Err(BrokerError::Unauthorized(format!(
                "{by} may not resume operations after an emergency stop"
            )));
        }
        if !self.emergency.thaw() {
            return Ok(false);
        }
        self.sandbox().registry().reopen();
        self.audit
            .after(&EventScope::global(), EventKind::EmergencyResumed {})?;
        Ok(true)
    }
}
