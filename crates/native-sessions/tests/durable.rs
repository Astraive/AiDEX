use aidex_native_sessions::{
    NATIVE_RECORD_KEY_SECRET, Run, RunStatus, SessionConfig, SessionStore,
};
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use std::sync::Arc;

fn fixture() -> (tempfile::TempDir, Arc<VaultService>, SessionStore) {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(
            dir.path().join("vault"),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(&SecretString::from("fixture-passphrase".to_owned()), false)
        .unwrap();
    let store = SessionStore::open(dir.path().join("sessions.sqlite3"), vault.clone()).unwrap();
    (dir, vault, store)
}
fn config() -> SessionConfig {
    serde_json::from_value(serde_json::json!({"title":"private fixture title","mode":"chat","provider_id":"fixture","model_id":"small"})).unwrap()
}

#[test]
fn history_is_encrypted_locked_and_reloaded() {
    let (dir, vault, store) = fixture();
    let session = store.create(config()).unwrap();
    let run = store
        .begin(&session.id, "private fixture prompt", "command-1", None)
        .unwrap();
    store
        .update(&session.id, |s| {
            s.runs[0].status = RunStatus::Running;
            Ok(())
        })
        .unwrap();
    let bytes = std::fs::read(dir.path().join("sessions.sqlite3")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("private fixture"));
    vault.lock();
    assert!(store.get(&session.id).is_err());
    assert!(store.list().unwrap()[0].title.is_none());
    drop(store);
    let reopened = SessionStore::open(dir.path().join("sessions.sqlite3"), vault.clone()).unwrap();
    vault
        .unlock(&SecretString::from("fixture-passphrase".to_owned()))
        .unwrap();
    let detail = reopened.get(&session.id).unwrap();
    assert_eq!(detail.messages[0].content, "private fixture prompt");
    assert_eq!(detail.runs[0].id, run.id);
    assert_eq!(detail.runs[0].status, RunStatus::Interrupted);
}
#[test]
fn duplicate_message_command_returns_original_run() {
    let (_drive, _vault, store) = fixture();
    let s = store.create(config()).unwrap();
    let a = store.begin(&s.id, "hello", "once", None).unwrap();
    let b = store.begin(&s.id, "hello", "once", None).unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(store.get(&s.id).unwrap().messages.len(), 1);
    assert!(store.begin(&s.id, "different", "once", None).is_err());
}
#[test]
fn memory_is_private_scoped_durable_and_locked() {
    let (dir, vault, store) = fixture();
    let record = store
        .memory_set("workspace-a", "notes", "private memory fixture")
        .unwrap();
    assert_eq!(
        store.memory_list("workspace-a").unwrap()[0].content,
        "private memory fixture"
    );
    assert!(store.memory_list("workspace-b").unwrap().is_empty());
    assert!(
        !String::from_utf8_lossy(&std::fs::read(dir.path().join("sessions.sqlite3")).unwrap())
            .contains("private memory fixture")
    );
    vault.lock();
    assert!(store.memory_list("workspace-a").is_err());
    vault
        .unlock(&SecretString::from("fixture-passphrase".to_owned()))
        .unwrap();
    drop(store);
    let reopened = SessionStore::open(dir.path().join("sessions.sqlite3"), vault).unwrap();
    assert_eq!(
        reopened.memory_list("workspace-a").unwrap()[0].id,
        record.id
    );
    assert!(reopened.memory_delete("workspace-a", &record.id).unwrap());
    assert!(reopened.memory_list("workspace-a").unwrap().is_empty());
}
#[test]
fn instructions_are_private_and_invalid_fallback_is_refused() {
    let (_drive, _vault, store) = fixture();
    let mut value = serde_json::to_value(config()).unwrap();
    value["instructions"] = serde_json::json!("user guidance");
    let parsed: SessionConfig = serde_json::from_value(value.clone()).unwrap();
    let s = store.create(parsed).unwrap();
    assert_eq!(
        serde_json::to_value(store.get(&s.id).unwrap()).unwrap()["instructions"],
        "user guidance"
    );
    value["fallback_model"] = serde_json::json!("invalid reference");
    let parsed: SessionConfig = serde_json::from_value(value).unwrap();
    assert!(store.create(parsed).is_err());
}
#[test]
fn restarting_a_waiting_run_marks_it_interrupted() {
    let (dir, vault, store) = fixture();
    let s = store.create(config()).unwrap();
    let r = store.begin(&s.id, "pending action", "once", None).unwrap();
    store
        .update(&s.id, |s| {
            s.runs[0].status = RunStatus::WaitingApproval;
            Ok(())
        })
        .unwrap();
    drop(store);
    let reopened = SessionStore::open(dir.path().join("sessions.sqlite3"), vault).unwrap();
    assert_eq!(reopened.run(&r.id).unwrap().status, RunStatus::Interrupted);
}
#[test]
fn record_key_is_stored_under_the_exported_name() {
    let (_drive, vault, store) = fixture();
    assert_eq!(NATIVE_RECORD_KEY_SECRET, "native-record-encryption-key");
    store.create(config()).unwrap();
    let name = aidex_vault::SecretName::new(NATIVE_RECORD_KEY_SECRET).unwrap();
    assert_eq!(vault.list_secrets().unwrap().len(), 1);
    assert_eq!(vault.secret_meta(&name).unwrap().name, name);
}
#[test]
fn run_counters_are_optional_and_omitted_while_zero() {
    let (_drive, _vault, store) = fixture();
    let s = store.create(config()).unwrap();
    let run = store.begin(&s.id, "hello", "once", None).unwrap();
    let mut wire = serde_json::to_value(&run).unwrap();
    assert!(wire.get("inference_retries").is_none(), "{wire}");
    assert!(wire.get("context_trimmed_messages").is_none(), "{wire}");
    // A run saved before the counters existed decodes with zeros.
    let decoded: Run = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(
        (decoded.inference_retries, decoded.context_trimmed_messages),
        (0, 0)
    );
    wire["inference_retries"] = 2.into();
    wire["context_trimmed_messages"] = 7.into();
    let decoded: Run = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(
        (decoded.inference_retries, decoded.context_trimmed_messages),
        (2, 7)
    );
    assert_eq!(serde_json::to_value(&decoded).unwrap(), wire);
}
#[test]
fn inference_retries_use_the_run_step_budget() {
    let (dir, vault, store) = fixture();
    let s = store.create(config()).unwrap();
    let run = store.begin(&s.id, "hello", "once", None).unwrap();
    store
        .update(&s.id, |s| {
            s.configuration.max_steps = 3;
            s.runs[0].status = RunStatus::Running;
            s.runs[0].steps = 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(store.reserve_inference_retry(&run.id).unwrap(), 1);
    assert_eq!(store.reserve_inference_retry(&run.id).unwrap(), 2);
    let error = store.reserve_inference_retry(&run.id).unwrap_err();
    assert!(error.contains("step budget"), "{error}");
    let saved = store.run(&run.id).unwrap();
    assert_eq!((saved.steps, saved.inference_retries), (3, 2));
    // A cancelled run takes no further retry, even with budget left.
    store
        .update(&s.id, |s| {
            s.configuration.max_steps = 8;
            s.runs[0].status = RunStatus::Cancelled;
            Ok(())
        })
        .unwrap();
    let error = store.reserve_inference_retry(&run.id).unwrap_err();
    assert!(error.contains("no longer active"), "{error}");
    assert!(store.reserve_inference_retry("run_missing").is_err());
    drop(store);
    let reopened = SessionStore::open(dir.path().join("sessions.sqlite3"), vault).unwrap();
    let saved = reopened.run(&run.id).unwrap();
    assert_eq!((saved.steps, saved.inference_retries), (3, 2));
}
#[test]
fn context_trims_keep_the_largest_count() {
    let (_drive, _vault, store) = fixture();
    let s = store.create(config()).unwrap();
    let run = store.begin(&s.id, "hello", "once", None).unwrap();
    store.record_context_trim(&run.id, 3).unwrap();
    store.record_context_trim(&run.id, 1).unwrap();
    assert_eq!(store.run(&run.id).unwrap().context_trimmed_messages, 3);
    store.record_context_trim(&run.id, 5).unwrap();
    assert_eq!(store.run(&run.id).unwrap().context_trimmed_messages, 5);
    // Zero is a no-op that needs no lookup; a real count needs a known run.
    store.record_context_trim("run_missing", 0).unwrap();
    assert!(store.record_context_trim("run_missing", 1).is_err());
}
