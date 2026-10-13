//! External runtimes share the daemon's vault, ChatGPT auth and offline switch.
use super::{ModelServices, ServicesError};
use aidex_adapters::RuntimeManager;
use aidex_broker::Broker;
use aidex_core::ServiceErrorCode;
use aidex_drive::DrivePaths;
use aidex_providers::OfflineMode;
use aidex_sandbox::Sandbox;
use aidex_vault::{VaultService, VaultState};
use std::sync::{Arc, Weak};

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
