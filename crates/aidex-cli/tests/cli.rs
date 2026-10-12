//! End-to-end checks of the `aidex` binary: `--json`, stable exit codes, and
//! daemon commands against an in-process daemon on a temporary, initialized
//! developer drive (`init_with` without the removable-media check).

use aidex_core::EntityId;
use aidex_events::{EventKind, EventLog, EventScope};
use aidexd::{Daemon, DaemonHandle, StopReason, endpoint};
use serde_json::Value;
use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn aidex(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_aidex"))
        .args(args)
        .env_remove("AIDEX_DRIVE_ROOT")
        .env("AIDEX_OMP", "aidex-test-missing-omp-executable")
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

fn drive_args<'a>(args: &[&'a str], root: &'a Path) -> Vec<&'a str> {
    let mut all = args.to_vec();
    all.extend(["--drive", root.to_str().expect("utf-8 temp path")]);
    all
}

fn stdout_json(output: &Output) -> Value {
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

fn code(output: &Output) -> i32 {
    output.status.code().expect("exit code")
}

#[test]
fn daemon_commands_report_status_and_stop_over_ipc() {
    let root = temp_drive();
    let daemon = Daemon::new(root.path()).unwrap().start().unwrap();

    let status = aidex(&drive_args(&["--json", "daemon", "status"], root.path()));
    assert_eq!(
        code(&status),
        0,
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let report = stdout_json(&status);
    assert_eq!(report["state"], "ready");
    assert_eq!(report["contract_version"], 2);
    assert_eq!(report["pid"], std::process::id());

    let human = aidex(&drive_args(&["daemon", "status"], root.path()));
    assert_eq!(code(&human), 0);
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("Daemon: ready"), "{text}");
    assert!(text.contains("IPC contract version: 2"), "{text}");

    let duplicate = aidex(&drive_args(&["daemon", "start"], root.path()));
    assert_eq!(code(&duplicate), 1);
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("already running"));

    let stopped = aidex(&drive_args(&["--json", "daemon", "stop"], root.path()));
    assert_eq!(
        code(&stopped),
        0,
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert_eq!(stdout_json(&stopped)["state"], "stopped");
    assert_eq!(daemon.wait().unwrap(), StopReason::Requested);

    let unavailable = aidex(&drive_args(&["--json", "daemon", "status"], root.path()));
    assert_eq!(code(&unavailable), 3);
    assert!(unavailable.stdout.is_empty());
    let error = &stderr_json(&unavailable)["error"];
    assert_eq!(error["code"], "daemon_unavailable");
    assert_eq!(error["exit_code"], 3);
    assert!(
        error["hint"]
            .as_str()
            .unwrap()
            .contains("aidex daemon start --drive")
    );

    let human = aidex(&drive_args(&["daemon", "stop"], root.path()));
    assert_eq!(code(&human), 3);
    let text = String::from_utf8_lossy(&human.stderr);
    assert!(
        text.starts_with("aidex daemon stop: daemon not running"),
        "{text}"
    );
}

#[test]
fn incompatible_daemon_endpoint_exits_with_code_10() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(endpoint::state_dir(root.path())).unwrap();
    fs::write(
        endpoint::state_path(root.path()),
        r#"{"version":1,"pid":1,"port":1,"token":"opaque"}"#,
    )
    .unwrap();
    let output = aidex(&drive_args(&["--json", "daemon", "status"], root.path()));
    assert_eq!(code(&output), 10);
    assert_eq!(
        stderr_json(&output)["error"]["code"],
        "incompatible_version"
    );
}

#[test]
fn drive_and_usage_failures_use_stable_exit_codes() {
    let root = tempfile::tempdir().unwrap();
    // A plain temporary directory is not a removable drive.
    let status = aidex(&drive_args(&["--json", "status"], root.path()));
    assert_eq!(code(&status), 4);
    assert!(status.stdout.is_empty());
    assert_eq!(stderr_json(&status)["error"]["code"], "drive_error");

    let missing = root.path().join("missing");
    let missing_drive = aidex(&["status", "--drive", missing.to_str().unwrap()]);
    assert_eq!(code(&missing_drive), 4);

    let no_drive = aidex(&["--json", "daemon", "status"]);
    assert_eq!(code(&no_drive), 4);

    assert_eq!(code(&aidex(&["--bogus"])), 2);
    assert_eq!(code(&aidex(&["init"])), 2);
    assert_eq!(code(&aidex(&["--help"])), 0);
    assert_eq!(code(&aidex(&["--version"])), 0);
}

#[test]
fn agent_list_and_doctor_emit_json_reports() {
    let agents = aidex(&["--json", "agent", "list"]);
    assert_eq!(code(&agents), 0);
    let report = stdout_json(&agents);
    assert_eq!(report["agents"][0]["id"], "omp");
    assert_eq!(report["agents"][0]["available"], false);

    let doctor = aidex(&["--json", "doctor"]);
    assert_eq!(code(&doctor), 1);
    let report = stdout_json(&doctor);
    assert_eq!(report["ok"], false);
    assert_eq!(report["drive"]["ok"], false);
    assert_eq!(report["daemon"]["running"], false);
    assert_eq!(report["oh_my_pi"]["available"], false);
    assert!(report["host"]["os"].is_string());
}

fn start_daemon(root: &Path) -> (DaemonHandle, EventLog) {
    let daemon = Daemon::new(root).unwrap();
    let log = daemon.services().events().clone();
    (daemon.start().unwrap(), log)
}

fn task(id: &str) -> EventKind {
    EventKind::TaskStarted {
        task_id: EntityId::new(id).unwrap(),
    }
}

fn scope() -> EventScope {
    EventScope::session(
        EntityId::new("ws_main").unwrap(),
        EntityId::new("ses_one").unwrap(),
    )
}

fn cursors(output: &Output) -> Vec<u64> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["cursor"]
                .as_u64()
                .unwrap()
        })
        .collect()
}

#[test]
fn events_tail_and_show_replay_over_ipc_from_the_cursor() {
    let root = temp_drive();
    let (daemon, log) = start_daemon(root.path());
    log.append(scope(), task("t1")).unwrap();
    log.append(scope(), task("t2")).unwrap();
    log.append(EventScope::global(), EventKind::DriveSafeToRemove {})
        .unwrap();

    let all = aidex(&drive_args(&["--json", "events", "tail"], root.path()));
    assert_eq!(code(&all), 0, "{}", String::from_utf8_lossy(&all.stderr));
    assert_eq!(cursors(&all), [1, 2, 3, 4]);
    let tasks = aidex(&drive_args(
        &[
            "--json",
            "events",
            "tail",
            "--after",
            "1",
            "--kind",
            "task",
            "--workspace",
            "ws_main",
        ],
        root.path(),
    ));
    assert_eq!(cursors(&tasks), [2, 3]);

    let page = aidex(&drive_args(
        &["--json", "events", "show", "--limit", "2"],
        root.path(),
    ));
    assert_eq!(code(&page), 0, "{}", String::from_utf8_lossy(&page.stderr));
    let page = stdout_json(&page);
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
    assert_eq!(page["next_cursor"], 2);
    assert_eq!(page["has_more"], true);

    let human = aidex(&drive_args(
        &["events", "show", "--after", "1"],
        root.path(),
    ));
    assert_eq!(code(&human), 0);
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(text.contains("task.started"), "{text}");
    assert!(text.contains("ws_main/ses_one"), "{text}");
    assert!(text.contains("drive.safe_to_remove"), "{text}");
    assert!(text.trim_end().ends_with("next cursor: 4"), "{text}");

    for args in [
        &["events", "tail", "--kind", "nope"][..],
        &["events", "tail", "--workspace", "bad id!"],
        &["events", "show", "--limit", "0"],
        &["events", "show", "--after", "99"],
    ] {
        let rejected = aidex(&drive_args(args, root.path()));
        assert_eq!(
            code(&rejected),
            2,
            "{args:?}: {}",
            String::from_utf8_lossy(&rejected.stderr)
        );
    }
    daemon.stop();
    daemon.wait().unwrap();

    let down = aidex(&drive_args(&["events", "show"], root.path()));
    assert_eq!(code(&down), 3);
}

#[test]
fn human_events_show_reports_a_closed_output_pipe_as_failure() {
    let root = temp_drive();
    let (daemon, log) = start_daemon(root.path());
    for index in 0..255 {
        log.append(
            scope(),
            EventKind::SessionFailed {
                reason: format!("event {index}: {}", "details ".repeat(512)),
            },
        )
        .unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_aidex"))
        .args(drive_args(
            &["events", "show", "--limit", "256"],
            root.path(),
        ))
        .env_remove("AIDEX_DRIVE_ROOT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let result = child.wait_with_output().unwrap();
    assert_eq!(
        code(&result),
        1,
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    daemon.stop();
    daemon.wait().unwrap();
}

/// Lines printed by a running `aidex` child, read on a background thread.
fn spawn_lines(args: &[&str]) -> (std::process::Child, mpsc::Receiver<String>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aidex"))
        .args(args)
        .env_remove("AIDEX_DRIVE_ROOT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn aidex");
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if sender.send(line).is_err() {
                return;
            }
        }
    });
    (child, receiver)
}

/// Collect printed cursors until `last` is seen.
fn read_through(lines: &mpsc::Receiver<String>, seen: &mut Vec<u64>, last: u64) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while seen.last() != Some(&last) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = lines
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("follow printed {seen:?}, never reached {last}"));
        let event: Value = serde_json::from_str(&line).unwrap();
        seen.push(event["cursor"].as_u64().unwrap());
    }
}

#[test]
fn events_tail_follow_reconnects_across_a_daemon_restart_without_duplicates() {
    let root = temp_drive();
    let (daemon, log) = start_daemon(root.path());
    log.append(scope(), task("t1")).unwrap(); // 2 (1 is daemon.started)
    let (mut child, lines) = spawn_lines(&drive_args(
        &["--json", "events", "tail", "--follow"],
        root.path(),
    ));
    let mut seen = Vec::new();
    read_through(&lines, &mut seen, 2);
    log.append(scope(), task("t2")).unwrap(); // 3, delivered live
    read_through(&lines, &mut seen, 3);

    drop(log);
    daemon.stop(); // 4: daemon.stopping
    daemon.wait().unwrap();
    let (restarted, log) = start_daemon(root.path()); // 5: daemon.started
    log.append(scope(), task("t3")).unwrap(); // 6
    read_through(&lines, &mut seen, 6);
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(seen, [1, 2, 3, 4, 5, 6]);
    restarted.stop();
    restarted.wait().unwrap();
}
