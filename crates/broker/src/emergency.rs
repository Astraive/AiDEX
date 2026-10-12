//! Emergency stop (spec §48) state and report.

use std::sync::{PoisonError, RwLock, RwLockReadGuard};

use aidex_core::{EntityId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::BrokerError;

/// What an emergency stop did.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmergencyReport {
    pub stopped_at: Timestamp,
    pub reason: String,
    /// Pending approvals cancelled.
    pub cancelled_approvals: Vec<EntityId>,
    /// Approved-but-unused approvals and session grants revoked.
    pub revoked_approvals: Vec<EntityId>,
    /// In-flight tool calls whose cancellation was requested.
    pub cancelled_calls: usize,
    /// Sandboxed process trees terminated.
    pub terminated_processes: usize,
    /// Process trees that could not be confirmed terminated.
    pub failed_terminations: usize,
    /// Vault handles revoked (every handle of the unlock session).
    pub revoked_handles: usize,
    /// Set by the daemon when it also locked the vault.
    pub vault_locked: bool,
}

#[derive(Debug, Default)]
struct Frozen {
    reason: Option<String>,
}

/// Whether new tool execution is frozen.
#[derive(Debug, Default)]
pub(crate) struct EmergencyState {
    frozen: RwLock<Frozen>,
}

impl EmergencyState {
    fn read(&self) -> RwLockReadGuard<'_, Frozen> {
        self.frozen.read().unwrap_or_else(PoisonError::into_inner)
    }

    /// `Err(Stopped)` while an emergency stop is in effect.
    pub fn check(&self) -> Result<(), BrokerError> {
        match &self.read().reason {
            Some(reason) => Err(BrokerError::Stopped {
                reason: reason.clone(),
            }),
            None => Ok(()),
        }
    }

    pub fn reason(&self) -> Option<String> {
        self.read().reason.clone()
    }

    pub fn freeze(&self, reason: &str) {
        self.frozen
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .reason = Some(reason.to_owned());
    }

    /// Lift the freeze; `false` when none was in effect.
    pub fn thaw(&self) -> bool {
        self.frozen
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .reason
            .take()
            .is_some()
    }
}
