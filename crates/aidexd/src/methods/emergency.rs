//! `emergency.*` methods (spec §48): stop everything, optionally lock the vault,
//! and resume.
//!
//! Besides the broker's stop (tool calls, approvals, managed tool processes,
//! vault handles), `emergency.stop` cancels every native and external run and
//! every pending official runtime login, and terminates every vendor process.
//! The runtime gate stays closed while the stop is in effect, so no runtime
//! launches until `emergency.resume`; the runtimes themselves stay usable.

use crate::{
    methods::approvals::broker_error,
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_broker::Principal;
use aidex_sandbox::StopReason;
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

/// End agent work the broker does not own: native and external runs are
/// cancelled (not shut down, so `emergency.resume` leaves the engine and the
/// runtimes usable), pending official logins are cancelled, and vendor
/// processes still running are terminated. The broker's stop already closed the
/// runtime gate, so nothing new launches meanwhile.
fn stop_agent_work(services: &Services) {
    services.native().cancel_all();
    if let Ok(runtime) = services.runtime() {
        // Best effort: a login whose process cannot be confirmed ended reports
        // `process_shutdown_failed` through its own `runtime.login.status`.
        let _ = runtime.cancel_logins();
        runtime
            .process_registry()
            .terminate_all(StopReason::EmergencyStop);
    }
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
        // The broker freezes before anything else, so even when it fails later
        // (recording its approvals or events) the stop is in effect: agent work
        // still ends and the vault still locks as asked before the error is
        // reported.
        let stopped = shared
            .broker()
            .emergency_stop(reason, &Principal::LocalUser);
        stop_agent_work(&shared);
        let locked = params.lock_vault.then(|| {
            // As `vault.lock`: native runs are cancelled, uploads in flight are
            // aborted (files deleted), and `vault.locked` is recorded.
            super::vault::lock_vault(&shared)
        });
        let mut report = stopped.map_err(|error| broker_error(&error))?;
        if let Some(outcome) = locked {
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
