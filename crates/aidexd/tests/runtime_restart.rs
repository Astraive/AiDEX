//! Configured external runtimes survive a daemon restart: once the vault
//! unlocks, the engine has them again without any other call.

mod common;

use aidex_vault::KdfParams;
use aidexd::Client;
use common::{options, start};
use serde_json::{Value, json};

const PASSPHRASE: &str = "fixture passphrase only";

/// The runtimes the native engine runs (`execution.status`).
fn engine_runtimes(client: &mut Client) -> Vec<String> {
    let status = client.call("execution.status", Value::Null).unwrap();
    status["runtime_descriptors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|descriptor| descriptor["id"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn configured_runtimes_reach_the_engine_after_a_restart_and_an_unlock() {
    let root = common::temp_drive();
    let daemon = start(root.path(), options(), |daemon| {
        // Tiny Argon2id cost keeps debug-build tests fast; production uses the default.
        daemon
            .services()
            .vault()
            .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
    });
    let mut client = Client::connect(root.path(), "test-runtime-restart").unwrap();
    client
        .call("vault.init", json!({"passphrase": PASSPHRASE}))
        .unwrap();
    // Offline: nothing probes or launches the runtime.
    client
        .call("network.offline", json!({"enabled": true}))
        .unwrap();
    let executable = root.path().join("missing").join("codex.exe");
    client
        .call(
            "runtime.add",
            json!({"id": "codex-fixture", "driver": "codex", "executable": executable.to_string_lossy()}),
        )
        .unwrap();
    assert_eq!(engine_runtimes(&mut client), ["codex-fixture"]);
    drop(client);
    daemon.stop();
    daemon.wait().unwrap();

    let restarted = start(root.path(), options(), |_| {});
    let mut client = Client::connect(root.path(), "test-runtime-restart").unwrap();
    // The configuration is sealed: a locked vault leaves the engine without it.
    assert!(engine_runtimes(&mut client).is_empty());
    client
        .call("vault.unlock", json!({"passphrase": PASSPHRASE}))
        .unwrap();
    assert_eq!(engine_runtimes(&mut client), ["codex-fixture"]);
    drop(client);
    restarted.stop();
    restarted.wait().unwrap();
}
