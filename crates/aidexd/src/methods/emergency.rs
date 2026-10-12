//! `emergency.*` methods (spec §48): stop everything, optionally lock the vault,
//! and resume.

use crate::{
    methods::{approvals::broker_error, record_event},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_broker::Principal;
use aidex_events::{EventKind, EventScope};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use aidex_broker::EmergencyReport;

pub const STOP: &str = "emergency.stop";
pub const RESUME: &str = "emergency.resume";

/// Reason recorded when the caller gives none.
const DEFAULT_REASON: &str = "emergency stop requested by the local user";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StopParams {
    #[serde(default)]
    reason: Option<String>,
    /// Also lock the vault (every secret then needs the passphrase again).
    #[serde(default)]
    lock_vault: bool,
}

/// Result of `emergency.resume`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResumeResult {
    /// `false` when no emergency stop was in effect.
    pub resumed: bool,
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(STOP, move |_context, params| {
        let params: StopParams = parse_params(params)?;
        let reason = params
            .reason
            .as_deref()
            .map(str::trim)
            .filter(|reason| !reason.is_empty())
            .unwrap_or(DEFAULT_REASON);
        let mut report = shared
            .broker()
            .emergency_stop(reason, &Principal::LocalUser)
            .map_err(|error| broker_error(&error))?;
        if params.lock_vault {
            // Attachment uploads in flight end with the vault (files deleted).
            shared.native().store().abort_all_uploads();
            let vault_id = shared.vault().status().vault_id;
            let outcome = shared.vault().lock();
            report.vault_locked = true;
            report.revoked_handles += outcome.revoked_handles;
            if let (true, Some(vault_id)) = (outcome.was_unlocked, vault_id) {
                record_event(
                    &shared,
                    EventScope::global(),
                    EventKind::VaultLocked { vault_id },
                )?;
            }
        }
        to_result(&report)
    })?;

    let shared = Arc::clone(services);
    router.register(RESUME, move |_context, params| {
        expect_no_params(&params)?;
        let resumed = shared
            .broker()
            .resume_operations(&Principal::LocalUser)
            .map_err(|error| broker_error(&error))?;
        to_result(&ResumeResult { resumed })
    })?;
    Ok(())
}
