//! External runtimes share the daemon's vault, ChatGPT auth and offline switch.
use super::{ModelServices, ServicesError};
use aidex_adapters::RuntimeManager;
use aidex_core::ServiceErrorCode;
use aidex_drive::DrivePaths;
use aidex_sandbox::Sandbox;
use aidex_vault::VaultService;
use std::sync::Arc;
pub fn open_runtime(
    paths: &DrivePaths,
    vault: Arc<VaultService>,
    models: &ModelServices,
) -> Result<Arc<RuntimeManager>, ServicesError> {
    let offline = models.offline().clone();
    RuntimeManager::open(
        paths.root().to_path_buf(),
        vault,
        models.auth().clone(),
        Sandbox::locate(),
        Arc::new(move || !offline.is_offline()),
    )
    .map(Arc::new)
    .map_err(|e| ServicesError::new("runtime", ServiceErrorCode::Unavailable, e.to_string()))
}
