//! Tauri composition: managed state, command registration, and lifecycle hooks.

use crate::{commands, state::AppState};
use std::sync::Arc;
use tauri::{webview::PageLoadEvent, Manager, RunEvent, WindowEvent};

/// Build and run the desktop app.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let state = Arc::new(AppState::default());
    let app = tauri::Builder::default()
        .manage(Arc::clone(&state))
        // The window stays on the app's pages, e.g. when a file is dropped outside a drop target.
        .plugin(crate::navigation::plugin())
        .invoke_handler(tauri::generate_handler![
            commands::aidex_status,
            commands::drive_status,
            commands::host_profile,
            commands::sandbox_capabilities,
            commands::daemon_start,
            commands::daemon_stop,
            commands::rpc_call,
            commands::rpc_call_long,
            commands::rpc_subscribe,
            commands::rpc_unsubscribe,
            commands::select_drive,
            commands::set_ui_scale,
        ])
        // A (re)loaded page has no listeners for earlier subscriptions.
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Started {
                webview
                    .state::<Arc<AppState>>()
                    .bridge
                    .subscriptions
                    .clear();
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build the AiDEX desktop application");

    app.run(move |_, event| match event {
        RunEvent::WindowEvent {
            event: WindowEvent::Destroyed,
            ..
        } => {
            state.bridge.subscriptions.clear();
        }
        RunEvent::Exit => {
            state.bridge.reset();
            // Only a daemon this app launched is stopped; failures cannot be reported now.
            let _ = state.daemon.stop_owned();
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use crate::commands::COMMANDS;
    use serde_json::Value;

    #[test]
    fn the_capability_grants_exactly_the_registered_commands_and_event_listening() {
        let capability: Value =
            serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
        let mut granted: Vec<String> = capability["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|permission| permission.as_str().unwrap().to_owned())
            .collect();
        granted.sort();
        let mut expected: Vec<String> = COMMANDS
            .iter()
            .map(|command| format!("allow-{}", command.replace('_', "-")))
            .chain([
                "core:event:allow-listen".into(),
                "core:event:allow-unlisten".into(),
            ])
            .collect();
        expected.sort();
        assert_eq!(granted, expected);
    }

    #[test]
    fn the_build_manifest_and_handler_list_name_every_command() {
        let build = include_str!("../build.rs");
        let app = include_str!("app.rs");
        for command in COMMANDS {
            assert!(
                build.contains(&format!("\"{command}\"")),
                "build.rs lacks {command}"
            );
            assert!(
                app.contains(&format!("commands::{command},")),
                "app.rs lacks {command}"
            );
        }
    }
}
