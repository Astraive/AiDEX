//! Tauri composition for the setup wizard.

use crate::commands;

/// Build and run the setup app.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::list_drives,
            commands::inspect_drive,
            commands::init_drive,
            commands::inspect_normal_target,
            commands::init_normal_target,
            commands::host_profile,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run AiDEX setup");
}

#[cfg(test)]
mod tests {
    use crate::commands::COMMANDS;
    use serde_json::Value;

    #[test]
    fn the_capability_grants_exactly_the_registered_commands() {
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
