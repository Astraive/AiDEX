//! End-to-end `aidex workspace` checks against an in-process daemon on a
//! temporary developer drive: add/list/select survive a daemon restart, and
//! ephemeral changes are applied only after confirmation.

use aidexd::Daemon;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

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

fn ok_json(output: &Output) -> Value {
    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn stderr_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stderr).unwrap_or_else(|error| {
        panic!(
            "stderr is not JSON ({error}): {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn workspaces_are_added_selected_and_reopened_after_a_daemon_restart() {
    let drive = temp_drive();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("README.md"), "hello").unwrap();
    let project_path = project.path().to_str().unwrap();

    let daemon = Daemon::new(drive.path()).unwrap().start().unwrap();
    let added = ok_json(&aidex(
        &["--json", "workspace", "add", project_path, "--name", "demo"],
        drive.path(),
    ));
    let id = added["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("wsp_"));
    assert_eq!(added["kind"]["type"], "host");
    assert_eq!(added["availability"]["state"], "available");

    let selected = ok_json(&aidex(
        &["--json", "workspace", "select", "demo"],
        drive.path(),
    ));
    assert_eq!(selected["id"], id.as_str());
    let created = ok_json(&aidex(
        &["--json", "project", "create", "notes"],
        drive.path(),
    ));
    assert_eq!(created["kind"]["type"], "drive");

    let duplicate = aidex(&["--json", "workspace", "add", project_path], drive.path());
    assert_eq!(duplicate.status.code(), Some(1));
    assert!(duplicate.stdout.is_empty());
    assert_eq!(stderr_json(&duplicate)["error"]["code"], "failure");

    daemon.stop();
    daemon.wait().unwrap();
    let restarted = Daemon::new(drive.path()).unwrap().start().unwrap();

    let list = ok_json(&aidex(&["--json", "workspace", "list"], drive.path()));
    assert_eq!(list["selected"], id.as_str());
    assert_eq!(list["workspaces"].as_array().unwrap().len(), 2);
    let shown = ok_json(&aidex(&["--json", "workspace", "show"], drive.path()));
    assert_eq!(shown["id"], id.as_str());
    let human = aidex(&["workspace", "list"], drive.path());
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(
        text.contains(&format!("* {id}  demo  host  available")),
        "{text}"
    );

    let missing = aidex(&["--json", "workspace", "show", "nope"], drive.path());
    assert_eq!(missing.status.code(), Some(8));
    assert_eq!(stderr_json(&missing)["error"]["code"], "not_found");

    // Removing a host workspace never deletes its files.
    ok_json(&aidex(
        &["--json", "workspace", "remove", &id],
        drive.path(),
    ));
    assert!(project.path().join("README.md").is_file());
    restarted.stop();
    restarted.wait().unwrap();
}

#[test]
fn ephemeral_apply_requires_confirmation_and_applies_only_selected_paths() {
    let drive = temp_drive();
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join("a.txt"), "a\n").unwrap();
    fs::write(project.path().join("b.txt"), "b\n").unwrap();
    let daemon = Daemon::new(drive.path()).unwrap().start().unwrap();
    ok_json(&aidex(
        &[
            "--json",
            "workspace",
            "add",
            project.path().to_str().unwrap(),
            "--name",
            "src",
        ],
        drive.path(),
    ));
    let created = ok_json(&aidex(
        &["--json", "workspace", "ephemeral", "create", "src"],
        drive.path(),
    ));
    let id = created["workspace"]["id"].as_str().unwrap().to_owned();
    let tree = Path::new(created["workspace"]["path"].as_str().unwrap()).to_path_buf();
    fs::write(tree.join("a.txt"), "a changed\n").unwrap();
    fs::write(tree.join("b.txt"), "b changed\n").unwrap();

    let review = aidex(&["workspace", "ephemeral", "review", &id], drive.path());
    let text = String::from_utf8_lossy(&review.stdout);
    assert!(
        text.contains("M a.txt") && text.contains("M b.txt"),
        "{text}"
    );
    assert!(text.contains("+a changed"), "{text}");

    // Without a terminal and without --yes, nothing is applied.
    let refused = aidex(
        &["--json", "workspace", "ephemeral", "apply", &id],
        drive.path(),
    );
    assert_eq!(refused.status.code(), Some(2));
    assert_eq!(
        fs::read_to_string(project.path().join("a.txt")).unwrap(),
        "a\n"
    );

    let report = ok_json(&aidex(
        &[
            "--json",
            "workspace",
            "ephemeral",
            "apply",
            &id,
            "--path",
            "a.txt",
            "--yes",
        ],
        drive.path(),
    ));
    assert_eq!(report["applied"], serde_json::json!(["a.txt"]));
    assert_eq!(
        fs::read_to_string(project.path().join("a.txt")).unwrap(),
        "a changed\n"
    );
    assert_eq!(
        fs::read_to_string(project.path().join("b.txt")).unwrap(),
        "b\n"
    );

    let bad_path = aidex(
        &[
            "workspace",
            "ephemeral",
            "apply",
            &id,
            "--path",
            "../x",
            "--yes",
        ],
        drive.path(),
    );
    assert_eq!(bad_path.status.code(), Some(2));

    let refused = aidex(&["workspace", "ephemeral", "discard", &id], drive.path());
    assert_eq!(refused.status.code(), Some(2));
    assert!(tree.exists());
    ok_json(&aidex(
        &["--json", "workspace", "ephemeral", "discard", &id, "--yes"],
        drive.path(),
    ));
    assert!(!tree.exists());
    assert_eq!(
        fs::read_to_string(project.path().join("b.txt")).unwrap(),
        "b\n"
    );
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn selective_apply_refuses_paths_omitted_from_a_truncated_review() {
    let drive = temp_drive();
    let project = tempfile::tempdir().unwrap();
    let daemon = Daemon::new(drive.path()).unwrap().start().unwrap();
    ok_json(&aidex(
        &[
            "--json",
            "workspace",
            "add",
            project.path().to_str().unwrap(),
            "--name",
            "source",
        ],
        drive.path(),
    ));
    let created = ok_json(&aidex(
        &["--json", "workspace", "ephemeral", "create", "source"],
        drive.path(),
    ));
    let id = created["workspace"]["id"].as_str().unwrap();
    let tree = Path::new(created["workspace"]["path"].as_str().unwrap());
    for index in 0..=aidexd::methods::workspace::MAX_REVIEW_ENTRIES {
        fs::write(tree.join(format!("file_{index:04}.txt")), "new\n").unwrap();
    }
    let review = ok_json(&aidex(
        &["--json", "workspace", "ephemeral", "review", id],
        drive.path(),
    ));
    assert_eq!(review["truncated"], true);
    let omitted = format!(
        "file_{:04}.txt",
        aidexd::methods::workspace::MAX_REVIEW_ENTRIES
    );
    assert!(
        review["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|change| change["path"] != omitted)
    );
    let result = aidex(
        &[
            "--json",
            "workspace",
            "ephemeral",
            "apply",
            id,
            "--path",
            &omitted,
            "--yes",
        ],
        drive.path(),
    );
    assert_eq!(
        result.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!project.path().join(&omitted).exists());
    daemon.stop();
    daemon.wait().unwrap();
}
