fn main() {
    // Commands get generated `allow-<command>` permissions; `capabilities/default.json`
    // grants exactly these (checked by a unit test in `src/app.rs`).
    let manifest = tauri_build::AppManifest::new().commands(&[
        "aidex_status",
        "drive_status",
        "host_profile",
        "sandbox_capabilities",
        "daemon_start",
        "daemon_stop",
        "rpc_call",
        "rpc_subscribe",
        "rpc_unsubscribe",
        "select_drive",
        "set_ui_scale",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run the Tauri build script");
}
