//! `#[tauri::command]` adapters. Each one hands its work to a blocking worker (daemon
//! IPC, PowerShell queries, and process launches block) and returns `BridgeError` on
//! failure. Behaviour lives in `bridge`, `lifecycle`, `reports`, and `ui_scale`.

pub mod reports;
pub mod ui_scale;

use crate::{
    bridge::{
        call_long, EndedPayload, NotificationEmitter, NotificationPayload, Subscribed, ENDED_EVENT,
        NOTIFICATION_EVENT,
    },
    error::BridgeError,
    lifecycle::drive::validate_selection,
    state::AppState,
};
use aidex_host::HostProfile;
use aidex_sandbox::BackendCapabilities;
use reports::{AidexStatus, DriveReport};
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use tauri::{AppHandle, Emitter, State, Webview};

/// Every command; tests check `build.rs`, `app.rs`, and the capability file against it.
#[cfg(test)]
pub const COMMANDS: &[&str] = &[
    "aidex_status",
    "drive_status",
    "host_profile",
    "sandbox_capabilities",
    "daemon_start",
    "daemon_stop",
    "rpc_call",
    "rpc_call_long",
    "rpc_subscribe",
    "rpc_unsubscribe",
    "select_drive",
    "set_ui_scale",
];

type Shared<'a> = State<'a, Arc<AppState>>;

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, BridgeError> + Send + 'static,
) -> Result<T, BridgeError> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| BridgeError::internal(format!("bridge worker failed: {error}")))?
}

/// The session drive and whether its daemon answers.
#[tauri::command]
pub async fn aidex_status(state: Shared<'_>) -> Result<AidexStatus, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || Ok(reports::aidex_status(&state))).await
}

/// Manifest health and volume facts for the session drive.
#[tauri::command]
pub async fn drive_status(state: Shared<'_>) -> Result<DriveReport, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let drive = state.drive.require()?;
        Ok(reports::drive_report(&drive.root, drive.source))
    })
    .await
}

/// Read-only host capability profile (`aidex-host`).
#[tauri::command]
pub async fn host_profile() -> Result<HostProfile, BridgeError> {
    blocking(|| Ok(HostProfile::current())).await
}

/// What the OS sandbox backend can enforce on this host (`aidex-sandbox`).
#[tauri::command]
pub async fn sandbox_capabilities() -> Result<BackendCapabilities, BridgeError> {
    blocking(|| Ok(aidex_sandbox::probe())).await
}

/// Launch the daemon for the session drive, then report the new status.
#[tauri::command]
pub async fn daemon_start(state: Shared<'_>) -> Result<AidexStatus, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let drive = state.drive.require()?;
        state.bridge.connection.disconnect();
        state.daemon.start(&drive.root)?;
        Ok(reports::aidex_status(&state))
    })
    .await
}

/// Stop the daemon serving the session drive, then report the new status.
#[tauri::command]
pub async fn daemon_stop(state: Shared<'_>) -> Result<AidexStatus, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let drive = state.drive.require()?;
        state.bridge.reset();
        state.daemon.stop(&drive.root)?;
        Ok(reports::aidex_status(&state))
    })
    .await
}

/// Call any daemon method. Errors keep the daemon's stable code, kind, and message.
#[tauri::command]
pub async fn rpc_call(
    state: Shared<'_>,
    method: String,
    params: Option<Value>,
) -> Result<Value, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let drive = state.drive.require()?;
        state
            .bridge
            .connection
            .call(&drive.root, &method, params.unwrap_or(Value::Null))
    })
    .await
}

/// Call `update.check`, `update.apply`, or `update.rollback` (refusing anything else) on a
/// connection of its own with the 30-minute update limit, so a slow package on a USB drive
/// neither times out nor holds up every other call on the shared connection.
#[tauri::command]
pub async fn rpc_call_long(
    state: Shared<'_>,
    method: String,
    params: Option<Value>,
) -> Result<Value, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let drive = state.drive.require()?;
        call_long(&drive.root, &method, params.unwrap_or(Value::Null))
    })
    .await
}

/// Subscribe to a daemon stream; notifications arrive as `aidex://notification` events.
#[tauri::command]
pub async fn rpc_subscribe(
    app: AppHandle,
    state: Shared<'_>,
    method: String,
    params: Option<Value>,
) -> Result<Subscribed, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let drive = state.drive.require()?;
        state.bridge.subscriptions.subscribe(
            &drive.root,
            &method,
            params.unwrap_or(Value::Null),
            Arc::new(AppEmitter(app)),
        )
    })
    .await
}

/// Stop a subscription; returns whether it was live.
#[tauri::command]
pub async fn rpc_unsubscribe(state: Shared<'_>, subscription: String) -> Result<bool, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || Ok(state.bridge.subscriptions.unsubscribe(&subscription))).await
}

/// Use `path` as this session's drive (memory only), then report the new status.
#[tauri::command]
pub async fn select_drive(state: Shared<'_>, path: String) -> Result<AidexStatus, BridgeError> {
    let state = Arc::clone(&state);
    blocking(move || {
        let root = validate_selection(&PathBuf::from(path))?;
        state.bridge.reset();
        state.drive.select(root);
        Ok(reports::aidex_status(&state))
    })
    .await
}

/// Zoom the calling window's webview to the Appearance interface scale, in
/// percent (75-125). Nothing is saved on the host.
#[tauri::command]
pub async fn set_ui_scale(webview: Webview, percent: u16) -> Result<(), BridgeError> {
    let factor = ui_scale::zoom_factor(percent)?;
    webview
        .set_zoom(factor)
        .map_err(|error| BridgeError::internal(format!("cannot set the interface scale: {error}")))
}

/// Forwards subscription workers' notifications to the webview.
struct AppEmitter(AppHandle);

impl NotificationEmitter for AppEmitter {
    fn notification(&self, payload: NotificationPayload) {
        let _ = self.0.emit(NOTIFICATION_EVENT, payload);
    }

    fn ended(&self, payload: EndedPayload) {
        let _ = self.0.emit(ENDED_EVENT, payload);
    }
}
