//! `#[tauri::command]` adapters. Each hands its work to a blocking worker (volume
//! queries run PowerShell on Windows) and returns `SetupError` on failure.

use crate::{
    error::SetupError,
    reports::{self, DriveInspection, InitReport},
};
use aidex_drive::VolumeInfo;
use aidex_host::HostProfile;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

/// Every command; tests check `build.rs`, `app.rs`, and the capability file against it.
#[cfg(test)]
pub const COMMANDS: &[&str] = &[
    "list_drives",
    "inspect_drive",
    "init_drive",
    "inspect_normal_target",
    "init_normal_target",
    "host_profile",
];

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, SetupError> + Send + 'static,
) -> Result<T, SetupError> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| SetupError::internal(format!("setup worker failed: {error}")))?
}

/// Mounted volumes with capacity, attachment, and AiDEX manifest state.
#[tauri::command]
pub async fn list_drives() -> Result<Vec<VolumeInfo>, SetupError> {
    blocking(|| aidex_drive::list_volumes().map_err(SetupError::from)).await
}

/// Whether `root` can be initialized and what AiDEX state it already holds. Read only.
#[tauri::command]
pub async fn inspect_drive(root: String) -> Result<DriveInspection, SetupError> {
    blocking(move || reports::inspect(&PathBuf::from(root))).await
}

/// Create the AiDEX layout and manifest at removable `root` (never formats or deletes).
#[tauri::command]
pub async fn init_drive(root: String) -> Result<InitReport, SetupError> {
    blocking(move || reports::initialize(&PathBuf::from(root))).await
}

/// Inspect the standard per-user install target. Read only.
#[tauri::command]
pub async fn inspect_normal_target(app: AppHandle) -> Result<DriveInspection, SetupError> {
    blocking(move || {
        let base = app
            .path()
            .local_data_dir()
            .map_err(|error| SetupError::internal(format!("cannot resolve local data directory: {error}")))?;
        reports::inspect_normal(&base)
    })
    .await
}

/// Create the AiDEX layout in the standard per-user target without elevation or destructive writes.
#[tauri::command]
pub async fn init_normal_target(app: AppHandle) -> Result<InitReport, SetupError> {
    blocking(move || {
        let base = app
            .path()
            .local_data_dir()
            .map_err(|error| SetupError::internal(format!("cannot resolve local data directory: {error}")))?;
        reports::initialize_normal(&base)
    })
    .await
}

/// Read-only host facts (`aidex-host`).
#[tauri::command]
pub async fn host_profile() -> Result<HostProfile, SetupError> {
    blocking(|| Ok(HostProfile::current())).await
}
