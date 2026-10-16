//! `vault.*` over authenticated IPC against an in-process daemon on a temp drive.

mod common;

use aidex_vault::{ConsumerScope, KdfParams, SecretName};
use aidexd::{
    Client, ClientError, ErrorCode,
    methods::vault::{
        DeleteResult, INIT, InitResult, LOCK, LockResult, PASSPHRASE_CHANGE, SECRET_DELETE,
        SECRET_LIST, SECRET_SET, STATUS, SecretList, SecretMeta, UNLOCK, UnlockMethod, VaultState,
        VaultStatus,
    },
};
use common::{options, start};
use serde_json::{Value, json};
use std::{fs, sync::Arc, time::Duration};

const PASSPHRASE: &str = "correct horse battery staple";
const VALUE: &str = "PLAINTEXT-MARKER-VALUE-ipc-55e1";

fn rpc_kind(error: ClientError) -> (Option<ErrorCode>, Option<Value>) {
    let kind = error.rpc_kind();
    match error {
        ClientError::Rpc { data, .. } => (kind, data),
        other => panic!("expected an RPC error, got {other}"),
    }
}

fn status(client: &mut Client) -> VaultStatus {
    client.call_as(STATUS, Value::Null).unwrap()
}

#[test]
fn vault_methods_drive_the_full_lifecycle_over_ipc() {
    let root = common::temp_drive();
    let mut captured = None;
    let daemon = start(root.path(), options(), |daemon| {
        let vault = Arc::clone(daemon.services().vault());
        // Tiny Argon2id cost keeps debug-build tests fast; production uses the default.
        vault.set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
        captured = Some(vault);
    });
    let vault = captured.expect("vault service captured before start");
    let mut client = Client::connect(root.path(), "test-vault").unwrap();
    assert_eq!(status(&mut client).state, VaultState::Missing);

    let error = client.call(SECRET_LIST, Value::Null).unwrap_err();
    assert_eq!(rpc_kind(error).0, Some(ErrorCode::NotFound));

    let init: InitResult = client
        .call_as(INIT, json!({"passphrase": PASSPHRASE, "recovery": true}))
        .unwrap();
    assert_eq!(init.state, VaultState::Unlocked);
    let recovery = init
        .recovery_key
        .clone()
        .expect("recovery key returned once");
    assert_eq!(recovery.split('-').count(), 14);
    let error = client
        .call(INIT, json!({"passphrase": PASSPHRASE}))
        .unwrap_err();
    assert_eq!(rpc_kind(error).0, Some(ErrorCode::Conflict));

    let meta: SecretMeta = client
        .call_as(
            SECRET_SET,
            json!({
                "name": "openai",
                "value": VALUE,
                "consumers": ["provider:openai", "mcp:github"],
                "kind": "api_key",
            }),
        )
        .unwrap();
    assert_eq!(meta.name.as_str(), "openai");
    assert_eq!(meta.policy.allowed_consumers.len(), 2);
    let listed: SecretList = client.call_as(SECRET_LIST, Value::Null).unwrap();
    assert_eq!(listed.secrets, std::slice::from_ref(&meta));
    assert!(listed.managed.is_empty());
    let raw = client.call(SECRET_LIST, Value::Null).unwrap();
    assert!(!raw.to_string().contains(VALUE), "{raw}");
    // Records AiDEX owns are refused before the vault is touched.
    for (method, params) in [
        (
            SECRET_SET,
            json!({"name": "oauth.openai.access", "value": VALUE, "consumers": ["provider:openai"]}),
        ),
        (
            SECRET_DELETE,
            json!({"name": "native-record-encryption-key"}),
        ),
    ] {
        let (kind, data) = rpc_kind(client.call(method, params).unwrap_err());
        assert_eq!(kind, Some(ErrorCode::PolicyDenied), "{method}");
        assert_eq!(data, Some(json!({"reason": "managed_secret"})), "{method}");
    }
    let current = status(&mut client);
    assert_eq!(
        (current.state, current.secrets, current.unlocked_by),
        (
            VaultState::Unlocked,
            Some(1),
            Some(UnlockMethod::Passphrase)
        )
    );
    assert_eq!(current.vault_id.unwrap().as_str(), init.vault_id);

    // The broker-facing handle API is Rust-only: it resolves in-process...
    let openai = ConsumerScope::provider("openai").unwrap();
    let handle = vault
        .issue_handle(
            &SecretName::new("openai").unwrap(),
            &openai,
            Duration::from_secs(30),
        )
        .unwrap();
    assert_eq!(
        vault.resolve(&handle, &openai).unwrap().expose(),
        VALUE.as_bytes()
    );
    // ...and is not reachable over IPC.
    for method in [
        "vault.handle.issue",
        "vault.issue_handle",
        "vault.resolve",
        "vault.secret.get",
    ] {
        let error = client.call(method, json!({})).unwrap_err();
        assert_eq!(
            rpc_kind(error).0,
            Some(ErrorCode::MethodNotFound),
            "{method}"
        );
    }

    let locked: LockResult = client.call_as(LOCK, Value::Null).unwrap();
    assert_eq!(
        (locked.state, locked.was_unlocked, locked.revoked_handles),
        (VaultState::Locked, true, 1)
    );
    assert!(vault.resolve(&handle, &openai).is_err());
    let (kind, data) = rpc_kind(client.call(SECRET_LIST, Value::Null).unwrap_err());
    assert_eq!(kind, Some(ErrorCode::VaultLocked));
    assert_eq!(data, Some(json!({"reason": "locked"})));
    let (kind, data) = rpc_kind(
        client
            .call(UNLOCK, json!({"passphrase": "definitely wrong"}))
            .unwrap_err(),
    );
    assert_eq!(kind, Some(ErrorCode::VaultLocked));
    assert_eq!(data, Some(json!({"reason": "wrong_passphrase"})));

    let unlocked: VaultStatus = client
        .call_as(UNLOCK, json!({"recovery_key": recovery.to_lowercase()}))
        .unwrap();
    assert_eq!(unlocked.unlocked_by, Some(UnlockMethod::RecoveryKey));
    client
        .call(
            PASSPHRASE_CHANGE,
            json!({"new_passphrase": "a new passphrase"}),
        )
        .unwrap();
    client.call(LOCK, Value::Null).unwrap();
    let unlocked: VaultStatus = client
        .call_as(UNLOCK, json!({"passphrase": "a new passphrase"}))
        .unwrap();
    assert_eq!(unlocked.unlocked_by, Some(UnlockMethod::Passphrase));
    let (kind, _) = rpc_kind(
        client
            .call(
                PASSPHRASE_CHANGE,
                json!({"new_passphrase": "no old passphrase"}),
            )
            .unwrap_err(),
    );
    assert_eq!(kind, Some(ErrorCode::PermissionDenied));
    client
        .call(
            PASSPHRASE_CHANGE,
            json!({"old_passphrase": "a new passphrase", "new_passphrase": PASSPHRASE}),
        )
        .unwrap();

    let deleted: DeleteResult = client
        .call_as(SECRET_DELETE, json!({"name": "openai"}))
        .unwrap();
    assert_eq!(deleted.deleted.secret_ref, meta.secret_ref);
    let (kind, _) = rpc_kind(
        client
            .call(SECRET_DELETE, json!({"name": "openai"}))
            .unwrap_err(),
    );
    assert_eq!(kind, Some(ErrorCode::NotFound));
    drop(client);
    daemon.stop();
    daemon.wait().unwrap();

    // The vault persists across daemon restarts and starts locked.
    let restarted = start(root.path(), options(), |_| {});
    let mut client = Client::connect(root.path(), "test-vault").unwrap();
    assert_eq!(status(&mut client).state, VaultState::Locked);
    client
        .call(UNLOCK, json!({"passphrase": PASSPHRASE}))
        .unwrap();
    let listed: SecretList = client.call_as(SECRET_LIST, Value::Null).unwrap();
    assert!(listed.secrets.is_empty());
    drop(client);
    restarted.stop();
    restarted.wait().unwrap();

    // Nothing the client sent is stored in plaintext on the drive.
    let vault_dir = aidex_drive::DrivePaths::open(root.path()).unwrap().vault();
    for entry in walk(&vault_dir) {
        let bytes = fs::read(&entry).unwrap();
        for needle in [VALUE, PASSPHRASE, "openai", recovery.as_str()] {
            assert!(
                !bytes
                    .windows(needle.len())
                    .any(|window| window == needle.as_bytes()),
                "{} contains {needle}",
                entry.display()
            );
        }
    }
}

#[test]
fn damaged_vault_does_not_stop_the_daemon() {
    let root = common::temp_drive();
    let vault_dir = aidex_drive::DrivePaths::open(root.path()).unwrap().vault();
    fs::write(vault_dir.join("vault.json"), b"{ not a vault").unwrap();
    let daemon = start(root.path(), options(), |_| {});
    let mut client = Client::connect(root.path(), "test-vault").unwrap();
    let current = status(&mut client);
    assert_eq!(current.state, VaultState::Unavailable);
    assert_eq!(current.reason.as_deref(), Some("corrupt"));
    let (kind, data) = rpc_kind(
        client
            .call(UNLOCK, json!({"passphrase": PASSPHRASE}))
            .unwrap_err(),
    );
    assert_eq!(kind, Some(ErrorCode::InternalError));
    assert_eq!(data, Some(json!({"reason": "corrupt"})));
    drop(client);
    daemon.stop();
    daemon.wait().unwrap();
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files.extend(walk(&entry.path()));
        } else {
            files.push(entry.path());
        }
    }
    files
}
