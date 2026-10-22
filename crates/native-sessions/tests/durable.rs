use aidex_native_sessions::{
    NATIVE_RECORD_KEY_SECRET, Run, RunOptions, RunStatus, SessionConfig, SessionDetail,
    SessionStore, TaskGraph, TaskGraphStatus, TaskNode, TaskNodeStatus, TaskRunRef,
    TaskVerification, WorkerRole,
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

fn graph(status: TaskGraphStatus, node_status: TaskNodeStatus, verifier: bool) -> TaskGraph {
    TaskGraph {
        id: "graph-1".into(),
        root_run_id: "run-1".into(),
        status,
        max_parallel: 2,
        max_steps: 8,
        steps_used: 0,
        nodes: vec![TaskNode {
            id: "node-1".into(),
            prompt: "work".into(),
            role: if verifier {
                WorkerRole::Verifier
            } else {
                WorkerRole::Worker
            },
            depends_on: vec![],
            run_id: None,
            status: node_status,
            input_artifacts: vec![],
            output_artifacts: vec![],
            step_budget: 4,
            steps_used: 0,
            verification: if verifier {
                Some(TaskVerification {
                    passed: true,
                    summary: "verified".into(),
                })
            } else {
                None
            },
            error: None,
        }],
    }
}

#[test]
fn legacy_session_detail_without_task_graphs_defaults_empty() {
    let (_dir, _vault, store) = fixture();
    let session = store.create(config()).unwrap();
    let mut value = serde_json::to_value(session).unwrap();
    value.as_object_mut().unwrap().remove("task_graphs");
    let decoded: SessionDetail = serde_json::from_value(value).unwrap();
    assert!(decoded.task_graphs.is_empty());
}

#[test]
fn task_graph_validation_rejects_invalid_structure_and_bounds() {
    let valid = graph(TaskGraphStatus::Running, TaskNodeStatus::Pending, false);
    let mut invalid = valid.clone();
    invalid.nodes.push(invalid.nodes[0].clone());
    assert!(invalid.validate().is_err(), "duplicate node ids");

    let mut invalid = valid.clone();
    invalid.nodes[0].depends_on.push("missing".into());
    assert!(invalid.validate().is_err(), "missing dependency");

    let mut invalid = valid.clone();
    let mut second = invalid.nodes[0].clone();
    second.id = "node-2".into();
    invalid.nodes[0].depends_on.push("node-2".into());
    second.depends_on.push("node-1".into());
    invalid.nodes.push(second);
    assert!(invalid.validate().is_err(), "dependency cycle");

    for (max_parallel, max_steps) in [(0, 1), (5, 1), (1, 0), (1, 129)] {
        let mut invalid = valid.clone();
        invalid.max_parallel = max_parallel;
        invalid.max_steps = max_steps;
        assert!(
            invalid.validate().is_err(),
            "limits {max_parallel}/{max_steps}"
        );
    }

    let completed_without_verifier =
        graph(TaskGraphStatus::Completed, TaskNodeStatus::Completed, false);
    assert!(
        completed_without_verifier.validate().is_err(),
        "missing verifier"
    );

    let mut completed_without_verification =
        graph(TaskGraphStatus::Completed, TaskNodeStatus::Completed, true);
    completed_without_verification.nodes[0].verification = None;
    assert!(
        completed_without_verification.validate().is_err(),
        "missing verifier result"
    );

    let mut completed_with_failed_verification =
        graph(TaskGraphStatus::Completed, TaskNodeStatus::Completed, true);
    completed_with_failed_verification.nodes[0]
        .verification
        .as_mut()
        .unwrap()
        .passed = false;
    assert!(
        completed_with_failed_verification.validate().is_err(),
        "failed verifier result"
    );

    for node_status in [
        TaskNodeStatus::Pending,
        TaskNodeStatus::Running,
        TaskNodeStatus::Failed,
        TaskNodeStatus::Interrupted,
        TaskNodeStatus::Cancelled,
    ] {
        let invalid = graph(TaskGraphStatus::Completed, node_status, true);
        assert!(
            invalid.validate().is_err(),
            "incomplete node status {node_status:?}"
        );
    }

    assert!(
        graph(TaskGraphStatus::Completed, TaskNodeStatus::Completed, true)
            .validate()
            .is_ok()
    );
}

#[test]
fn reopening_interrupts_active_graphs_and_nodes_only() {
    let (dir, vault, store) = fixture();
    let session = store.create(config()).unwrap();
    let root = store
        .begin(&session.id, "root", "root-command", None)
        .unwrap();
    store
        .update(&session.id, |detail| {
            detail.runs[0].status = RunStatus::Running;
            detail.task_graphs = vec![
                graph(TaskGraphStatus::Pending, TaskNodeStatus::Pending, false),
                graph(TaskGraphStatus::Running, TaskNodeStatus::Running, false),
                graph(TaskGraphStatus::Completed, TaskNodeStatus::Completed, true),
                graph(TaskGraphStatus::Failed, TaskNodeStatus::Failed, false),
            ];
            detail.task_graphs[1].id = "graph-2".into();
            detail.task_graphs[2].id = "graph-3".into();
            detail.task_graphs[3].id = "graph-4".into();
            for task_graph in &mut detail.task_graphs {
                task_graph.root_run_id = root.id.clone();
            }
            Ok(())
        })
        .unwrap();
    drop(store);
    let reopened = SessionStore::open(dir.path().join("sessions.sqlite3"), vault).unwrap();
    let graphs = reopened.get(&session.id).unwrap().task_graphs;
    assert_eq!(graphs[0].status, TaskGraphStatus::Interrupted);
    assert_eq!(graphs[0].nodes[0].status, TaskNodeStatus::Interrupted);
    assert_eq!(graphs[1].status, TaskGraphStatus::Interrupted);
    assert_eq!(graphs[1].nodes[0].status, TaskNodeStatus::Interrupted);
    assert_eq!(graphs[2].status, TaskGraphStatus::Completed);
    assert_eq!(graphs[2].nodes[0].status, TaskNodeStatus::Completed);
    assert_eq!(graphs[3].status, TaskGraphStatus::Failed);
    assert_eq!(graphs[3].nodes[0].status, TaskNodeStatus::Failed);
}

#[test]
fn inference_retry_rejects_settled_task_child_without_changing_counters() {
    let (_dir, _vault, store) = fixture();
    let session = store.create(config()).unwrap();
    let root = store
        .begin(&session.id, "root", "root-command", None)
        .unwrap();
    store
        .update(&session.id, |detail| {
            detail.runs[0].status = RunStatus::Running;
            detail.task_graphs = vec![graph(
                TaskGraphStatus::Running,
                TaskNodeStatus::Pending,
                false,
            )];
            detail.task_graphs[0].root_run_id = root.id.clone();
            Ok(())
        })
        .unwrap();
    let child = store
        .begin_with(
            &session.id,
            "child",
            "child-command",
            Some(root.id.clone()),
            &RunOptions {
                task: Some(TaskRunRef {
                    graph_id: "graph-1".into(),
                    node_id: "node-1".into(),
                }),
                ..RunOptions::default()
            },
        )
        .unwrap();
    store
        .update(&session.id, |detail| {
            detail
                .runs
                .iter_mut()
                .find(|run| run.id == child.id)
                .unwrap()
                .status = RunStatus::Running;
            detail.task_graphs[0].status = TaskGraphStatus::Failed;
            detail.task_graphs[0].nodes[0].status = TaskNodeStatus::Failed;
            detail.task_graphs[0].steps_used = 2;
            detail.task_graphs[0].nodes[0].steps_used = 2;
            Ok(())
        })
        .unwrap();

    let error = store.reserve_inference_retry(&child.id).unwrap_err();
    assert!(error.contains("task"), "{error}");
    let saved = store.get(&session.id).unwrap();
    let saved_child = saved.runs.iter().find(|run| run.id == child.id).unwrap();
    assert_eq!((saved_child.steps, saved_child.inference_retries), (0, 0));
    assert_eq!(saved.task_graphs[0].steps_used, 2);
    assert_eq!(saved.task_graphs[0].nodes[0].steps_used, 2);
}
