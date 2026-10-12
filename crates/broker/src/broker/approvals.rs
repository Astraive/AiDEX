//! Approval transitions, each recorded as an `approval.*` event.

use aidex_core::EntityId;
use aidex_events::{EventKind, EventScope};

use super::Broker;
use crate::{
    ApprovalRefusal, BrokerError, Principal,
    approvals::{ApprovalFilter, ApprovalRecord, ApprovalResolution, ApprovalState},
    audit::events_scope,
};

fn scope_of(record: &ApprovalRecord) -> EventScope {
    EventScope::session(record.workspace_id.clone(), record.session_id.clone())
}

fn refusal_for(state: &ApprovalState) -> ApprovalRefusal {
    match state {
        ApprovalState::Pending => ApprovalRefusal::Pending,
        ApprovalState::Approved { .. } => ApprovalRefusal::AlreadyResolved,
        ApprovalState::Denied { .. } => ApprovalRefusal::Denied,
        ApprovalState::Expired { .. } => ApprovalRefusal::Expired,
        ApprovalState::Revoked { .. } => ApprovalRefusal::Revoked,
        ApprovalState::Cancelled { .. } => ApprovalRefusal::Cancelled,
    }
}

impl Broker {
    /// One approval record.
    pub fn approval(&self, approval_id: &EntityId) -> Result<ApprovalRecord, BrokerError> {
        self.store
            .get(approval_id, &self.clock.now())?
            .ok_or_else(|| {
                BrokerError::approval(approval_id, ApprovalRefusal::NotFound, "no such approval")
            })
    }

    /// Approvals matching `filter`, newest first.
    pub fn list_approvals(
        &self,
        filter: &ApprovalFilter,
    ) -> Result<Vec<ApprovalRecord>, BrokerError> {
        self.store.list(filter, &self.clock.now())
    }

    /// Decide a pending approval. Exactly one of any racing decisions (approve,
    /// deny, cancel, expire, revoke) wins; the others are refused with the winner's
    /// state. An approval whose deadline passed is expired instead.
    /// `approve_session` is accepted only when the card offered the session scope;
    /// it creates a session grant valid for the configured grant lifetime.
    pub fn resolve_approval(
        &self,
        approval_id: &EntityId,
        resolution: ApprovalResolution,
        by: &Principal,
    ) -> Result<ApprovalRecord, BrokerError> {
        let now = self.clock.now();
        let current = self.approval(approval_id)?;
        if let Some(scope) = resolution.scope() {
            let offered = &current.card.scope_options;
            if current.state == ApprovalState::Pending && !offered.contains(&scope) {
                let offered: Vec<&str> = offered.iter().map(|scope| scope.as_str()).collect();
                return Err(BrokerError::approval(
                    approval_id,
                    ApprovalRefusal::ScopeNotOffered,
                    format!(
                        "approving for `{scope}` is not offered (only: {})",
                        offered.join(", ")
                    ),
                ));
            }
        }
        let changed = match resolution.scope() {
            Some(scope) => {
                let grant_expires = (scope == aidex_policy::ApprovalScope::Session)
                    .then(|| {
                        let ttl = i64::try_from(self.config.session_grant_ttl.as_millis())
                            .unwrap_or(i64::MAX);
                        aidex_core::Timestamp::from_unix_millis(
                            now.unix_millis().saturating_add(ttl),
                        )
                    })
                    .transpose()
                    .map_err(|error| BrokerError::internal("grant expiry", error))?;
                self.store
                    .approve(approval_id, scope, by, &now, grant_expires.as_ref())?
                    .map(|record| {
                        let event = EventKind::ApprovalApproved {
                            approval_id: approval_id.clone(),
                            scope: events_scope(scope),
                        };
                        (record, event)
                    })
            }
            None => self.store.deny(approval_id, by, &now)?.map(|record| {
                self.pending().remove(approval_id);
                let event = EventKind::ApprovalDenied {
                    approval_id: approval_id.clone(),
                    reason: Some(format!("denied by {by}")),
                };
                (record, event)
            }),
        };
        match changed {
            Some((record, event)) => {
                self.audit.after(&scope_of(&record), event)?;
                Ok(record)
            }
            None => {
                // Lost the race, or the deadline passed: report the winner's state.
                self.expire_due()?;
                let latest = self.approval(approval_id)?;
                Err(BrokerError::approval(
                    approval_id,
                    refusal_for(&latest.state),
                    format!("the approval is already {}", latest.status()),
                ))
            }
        }
    }

    /// Revoke a pending approval, an approved approval that has not run yet, or a
    /// session grant.
    pub fn revoke_approval(
        &self,
        approval_id: &EntityId,
        by: &Principal,
        reason: Option<&str>,
    ) -> Result<ApprovalRecord, BrokerError> {
        let now = self.clock.now();
        let reason = reason
            .map(str::to_owned)
            .unwrap_or_else(|| format!("revoked by {by}"));
        match self.store.revoke(approval_id, by, &now, Some(&reason))? {
            Some(record) => {
                self.pending().remove(approval_id);
                self.audit.after(
                    &scope_of(&record),
                    EventKind::ApprovalRevoked {
                        approval_id: approval_id.clone(),
                        reason: Some(reason),
                    },
                )?;
                Ok(record)
            }
            None => {
                let latest = self.approval(approval_id)?;
                Err(BrokerError::approval(
                    approval_id,
                    ApprovalRefusal::AlreadyResolved,
                    format!(
                        "a {} approval{} cannot be revoked",
                        latest.status(),
                        if latest.consumed_at.is_some() {
                            " that already ran"
                        } else {
                            ""
                        }
                    ),
                ))
            }
        }
    }

    /// Cancel a pending approval (the requester gave up, the session ended).
    pub fn cancel_approval(
        &self,
        approval_id: &EntityId,
        reason: &str,
    ) -> Result<ApprovalRecord, BrokerError> {
        let now = self.clock.now();
        match self.store.cancel(approval_id, &now, reason)? {
            Some(record) => {
                self.pending().remove(approval_id);
                self.audit.after(
                    &scope_of(&record),
                    EventKind::ApprovalCancelled {
                        approval_id: approval_id.clone(),
                    },
                )?;
                Ok(record)
            }
            None => {
                let latest = self.approval(approval_id)?;
                Err(BrokerError::approval(
                    approval_id,
                    refusal_for(&latest.state),
                    format!("the approval is already {}", latest.status()),
                ))
            }
        }
    }

    /// Expire every pending approval whose deadline has passed (the daemon calls
    /// this periodically). Returns the approvals that expired.
    pub fn expire_due(&self) -> Result<Vec<ApprovalRecord>, BrokerError> {
        let expired = self.store.expire_due(&self.clock.now())?;
        for record in &expired {
            self.pending().remove(&record.approval_id);
            self.audit.after(
                &scope_of(record),
                EventKind::ApprovalExpired {
                    approval_id: record.approval_id.clone(),
                },
            )?;
        }
        Ok(expired)
    }
}
