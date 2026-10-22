fn main() {
    // Commands get generated `allow-<command>` permissions; `capabilities/default.json`
    // grants exactly these (checked by a unit test in `src/app.rs`).
    let manifest = tauri_build::AppManifest::new().commands(&[
        "list_drives",
        "inspect_drive",
        "init_drive",
        "inspect_normal_target",
        "init_normal_target",
        "host_profile",
    ]);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest))
        .expect("failed to run the Tauri build script");
}
