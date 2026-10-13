//! Agent work counts as vault use: while it runs, the vault's idle auto-lock
//! waits. An explicit `vault.lock` or an emergency stop with `lock_vault` still
//! locks at once (and cancels the work).
//!
//! Agent work is the same for native and external runs: a run with a working
//! thread, any vendor process (an official login included), and a run that
//! waits for an approval. A native run waiting for an approval has no thread;
//! it is found in the session store's plaintext run index, which never touches
//! the vault. An external run paused for an approval keeps its thread entry
//! and its vendor process, and is listed as waiting in the same index.
//!
//! A waiting run counts only once it belongs to the current vault session.
//! Every end of a session drops the prepared plans its waiting runs need (the
//! lock cleanup cancels them), and `reconcile_after_unlock` settles those runs
//! after the next unlock. A run the index still lists with the status it had
//! when the session ended is a record that could not be settled, not work, so
//! it never holds the vault open; it counts again once its status changes.

use crate::lifecycle::ShutdownSignal;
use aidex_adapters::RuntimeManager;
use aidex_harness::NativeEngine;
use aidex_native_sessions::{RunStatus, SessionStore};
use aidex_vault::VaultService;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};

/// The longest the keep-alive waits between checks.
const MAX_TICK: Duration = Duration::from_secs(1);
/// The shortest, for very short idle periods (tests).
const MIN_TICK: Duration = Duration::from_millis(10);

/// How long the keep-alive waits between checks: a quarter of the idle period,
/// at most a second, so active work touches the session at least four times
/// per period.
pub(crate) fn tick(idle: Option<Duration>) -> Duration {
    idle.map_or(MAX_TICK, |timeout| (timeout / 4).clamp(MIN_TICK, MAX_TICK))
}

/// Which runs listed as active belong to an earlier vault session.
pub(crate) struct RunActivity {
    /// Runs the index listed as active when the last vault session ended (or
    /// the daemon started, with the vault locked), with the status they had.
    left_over: Mutex<HashMap<String, RunStatus>>,
}

impl RunActivity {
    /// At daemon start the vault is locked, so every run the index lists as
    /// active is left over from an earlier daemon.
    pub(crate) fn new(store: &SessionStore) -> Arc<Self> {
        let activity = Arc::new(Self {
            left_over: Mutex::new(HashMap::new()),
        });
        activity.session_ended(store);
        activity
    }

    fn left_over(&self) -> MutexGuard<'_, HashMap<String, RunStatus>> {
        self.left_over
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The vault session ended (called by the lock cleanup, after it cancelled
    /// the runs): every run still listed as active belongs to it. An index that
    /// cannot be read keeps the previous list.
    pub(crate) fn session_ended(&self, store: &SessionStore) {
        // The list stays locked while the index is read, so a concurrent check
        // never mixes two readings.
        let mut left_over = self.left_over();
        if let Ok(runs) = store.run_summaries() {
            *left_over = runs
                .into_iter()
                .filter(|run| run.status.active())
                .map(|run| (run.id, run.status))
                .collect();
        }
    }

    /// Whether a run of the current vault session waits for an approval.
    /// Left-over runs whose status changed (the engine moved them on) or that
    /// ended are forgotten. An index that cannot be read counts as no waiting
    /// run, so the idle lock is never held open by an error.
    pub(crate) fn waiting_run(&self, store: &SessionStore) -> bool {
        let mut left_over = self.left_over();
        let Ok(runs) = store.run_summaries() else {
            return false;
        };
        let mut still_left = HashMap::new();
        let mut waiting = false;
        for run in runs.into_iter().filter(|run| run.status.active()) {
            if left_over.get(&run.id) == Some(&run.status) {
                still_left.insert(run.id, run.status);
            } else if run.status == RunStatus::WaitingApproval {
                waiting = true;
            }
        }
        *left_over = still_left;
        waiting
    }
}

/// Whether agent work is running that will need the vault (see the module
/// documentation). Reads in-memory state and the plaintext run index only.
pub(crate) fn agent_work_active(
    native: &NativeEngine,
    runtime: Option<&RuntimeManager>,
    activity: &RunActivity,
) -> bool {
    native.has_active()
        || runtime.is_some_and(|runtime| !runtime.process_registry().is_empty())
        || activity.waiting_run(native.store())
}

/// Keep the vault's idle timer from expiring while agent work is running, until
/// shutdown (or the vault is gone). Once the work ends, nothing is touched and
/// the idle auto-lock follows its usual period. It never reads the vault file,
/// and a locked vault stays locked.
pub(crate) fn spawn(
    vault: &Arc<VaultService>,
    native: &Arc<NativeEngine>,
    runtime: Option<&Arc<RuntimeManager>>,
    activity: &Arc<RunActivity>,
    shutdown: &ShutdownSignal,
) {
    let vault = Arc::downgrade(vault);
    let native = Arc::downgrade(native);
    let runtime = runtime.map(Arc::downgrade);
    let activity = Arc::clone(activity);
    let shutdown = shutdown.clone();
    // Without this thread an active run can be cut off by the idle auto-lock,
    // which then cancels it: a failed spawn only loses that protection.
    let _ = std::thread::Builder::new()
        .name("aidexd-vault-keep-alive".into())
        .spawn(move || {
            while !shutdown.is_requested() {
                let pause = {
                    let Some(vault) = vault.upgrade() else {
                        break;
                    };
                    let runtime = runtime.as_ref().and_then(Weak::upgrade);
                    if native.upgrade().is_some_and(|native| {
                        agent_work_active(&native, runtime.as_deref(), &activity)
                    }) {
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
