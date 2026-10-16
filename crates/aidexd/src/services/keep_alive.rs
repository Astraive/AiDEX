//! Executing agent work counts as vault use: while a run executes, the vault's
//! idle auto-lock waits. An explicit `vault.lock` or an emergency stop with
//! `lock_vault` still locks at once (and cancels the work).
//!
//! Only a run that is actually executing counts, the same for native and
//! external runs: one whose thread works on it (an external run's thread drives
//! its vendor process). Work that waits for a person does not: a run waiting
//! for an approval (a native one, or an external one paused with its vendor
//! process) and a pending official login. Nor do the reads clients poll in the
//! background (status, list and view methods registered with
//! `Router::register_passive`, which run inside `VaultService::passively`):
//! an open conversation or Security page does not hold the vault open. So
//! when the user walks away, the idle auto-lock happens as usual, and its
//! cleanup (`vault_lock`) stops that work: the waiting run is interrupted once
//! the vault is unlocked again and can be resumed as before, a vendor process
//! paused at its approval is ended and its temporary home removed, and the
//! login is cancelled.
//!
//! A native parent that finished its turn and waits for its active children
//! keeps executing while it waits, so a child waiting for an approval holds
//! the vault open through its parent until that wait ends (at most ten
//! minutes; the parent then completes without the child).

use crate::lifecycle::ShutdownSignal;
use aidex_harness::NativeEngine;
use aidex_vault::VaultService;
use std::{sync::Arc, time::Duration};

/// The longest the keep-alive waits between checks.
const MAX_TICK: Duration = Duration::from_secs(1);
/// The shortest, for very short idle periods (tests).
const MIN_TICK: Duration = Duration::from_millis(10);

/// How long the keep-alive waits between checks: a quarter of the idle period,
/// at most a second, so executing work touches the session at least four times
/// per period.
pub(crate) fn tick(idle: Option<Duration>) -> Duration {
    idle.map_or(MAX_TICK, |timeout| (timeout / 4).clamp(MIN_TICK, MAX_TICK))
}

/// Keep the vault's idle timer from expiring while a run is executing (see the
/// module documentation), until shutdown (or the vault is gone). Once no run
/// executes, nothing is touched and the idle auto-lock follows its usual
/// period. It reads in-memory engine state only, never the vault file, and a
/// locked vault stays locked.
pub(crate) fn spawn(
    vault: &Arc<VaultService>,
    native: &Arc<NativeEngine>,
    shutdown: &ShutdownSignal,
) {
    let vault = Arc::downgrade(vault);
    let native = Arc::downgrade(native);
    let shutdown = shutdown.clone();
    // Without this thread an executing run can be cut off by the idle
    // auto-lock, which then cancels it: a failed spawn only loses that
    // protection.
    let _ = std::thread::Builder::new()
        .name("aidexd-vault-keep-alive".into())
        .spawn(move || {
            while !shutdown.is_requested() {
                let pause = {
                    let Some(vault) = vault.upgrade() else {
                        break;
                    };
                    if native
                        .upgrade()
                        .is_some_and(|native| native.has_executing())
                    {
                        vault.keep_alive();
                    }
                    tick(vault.config().idle_timeout)
                };
                std::thread::sleep(pause);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_check_runs_at_least_four_times_per_idle_period_and_at_most_once_a_second() {
        assert_eq!(tick(None), MAX_TICK);
        assert_eq!(tick(Some(Duration::from_secs(30 * 60))), MAX_TICK);
        assert_eq!(
            tick(Some(Duration::from_secs(2))),
            Duration::from_millis(500)
        );
        assert_eq!(tick(Some(Duration::from_millis(8))), MIN_TICK);
    }
}
