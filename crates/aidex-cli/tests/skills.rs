//! End-to-end `aidex skills list|show|enable|disable` against an in-process
//! daemon: drive skills, workspace skills that start disabled, and errors.

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

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"),
    )
    .unwrap();
}

#[test]
fn skills_are_listed_shown_and_toggled_from_the_command_line() {
    let drive = temp_drive();
    let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
    write_skill(
        &paths.skills(),
        "notes",
        "Take notes",
        "Write \u{1b}[31mnotes\u{1b}[0m.",
    );
    let project = tempfile::tempdir().unwrap();
    write_skill(
        &project.path().join(".aidex").join("skills"),
        "deploy",
        "Deploy the app",
        "Run the deploy script.",
    );
    let daemon = Daemon::new(drive.path()).unwrap().start().unwrap();
    let path = project.path().to_str().unwrap();
    ok_json(&aidex(
        &["--json", "workspace", "add", path, "--name", "app"],
        drive.path(),
    ));

    let list = ok_json(&aidex(&["--json", "skills", "list"], drive.path()));
    let skills = list["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 1, "{list}");
    assert_eq!(skills[0]["id"], "drive:notes");
    assert_eq!(skills[0]["enabled"], true);

    let list = ok_json(&aidex(
        &["--json", "skills", "list", "--workspace", "app"],
        drive.path(),
    ));
    let ids: Vec<(&str, bool)> = list["skills"]
        .as_array()
        .unwrap()
        .iter()
        .map(|skill| {
            (
                skill["id"].as_str().unwrap(),
                skill["enabled"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(ids, [("project:deploy", false), ("drive:notes", true)]);
    let human = ok(&aidex(
        &["skills", "list", "--workspace", "app"],
        drive.path(),
    ));
    assert!(human.contains("project:deploy"), "{human}");
    assert!(human.contains("disabled"), "{human}");

    let refused = aidex(
        &["--json", "skills", "enable", "project:deploy"],
        drive.path(),
    );
    assert_ne!(refused.status.code(), Some(0), "{refused:?}");
    let error: Value = serde_json::from_slice(&refused.stderr).expect("stderr is JSON");
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("workspace"),
        "{error}"
    );
    let enabled = ok(&aidex(
        &["skills", "enable", "project:deploy", "--workspace", "app"],
        drive.path(),
    ));
    assert_eq!(enabled.trim(), "Enabled project:deploy.");
    let disabled = ok_json(&aidex(
        &["--json", "skills", "disable", "drive:notes"],
        drive.path(),
    ));
    assert_eq!(disabled["enabled"], false);

    let shown = ok_json(&aidex(
        &[
            "--json",
            "skills",
            "show",
            "project:deploy",
            "--workspace",
            "app",
        ],
        drive.path(),
    ));
    assert_eq!(shown["body"], "Run the deploy script.");
    assert_eq!(shown["enabled"], true);
    assert_eq!(shown["front_matter"][0]["key"], "name");
    let human = ok(&aidex(&["skills", "show", "drive:notes"], drive.path()));
    assert!(human.contains("state:       disabled"), "{human}");
    assert!(human.contains("notes"), "{human}");
    assert!(
        !human.contains('\u{1b}'),
        "escape sequences are neutralised: {human:?}"
    );

    let missing = aidex(&["--json", "skills", "show", "drive:missing"], drive.path());
    assert_ne!(missing.status.code(), Some(0), "{missing:?}");
    let bad = aidex(&["skills", "show", "nonsense"], drive.path());
    assert_ne!(bad.status.code(), Some(0), "{bad:?}");
    daemon.stop();
}
