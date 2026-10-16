//! External runtimes share the daemon's vault, ChatGPT auth and offline switch.
use super::{ModelServices, ServicesError};
use aidex_adapters::RuntimeManager;
use aidex_broker::Broker;
use aidex_core::ServiceErrorCode;
use aidex_drive::DrivePaths;
use aidex_providers::OfflineMode;
use aidex_sandbox::Sandbox;
use aidex_vault::{VaultService, VaultState};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

/// No full load of the runtimes has happened yet.
const NEVER_LOADED: u64 = u64::MAX;

/// What the daemon keeps about the external runtimes it loaded into the native
/// engine: the runtime manager's configuration generation
/// (`RuntimeManager::generation`) at the last full load, so a change the
/// engine has not seen yet (a login that connected from its worker thread
/// while the load was refused) is noticed before the next turn; and the
/// runtimes whose managed account is being changed (an import or a sign-out
/// while the official CLI runs), which stay out of the engine until the change
/// ended. In memory only.
#[derive(Debug)]
pub(crate) struct RuntimeSync {
    loaded: AtomicU64,
    changing: Mutex<BTreeSet<String>>,
}

impl Default for RuntimeSync {
    fn default() -> Self {
        Self {
            loaded: AtomicU64::new(NEVER_LOADED),
            changing: Mutex::new(BTreeSet::new()),
        }
    }
}

impl RuntimeSync {
    /// The manager generation the engine's runtimes were last fully loaded
    /// from, if they ever were.
    pub(crate) fn loaded(&self) -> Option<u64> {
        Some(self.loaded.load(Ordering::SeqCst)).filter(|generation| *generation != NEVER_LOADED)
    }

    pub(crate) fn set_loaded(&self, generation: u64) {
        self.loaded.store(generation, Ordering::SeqCst);
    }

    fn changing_ids(&self) -> std::sync::MutexGuard<'_, BTreeSet<String>> {
        self.changing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Mark the accounts of runtimes `ids` as being changed, all or none,
    /// until the returned guard is dropped (also by a panic); `None` (and
    /// nothing marked) when one of them already is.
    pub(crate) fn begin_change(&self, ids: BTreeSet<String>) -> Option<AccountChange<'_>> {
        let mut changing = self.changing_ids();
        if ids.iter().any(|id| changing.contains(id)) {
            return None;
        }
        changing.extend(ids.iter().cloned());
        Some(AccountChange { sync: self, ids })
    }

    /// Whether runtime `id`'s account is being changed now.
    pub(crate) fn changing(&self, id: &str) -> bool {
        self.changing_ids().contains(id)
    }
}

/// Runtimes whose account is being changed ([`RuntimeSync::begin_change`]),
/// until this is dropped.
pub(crate) struct AccountChange<'a> {
    sync: &'a RuntimeSync,
    ids: BTreeSet<String>,
}

impl Drop for AccountChange<'_> {
    fn drop(&mut self) {
        let mut changing = self.sync.changing_ids();
        for id in &self.ids {
            changing.remove(id);
        }
    }
}

/// Open the runtime manager with `gate` ([`runtime_gate`]). It admits vendor
/// processes and official logins only while the drive is online, the vault is
/// unlocked and no emergency stop is in effect; once it closes, adapters stop
/// running vendor processes and pending logins within about 25 ms and refuse
/// new launches (`offline_refused`).
pub fn open_runtime(
    paths: &DrivePaths,
    vault: Arc<VaultService>,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
    models: &ModelServices,
) -> Result<Arc<RuntimeManager>, ServicesError> {
    RuntimeManager::open(
        paths.root().to_path_buf(),
        vault,
        models.auth().clone(),
        Sandbox::locate(),
        gate,
    )
    .map(Arc::new)
    .map_err(|e| ServicesError::new("runtime", ServiceErrorCode::Unavailable, e.to_string()))
}

/// Whether external runtimes may run now. Checked cheapest first: a locked
/// vault is the only case that reads the vault file, and adapters stop asking
/// once the gate has closed for them. The services are held weakly, so the gate
/// never keeps them alive; once they are gone it stays closed.
pub(crate) fn runtime_gate(
    offline: Arc<OfflineMode>,
    vault: Weak<VaultService>,
    broker: Weak<Broker>,
) -> Arc<dyn Fn() -> bool + Send + Sync> {
    Arc::new(move || {
        !offline.is_offline()
            && broker
                .upgrade()
                .is_some_and(|broker| broker.emergency_reason().is_none())
            && vault
                .upgrade()
                .is_some_and(|vault| vault.status().state == VaultState::Unlocked)
    })
}
