//! `aidex approvals|approve|deny|policy|tools|stop|resume` end to end against an
//! in-process daemon.

use aidex_sessions::Session;
use aidex_workspace::Workspace;
use aidexd::{
    Client, Daemon, DaemonHandle,
    methods::{sessions::CREATE, tools::INVOKE, workspace::CREATE_DRIVE},
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

fn aidex(args: &[&str], root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aidex"))
        .args(args)
        .args(["--drive", root.to_str().expect("utf-8 temp path")])
        .env_remove("AIDEX_DRIVE_ROOT")
        .env("AIDEX_OMP", "aidex-test-missing-omp-executable")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run aidex")
}

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json_of(output: &Output) -> Value {
    assert_eq!(code(output), 0, "{}", text(output));
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", text(output)))
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

struct Setup {
    _daemon: DaemonHandle,
    client: Client,
    session: Session,
    workspace: PathBuf,
}

fn setup(root: &Path) -> Setup {
    let daemon = Daemon::new(root).unwrap().start().unwrap();
    let mut client = Client::connect(root, "test-cli-approvals").unwrap();
    let workspace: Workspace = client
        .call_as(CREATE_DRIVE, json!({"name": "project"}))
        .unwrap();
    let session: Session = client
        .call_as(
            CREATE,
            json!({"workspace_id": workspace.id.as_str(), "agent_id": "aidex", "title": "cli"}),
        )
        .unwrap();
    Setup {
        _daemon: daemon,
        client,
        session,
        workspace: PathBuf::from(&workspace.path),
    }
}

impl Setup {
    fn pending_delete(&mut self, name: &str) -> String {
        std::fs::write(self.workspace.join(name), "x\n").unwrap();
        let outcome = self
            .client
            .call(
                INVOKE,
                json!({"session": self.session.id.as_str(), "tool": "fs.delete", "input": {"path": name}}),
            )
            .unwrap();
        assert_eq!(outcome["outcome"], "pending_approval", "{outcome}");
        outcome["approval_id"].as_str().unwrap().to_owned()
    }
}

#[test]
fn approval_commands_list_show_approve_and_deny() {
    let root = temp_drive();
    let drive = root.path();
    let mut setup = setup(drive);

    let profile = aidex(&["policy", "profile", "safe"], drive);
    assert_eq!(code(&profile), 0, "{}", text(&profile));
    let shown = json_of(&aidex(&["--json", "policy", "show"], drive));
    assert_eq!(shown["settings"]["profile"], "safe");

    let first = setup.pending_delete("a.txt");
    let second = setup.pending_delete("b.txt");

    let listed = json_of(&aidex(&["--json", "approvals", "--pending"], drive));
    let ids: Vec<&str> = listed["approvals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["approval_id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&first.as_str()) && ids.contains(&second.as_str()),
        "{ids:?}"
    );

    let card = aidex(&["approvals", "show", &first], drive);
    let card = text(&card);
    for expected in [
        "Effect:",
        "Deletes a.txt",
        "Workspace:  project",
        "Scope:      once or session",
        "Expires:",
    ] {
        assert!(card.contains(expected), "{expected} missing:\n{card}");
    }

    // Without a terminal, approving needs --yes and changes nothing.
    let refused = aidex(&["approve", &first], drive);
    assert_eq!(code(&refused), 2, "{}", text(&refused));
    let still = json_of(&aidex(&["--json", "approvals", "show", &first], drive));
    assert_eq!(still["state"]["status"], "pending");

    let approved = json_of(&aidex(&["--json", "approve", &first, "--yes"], drive));
    assert_eq!(approved["state"]["status"], "approved");
    assert_eq!(approved["state"]["scope"], "once");
    let denied = json_of(&aidex(&["--json", "deny", &second], drive));
    assert_eq!(denied["state"]["status"], "denied");
    let again = aidex(&["deny", &second], drive);
    assert_ne!(code(&again), 0, "{}", text(&again));

    let third = setup.pending_delete("c.txt");
    let revoked = json_of(&aidex(&["--json", "approvals", "revoke", &third], drive));
    assert_eq!(revoked["state"]["status"], "revoked");

    let explained = json_of(&aidex(
        &[
            "--json",
            "policy",
            "explain",
            "--tool",
            "fs.delete",
            "--input",
            r#"{"path":"c.txt"}"#,
            "--workspace",
            "project",
        ],
        drive,
    ));
    assert_eq!(explained["decision"]["verdict"]["kind"], "require_approval");
    let human = aidex(
        &[
            "policy",
            "explain",
            "--tool",
            "fs.read",
            "--input",
            r#"{"path":"c.txt"}"#,
            "--workspace",
            "project",
        ],
        drive,
    );
    assert!(text(&human).contains("Decision:"), "{}", text(&human));

    let tools = aidex(&["tools", "list"], drive);
    assert_eq!(code(&tools), 0, "{}", text(&tools));
    assert!(text(&tools).contains("git.push"), "{}", text(&tools));
}

#[test]
fn emergency_stop_and_resume_from_the_cli() {
    let root = temp_drive();
    let drive = root.path();
    let mut setup = setup(drive);
    aidex(&["policy", "profile", "safe"], drive);
    let waiting = setup.pending_delete("a.txt");

    let usage = aidex(&["stop"], drive);
    assert_eq!(code(&usage), 2, "{}", text(&usage));
    let report = json_of(&aidex(
        &["--json", "stop", "--emergency", "--reason", "drill"],
        drive,
    ));
    assert_eq!(report["cancelled_approvals"][0], waiting.as_str());
    let shown = json_of(&aidex(&["--json", "approvals", "show", &waiting], drive));
    assert_eq!(shown["state"]["status"], "cancelled");
    let status = aidex(&["policy", "show"], drive);
    assert!(text(&status).contains("STOPPED"), "{}", text(&status));

    let resumed = json_of(&aidex(&["--json", "resume"], drive));
    assert_eq!(resumed["resumed"], true);
}
