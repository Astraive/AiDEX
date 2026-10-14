//! What the daemon does whenever the vault's unlocked session ends.
//!
//! `VaultService` reports every end exactly once, outside its own locks: an
//! explicit lock (`vault.lock`, `emergency.stop` with `lock_vault`), the idle
//! auto-lock, a `Conflict`, an uncertain commit of its own header, or a panic.
//! The next unlock waits until the report was handled, so this cleanup never
//! reaches the session that follows. Each end gets the cleanup `vault.lock`
//! always did: native runs are cancelled, attachment uploads in flight are
//! aborted (their partial sealed files are deleted and later chunks are
//! refused), and `vault.locked` is recorded. External runtimes lose their
//! credentials with the vault: pending official logins are cancelled and every
//! vendor process still running is terminated (the runtime gate, which needs an
//! unlocked vault, already refuses new launches). MCP servers are stopped by
//! their own lock observer, with a polling watchdog as a backstop
//! (`services/mcp.rs`). Only executing runs hold the idle auto-lock off
//! (`keep_alive`), so this cleanup is also what stops a run left waiting for
//! an approval when the user walks away. Further cleanups that must follow the
//! vault belong here.
//!
//! One replacement is not reported: `vault.unlock` while a session is open whose
//! `vault.json` changed on disk swaps in a session for the new header, because
//! the vault never locks in between. Nothing here runs then.

use aidex_adapters::RuntimeManager;
use aidex_events::{EventKind, EventLog, EventScope};
use aidex_harness::NativeEngine;
use aidex_sandbox::StopReason;
use aidex_vault::VaultService;
use std::{sync::Arc, time::Duration};

/// How long the lock cleanup waits for vendor processes to exit. The runtime
/// gate closed with the vault, so most have already gone.
const RUNTIME_TERMINATION_GRACE: Duration = Duration::from_secs(2);

/// Run the lock cleanup for every session end of `vault`. The engine and the
/// runtime manager are held weakly because both own the vault.
pub fn clean_up_on_lock(
    vault: &VaultService,
    native: &Arc<NativeEngine>,
    runtime: Option<&Arc<RuntimeManager>>,
    events: &EventLog,
) {
    let native = Arc::downgrade(native);
    let runtime = runtime.map(Arc::downgrade);
    let events = events.clone();
    vault.add_lock_observer(move |notice| {
        if let Some(native) = native.upgrade() {
            native.cancel_all();
            native.store().abort_all_uploads();
        }
        if let Some(runtime) = runtime.as_ref().and_then(|runtime| runtime.upgrade()) {
            // Best effort: a login whose process cannot be confirmed ended is
            // reported by its own `runtime.login.status`.
            let _ = runtime.cancel_logins();
            runtime
                .process_registry()
                .terminate_all_within(StopReason::Cancelled, RUNTIME_TERMINATION_GRACE);
        }
        // Best effort by design: the session has already ended, and nothing
        // waits for this notice to report a failure to.
        let _ = events.append(
            EventScope::global(),
            EventKind::VaultLocked {
                vault_id: notice.vault_id.clone(),
            },
        );
    });
}
