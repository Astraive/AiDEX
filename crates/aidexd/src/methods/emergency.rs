//! `emergency.*` methods (spec §48): stop everything, optionally lock the vault,
//! and resume.
//!
//! Besides the broker's stop (tool calls, approvals, managed tool processes,
//! vault handles), `emergency.stop` cancels every native and external run and
//! every pending official runtime login, terminates every vendor process and
//! official CLI command, and releases every launch parked for an approval (its
//! temporary home is removed). Its report counts the vendor process trees with
//! the broker's own. The runtime gate stays closed while the stop is in effect,
//! so no runtime launches until `emergency.resume`; the runtimes themselves
//! stay usable.

use crate::{
    methods::approvals::broker_error,
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_broker::Principal;
use aidex_sandbox::{StopReason, TerminationReport};
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
/// runtimes usable), pending official logins are cancelled, vendor processes
/// and official CLI commands still running are terminated, and launches parked
/// for an approval are released. The broker's stop already closed the runtime
/// gate, so nothing new launches meanwhile. Returns the runtime process
/// registry's report (`None` without runtimes); a tree it could not confirm
/// ended is logged as one fixed line, without ids or vendor text.
fn stop_agent_work(services: &Services) -> Option<TerminationReport> {
    services.native().cancel_all();
    let report = super::runtime::stop_runtime_processes(services, StopReason::EmergencyStop)?;
    if !report.is_clean() {
        eprintln!(
            "aidexd: an emergency stop could not confirm that every external runtime process ended"
        );
    }
    Some(report)
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
        // Runs on external runtimes are cancelled while the runtime gate is
        // still open: the broker's stop closes it, which ends their vendor
        // processes, and a run whose own cancel did not come first settles as
        // failed. This cannot fail. Then the broker freezes, so even when it
        // fails later (recording its approvals or events) the stop is in
        // effect: agent work still ends and the vault still locks as asked
        // before the error is reported.
        super::runtime::cancel_external_runs(&shared);
        let stopped = shared
            .broker()
            .emergency_stop(reason, &Principal::LocalUser);
        let runtime_report = stop_agent_work(&shared);
        let locked = params.lock_vault.then(|| {
            // As `vault.lock`: native runs are cancelled, uploads in flight are
            // aborted (files deleted), and `vault.locked` is recorded.
            super::vault::lock_vault(&shared)
        });
        let mut report = stopped.map_err(|error| broker_error(&error))?;
        // The vendor process trees count with the broker's own.
        if let Some(runtime) = runtime_report {
            report.terminated_processes += runtime.terminated.len();
            report.failed_terminations += runtime.failed.len();
        }
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
