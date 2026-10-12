//! End-to-end `aidex plugins` against an in-process daemon: install a folder, review
//! it, enable it by trusting its tree hash, disable it, and uninstall it.

use aidexd::Daemon;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

const SECRET: &str = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";

fn aidex(args: &[&str], drive: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aidex"))
        .args(args)
        .args(["--drive", drive.to_str().expect("utf-8 temp path")])
        .env_remove("AIDEX_DRIVE_ROOT")
        .output()
        .expect("run aidex")
}

fn temp_drive() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    aidex_drive::init_with(
        root.path(),
        aidex_drive::InitOptions {
            require_removable: false,
        },
    )
    .unwrap();
    root
}

fn ok(output: &Output) -> String {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn ok_json(output: &Output) -> Value {
    serde_json::from_str(&ok(output)).expect("stdout is JSON")
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).expect("stderr is JSON")
}

fn write_plugin(root: &Path) {
    for (path, content) in [
        (
            ".claude-plugin/plugin.json",
            r#"{"name": "review", "version": "1.0.0", "description": "Reviews code.", "permissions": {"network": ["api.github.com"]}}"#.to_owned(),
        ),
        (
            "skills/review/SKILL.md",
            "---\nname: review\ndescription: Checks a diff\n---\nBody.\n".to_owned(),
        ),
        (
            ".mcp.json",
            format!(r#"{{"mcpServers": {{"github": {{"command": "npx", "args": ["-y", "server-github"], "env": {{"GITHUB_TOKEN": "{SECRET}"}}}}}}}}"#),
        ),
    ] {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

#[test]
fn install_review_enable_disable_and_uninstall_from_the_command_line() {
    let drive = temp_drive();
    let sources = tempfile::tempdir().unwrap();
    let plugin = sources.path().join("review");
    write_plugin(&plugin);
    let daemon = Daemon::new(drive.path()).unwrap().start().unwrap();
    let source = plugin.to_str().unwrap();

    let human = ok(&aidex(&["plugins", "list"], drive.path()));
    assert!(human.contains("No plugins installed"), "{human}");

    let installed = aidex(&["plugins", "install", source], drive.path());
    let human = ok(&installed);
    assert!(
        human.contains("Installed plugin review 1.0.0 (disabled)"),
        "{human}"
    );
    assert!(human.contains("api.github.com"), "{human}");
    assert!(
        human.contains("github (stdio): npx -y server-github"),
        "{human}"
    );
    assert!(
        human.contains("GITHUB_TOKEN (values are not imported)"),
        "{human}"
    );
    assert!(human.contains("--trust"), "{human}");
    assert!(!human.contains(SECRET), "{human}");

    let shown = ok_json(&aidex(
        &["--json", "plugins", "show", "review"],
        drive.path(),
    ));
    assert_eq!(shown["enabled"], false);
    assert!(!shown.to_string().contains(SECRET));
    let hash = shown["tree_sha256"].as_str().unwrap().to_owned();

    // Enabling needs the reviewed tree hash first.
    let refused = aidex(&["--json", "plugins", "enable", "review"], drive.path());
    assert_eq!(refused.status.code(), Some(6), "{refused:?}");
    let error = stderr_json(&refused);
    assert!(
        error["error"]["hint"].as_str().unwrap().contains("--trust"),
        "{error}"
    );
    let wrong = aidex(
        &["plugins", "enable", "review", "--trust", &"0".repeat(64)],
        drive.path(),
    );
    assert_eq!(wrong.status.code(), Some(1), "{wrong:?}");
    assert!(String::from_utf8_lossy(&wrong.stderr).contains("review it again"));

    let enabled = ok_json(&aidex(
        &["--json", "plugins", "enable", "review", "--trust", &hash],
        drive.path(),
    ));
    assert_eq!(
        (enabled["enabled"].clone(), enabled["trusted"].clone()),
        (Value::Bool(true), Value::Bool(true))
    );
    let human = ok(&aidex(&["plugins", "list"], drive.path()));
    assert!(
        human.contains("review") && human.contains("enabled"),
        "{human}"
    );

    let human = ok(&aidex(&["plugins", "disable", "review"], drive.path()));
    assert!(human.contains("Disabled plugin review"), "{human}");
    // Trust is kept: enabling again needs no hash.
    ok(&aidex(&["plugins", "enable", "review"], drive.path()));

    let exists = aidex(&["--json", "plugins", "install", source], drive.path());
    assert_eq!(exists.status.code(), Some(1), "{exists:?}");
    assert!(
        stderr_json(&exists)["error"]["hint"]
            .as_str()
            .unwrap()
            .contains("--replace")
    );

    let removed = ok_json(&aidex(
        &["--json", "plugins", "uninstall", "review"],
        drive.path(),
    ));
    assert_eq!(removed["was_enabled"], true);
    let missing = aidex(&["plugins", "show", "review"], drive.path());
    assert_eq!(missing.status.code(), Some(8), "{missing:?}");
    daemon.stop();
}
