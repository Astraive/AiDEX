//! `emergency.*` methods (spec §48): stop everything, optionally lock the vault,
//! and resume.

use crate::{
    methods::approvals::broker_error,
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_broker::Principal;
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
            // As `vault.lock`: native runs are cancelled, uploads in flight are
            // aborted (files deleted), and `vault.locked` is recorded.
            let outcome = super::vault::lock_vault(&shared);
            report.vault_locked = true;
            report.revoked_handles += outcome.revoked_handles;
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
