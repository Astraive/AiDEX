//! What the daemon does whenever the vault's unlocked session ends.
//!
//! `VaultService` reports every end exactly once, outside its own locks: an
//! explicit lock (`vault.lock`, `emergency.stop` with `lock_vault`), the idle
//! auto-lock, a `Conflict`, an uncertain commit of its own header, or a panic.
//! The next unlock waits until the report was handled, so this cleanup never
//! reaches the session that follows. Each end gets the cleanup `vault.lock`
//! always did: native runs are cancelled, attachment uploads in flight are
//! aborted (their partial sealed files are deleted and later chunks are
//! refused), and `vault.locked` is recorded. MCP servers are stopped by their own
//! lock observer, with a polling watchdog as a backstop (`services/mcp.rs`).
//! Further cleanups that must follow the vault (for example runtime sign-ins or
//! external runtime processes) belong here.
//!
//! One replacement is not reported: `vault.unlock` while a session is open whose
//! `vault.json` changed on disk swaps in a session for the new header, because
//! the vault never locks in between. Nothing here runs then.

use aidex_events::{EventKind, EventLog, EventScope};
use aidex_harness::NativeEngine;
use aidex_vault::VaultService;
use std::sync::Arc;

/// Run the lock cleanup for every session end of `vault`. The engine is held
/// weakly because its session store owns the vault.
pub fn clean_up_on_lock(vault: &VaultService, native: &Arc<NativeEngine>, events: &EventLog) {
    let native = Arc::downgrade(native);
    let events = events.clone();
    vault.add_lock_observer(move |notice| {
        if let Some(native) = native.upgrade() {
            native.cancel_all();
            native.store().abort_all_uploads();
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
