//! `aidex vault ...` end to end: secrets come only from stdin or the terminal, wrong
//! credentials and a locked vault exit 5, and values are never printed.

use aidex_vault::KdfParams;
use aidexd::{Daemon, DaemonHandle};
use serde_json::Value;
use std::{
    io::{self, Write},
    path::Path,
    process::{Child, Command, Output, Stdio},
};

const PASSPHRASE: &str = "correct horse battery staple";
const VALUE: &str = "PLAINTEXT-MARKER-VALUE-cli-0c3d";

fn aidex(args: &[&str], root: &Path, stdin: Option<&str>) -> Output {
    let mut child = spawn_aidex(args, root, stdin.is_some());
    let written = if let Some(input) = stdin {
        let mut pipe = child.stdin.take().expect("stdin pipe");
        pipe.write_all(input.as_bytes())
    } else {
        Ok(())
    };
    wait_for_aidex(child, written)
}

fn spawn_aidex(args: &[&str], root: &Path, piped_stdin: bool) -> Child {
    Command::new(env!("CARGO_BIN_EXE_aidex"))
        .args(args)
        .args(["--drive", root.to_str().expect("utf-8 temp path")])
        .env_remove("AIDEX_DRIVE_ROOT")
        .env("AIDEX_OMP", "aidex-test-missing-omp-executable")
        .stdin(if piped_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run aidex")
}

fn wait_for_aidex(child: Child, written: io::Result<()>) -> Output {
    let output = child.wait_with_output().expect("wait for aidex");
    if let Err(error) = written {
        // Rejection can close stdin before our write. Successful commands must
        // consume the complete input; every other write failure remains an error.
        assert!(
            error.kind() == io::ErrorKind::BrokenPipe && !output.status.success(),
            "write stdin: {error}"
        );
    }
    output
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

/// In-process daemon with a tiny Argon2id cost (debug-build speed; never production).
fn daemon(root: &Path) -> DaemonHandle {
    let daemon = Daemon::new(root).unwrap();
    daemon
        .services()
        .vault()
        .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
    daemon.start().unwrap()
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

fn json(output: &Output) -> Value {
    assert_eq!(code(output), 0, "{}", text(output));
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", text(output)))
}

fn assert_quiet(output: &Output, secrets: &[&str]) {
    let all = text(output);
    for secret in secrets {
        assert!(!all.contains(secret), "output leaked {secret}: {all}");
    }
}

#[test]
fn vault_commands_manage_secrets_through_the_daemon() {
    let root = temp_drive();
    let drive = root.path();
    let handle = daemon(drive);

    assert_eq!(
        json(&aidex(&["--json", "vault", "status"], drive, None))["state"],
        "missing"
    );

    let init = aidex(
        &[
            "--json",
            "vault",
            "init",
            "--recovery-key",
            "--passphrase-stdin",
        ],
        drive,
        Some(&format!("{PASSPHRASE}\n")),
    );
    assert_quiet(&init, &[PASSPHRASE]);
    let init = json(&init);
    assert_eq!(init["state"], "unlocked");
    let recovery = init["recovery_key"].as_str().unwrap().to_owned();
    assert_eq!(recovery.split('-').count(), 14);

    let set = aidex(
        &[
            "--json",
            "vault",
            "set",
            "openai",
            "--consumer",
            "provider:openai",
            "--consumer",
            "mcp:github",
            "--kind",
            "api_key",
            "--from-stdin",
        ],
        drive,
        Some(&format!("{VALUE}\n")),
    );
    assert_quiet(&set, &[VALUE]);
    let meta = json(&set);
    assert_eq!(meta["name"], "openai");
    assert_eq!(meta["kind"], "api_key");
    assert_eq!(
        meta["policy"]["allowed_consumers"],
        serde_json::json!(["provider:openai", "mcp:github"])
    );

    let forbidden = aidex(
        &[
            "vault",
            "set",
            "x",
            "--consumer",
            "model:gpt",
            "--from-stdin",
        ],
        drive,
        Some(VALUE),
    );
    assert_eq!(code(&forbidden), 6, "{}", text(&forbidden));
    assert_quiet(&forbidden, &[VALUE]);

    let listed = aidex(&["--json", "vault", "list"], drive, None);
    assert_quiet(&listed, &[VALUE]);
    let secrets = json(&listed)["secrets"].as_array().unwrap().clone();
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0]["name"], "openai");
    let human = aidex(&["vault", "list"], drive, None);
    assert_eq!(code(&human), 0);
    assert!(
        text(&human).contains("openai  api_key  provider:openai, mcp:github"),
        "{}",
        text(&human)
    );
    assert_quiet(&human, &[VALUE]);

    let locked = json(&aidex(&["--json", "vault", "lock"], drive, None));
    assert_eq!(locked["state"], "locked");
    let denied = aidex(&["vault", "list"], drive, None);
    assert_eq!(code(&denied), 5, "{}", text(&denied));
    assert!(
        text(&denied).contains("aidex vault unlock"),
        "{}",
        text(&denied)
    );

    let wrong = aidex(
        &["--json", "vault", "unlock", "--passphrase-stdin"],
        drive,
        Some("not the passphrase\n"),
    );
    assert_eq!(code(&wrong), 5, "{}", text(&wrong));
    assert_quiet(&wrong, &["not the passphrase"]);
    let error: Value = serde_json::from_slice(&wrong.stderr).unwrap();
    assert_eq!(error["error"]["code"], "auth_failed");

    let unlocked = aidex(
        &[
            "--json",
            "vault",
            "unlock",
            "--recovery-key",
            "--passphrase-stdin",
        ],
        drive,
        Some(&format!("{}\n", recovery.to_lowercase())),
    );
    assert_quiet(&unlocked, &[&recovery]);
    assert_eq!(json(&unlocked)["unlocked_by"], "recovery_key");

    // After a recovery-key unlock only the new passphrase is read.
    let changed = aidex(
        &["vault", "passphrase", "change", "--passphrase-stdin"],
        drive,
        Some("first new passphrase\n"),
    );
    assert_eq!(code(&changed), 0, "{}", text(&changed));
    assert_quiet(&changed, &["first new passphrase"]);
    assert_eq!(code(&aidex(&["vault", "lock"], drive, None)), 0);
    let unlocked = aidex(
        &["vault", "unlock", "--passphrase-stdin"],
        drive,
        Some("first new passphrase\n"),
    );
    assert_eq!(code(&unlocked), 0, "{}", text(&unlocked));
    assert!(
        text(&unlocked).contains("Vault: unlocked"),
        "{}",
        text(&unlocked)
    );
    let changed = aidex(
        &[
            "--json",
            "vault",
            "passphrase",
            "change",
            "--passphrase-stdin",
        ],
        drive,
        Some("first new passphrase\nsecond new passphrase\n"),
    );
    assert_eq!(json(&changed)["changed"], true);

    let deleted = json(&aidex(
        &["--json", "vault", "delete", "openai"],
        drive,
        None,
    ));
    assert_eq!(deleted["deleted"]["name"], "openai");
    let missing = aidex(&["vault", "delete", "openai"], drive, None);
    assert_eq!(code(&missing), 8, "{}", text(&missing));

    handle.stop();
    handle.wait().unwrap();
}

#[test]
fn fast_argument_rejection_closes_stdin_without_persisting_the_secret() {
    let root = temp_drive();
    let handle = daemon(root.path());
    json(&aidex(
        &["--json", "vault", "init", "--passphrase-stdin"],
        root.path(),
        Some(&format!("{PASSPHRASE}\n")),
    ));

    // A missing consumer is rejected by argument parsing before stdin is read.
    // Keep our pipe across wait() so the write deterministically happens after exit.
    let mut child = spawn_aidex(
        &["vault", "set", "denied", "--from-stdin"],
        root.path(),
        true,
    );
    let mut pipe = child.stdin.take().expect("stdin pipe");
    assert_eq!(
        child.wait().expect("wait for argument rejection").code(),
        Some(2)
    );
    let written = pipe.write_all(VALUE.as_bytes());
    drop(pipe);
    assert_eq!(
        written.as_ref().unwrap_err().kind(),
        io::ErrorKind::BrokenPipe
    );

    let output = wait_for_aidex(child, written);
    assert_eq!(code(&output), 2, "{}", text(&output));
    assert_quiet(&output, &[PASSPHRASE, VALUE]);
    let listed = json(&aidex(&["--json", "vault", "list"], root.path(), None));
    assert_eq!(listed["secrets"], serde_json::json!([]));

    handle.stop();
    handle.wait().unwrap();
}

#[test]
fn secrets_are_never_read_from_argv_or_without_an_explicit_stdin_flag() {
    // No daemon is running: input rules are checked before connecting.
    let root = temp_drive();
    for args in [
        &["vault", "init"][..],
        &["vault", "unlock"],
        &["vault", "unlock", "--recovery-key"],
        &["vault", "set", "name", "--consumer", "provider:openai"],
        &["vault", "passphrase", "change"],
    ] {
        let output = aidex(args, root.path(), None);
        assert_eq!(code(&output), 2, "{args:?}: {}", text(&output));
        assert_quiet(&output, &[PASSPHRASE, VALUE]);
        assert!(
            text(&output).contains("stdin"),
            "{args:?}: {}",
            text(&output)
        );
    }
    for args in [
        &["vault", "unlock", "--passphrase", PASSPHRASE][..],
        &["vault", "init", "--passphrase", PASSPHRASE],
        &[
            "vault",
            "set",
            "name",
            "--consumer",
            "provider:openai",
            "--value",
            VALUE,
        ],
        &["vault", "set", "name", "--from-stdin"],
    ] {
        let output = aidex(args, root.path(), None);
        assert_eq!(code(&output), 2, "{args:?}: {}", text(&output));
        assert_quiet(&output, &[PASSPHRASE, VALUE]);
    }
    // With the flag but no daemon, the failure is "daemon unavailable", not a prompt.
    let output = aidex(
        &["vault", "unlock", "--passphrase-stdin"],
        root.path(),
        Some(PASSPHRASE),
    );
    assert_eq!(code(&output), 3, "{}", text(&output));
    assert_quiet(&output, &[PASSPHRASE]);
}
