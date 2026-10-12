//! `aidex mcp list|start|tools|logs|stop` end to end against an in-process
//! daemon and the `aidex-mcp` Python fixture server (`AIDEX_TEST_PYTHON`).

use aidexd::Daemon;
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn aidex(args: &[&str], root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aidex"))
        .args(args)
        .args(["--drive", root.to_str().expect("utf-8 temp path")])
        .env_remove("AIDEX_DRIVE_ROOT")
        .env("AIDEX_OMP", "aidex-test-missing-omp-executable")
        .output()
        .expect("run aidex")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json(output: &Output) -> Value {
    assert_eq!(output.status.code(), Some(0), "{}", text(output));
    serde_json::from_slice(&output.stdout).expect("stdout is JSON")
}

fn python() -> String {
    std::env::var("AIDEX_TEST_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "C:/Users/gskne/AppData/Local/Programs/Python/Python312/python.exe".into()
        } else {
            "/usr/bin/python3".into()
        }
    })
}

/// Write an enabled fixture server into the drive's `mcp/` directory.
fn configure_fixture(root: &Path) {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../mcp/tests/fixtures/mcp_server.py")
        .canonicalize()
        .unwrap();
    let paths = aidex_drive::DrivePaths::open(root).unwrap();
    let dir = paths.ensure_dir(&paths.mcp()).unwrap();
    std::fs::write(
        dir.join("fixture.toml"),
        format!(
            "version = 1\nid = \"fixture\"\nname = \"Fixture\"\ntransport = \"stdio\"\n\
             command = '{}'\nargs = ['-u', '{}', 'normal']\nenabled = true\n",
            python(),
            script.display()
        ),
    )
    .unwrap();
}

#[test]
fn mcp_commands_list_start_inspect_and_stop_servers() {
    let root = tempfile::tempdir().unwrap();
    aidex_drive::init_with(
        root.path(),
        aidex_drive::InitOptions {
            require_removable: false,
        },
    )
    .unwrap();
    configure_fixture(root.path());
    let daemon = Daemon::new(root.path()).unwrap().start().unwrap();

    let listed = json(&aidex(&["--json", "mcp", "list"], root.path()));
    assert_eq!(listed["servers"][0]["id"], "fixture");
    assert_eq!(listed["servers"][0]["status"], "stopped");
    let human = text(&aidex(&["mcp", "list"], root.path()));
    assert!(human.contains("fixture"), "{human}");
    assert!(human.contains("enabled"), "{human}");

    let started = json(&aidex(&["--json", "mcp", "start", "fixture"], root.path()));
    assert_eq!(started["status"], "running");
    assert_eq!(started["tool_count"], 7);

    let tools = aidex(&["mcp", "tools", "fixture"], root.path());
    let tools = text(&tools);
    assert!(tools.contains("echo"), "{tools}");
    assert!(tools.contains("Echo the given text"), "{tools}");

    let logs = json(&aidex(
        &["--json", "mcp", "logs", "fixture", "--limit", "3"],
        root.path(),
    ));
    assert_eq!(logs["lines"].as_array().unwrap().len(), 3);

    let stopped = json(&aidex(&["--json", "mcp", "stop", "fixture"], root.path()));
    assert_eq!(stopped["status"], "stopped");

    let missing = aidex(&["mcp", "start", "absent"], root.path());
    assert_ne!(missing.status.code(), Some(0));
    assert!(
        text(&missing).contains("no MCP server `absent`"),
        "{}",
        text(&missing)
    );
    daemon.stop();
}
