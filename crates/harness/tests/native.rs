use aidex_events::MemoryEventSink;
use aidex_harness::NativeEngine;
use aidex_native_sessions::{Mode, RunStatus, SessionStore};
use aidex_providers::{
    CapabilitySource, FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId,
    ModelRef, NoVault, OfflineMode, ProviderId, ProviderRegistry,
};
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

fn fixture(
    turns: Vec<FakeTurn>,
    mode: Mode,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<NativeEngine>,
    String,
    Arc<FakeProvider>,
    Arc<MemoryEventSink>,
) {
    fixture_with_limits(turns, mode, 65536, None)
}
fn fixture_with_limits(
    turns: Vec<FakeTurn>,
    mode: Mode,
    context: u32,
    output: Option<u32>,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<NativeEngine>,
    String,
    Arc<FakeProvider>,
    Arc<MemoryEventSink>,
) {
    fixture_with_source(turns, mode, context, output, CapabilitySource::Configured)
}
fn fixture_with_source(
    turns: Vec<FakeTurn>,
    mode: Mode,
    context: u32,
    output: Option<u32>,
    source: CapabilitySource,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<NativeEngine>,
    String,
    Arc<FakeProvider>,
    Arc<MemoryEventSink>,
) {
    let drive = tempfile::tempdir().unwrap();
    let ws = tempfile::tempdir().unwrap();
    aidex_drive::init_with(
        drive.path(),
        aidex_drive::InitOptions {
            require_removable: false,
        },
    )
    .unwrap();
    let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
    let vault = Arc::new(
        VaultService::open(
            paths.ensure_dir(&paths.vault()).unwrap(),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(&SecretString::from("passphrase".to_string()), false)
        .unwrap();
    let store = Arc::new(
        SessionStore::open(
            paths
                .ensure_dir(&paths.sessions())
                .unwrap()
                .join("native.sqlite3"),
            vault,
        )
        .unwrap(),
    );
    let workspaces = Arc::new(aidex_workspace::WorkspaceRegistry::open(&paths).unwrap());
    let workspace = workspaces.add_host(ws.path(), Some("fixture")).unwrap();
    let registry = Arc::new(
        ProviderRegistry::load(
            &paths,
            Arc::new(NoVault::default()),
            Arc::new(OfflineMode::new(false)),
        )
        .unwrap(),
    );
    let mut descriptor = ModelDescriptor::new(
        ModelRef::new(
            ProviderId::new("fixture").unwrap(),
            ModelId::new("small").unwrap(),
        ),
        Locality::Local,
        context,
    )
    .with_tool_calls(aidex_providers::ToolCallSupport::Native)
    .with_source(source);
    descriptor.max_output_tokens = output;
    let provider = Arc::new(
        FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
            .with_model(descriptor)
            .with_script(turns.into_iter().collect::<FakeScript>()),
    );
    registry.register_shared(provider.clone()).unwrap();
    let events = Arc::new(MemoryEventSink::new());
    let engine = NativeEngine::new(
        store.clone(),
        registry,
        workspaces,
        events.clone(),
        drive.path().join("scratch"),
    );
    let session=store.create(serde_json::from_value(serde_json::json!({"title":"fixture","workspace_id":workspace.id,"mode":mode,"provider_id":"fixture","model_id":"small","permission_mode":"supervised"})).unwrap()).unwrap();
    (drive, ws, engine, session.id, provider, events)
}
fn wait(e: &NativeEngine, id: &str) -> aidex_native_sessions::Run {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let r = e.store().run(id).unwrap();
        if !matches!(r.status, RunStatus::Pending | RunStatus::Running) {
            return r;
        }
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn chat_streams_and_keeps_history_without_offering_tools() {
    let (_drive, _ws, e, s, p, events) = fixture(vec![FakeTurn::text("hello")], Mode::Chat);
    let r = e.start(&s, "hi", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert_eq!(
        e.store().get(&s).unwrap().messages.last().unwrap().content,
        "hello"
    );
    assert!(p.requests()[0].tools.is_empty());
    assert!(events.names().contains(&"agent.delta"));
    assert_eq!(e.start(&s, "hi", "once", None).unwrap().id, r.id);
}
#[test]
fn denied_write_executes_nothing_and_duplicate_response_is_safe() {
    let (_drive, ws, e, s, _provider, _events) = fixture(
        vec![
            FakeTurn::tool_call("fs_write", r#"{"path":"file.txt","content":"new"}"#),
            FakeTurn::text("denied"),
        ],
        Mode::Agent,
    );
    let r = e.start(&s, "write", "once", None).unwrap();
    {
        let stopped = wait(&e, &r.id);
        assert_eq!(
            stopped.status,
            RunStatus::WaitingApproval,
            "{:?}",
            stopped.error
        );
    };
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    e.respond(&a.id, &a.fingerprint, false, "deny").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert!(!ws.path().join("file.txt").exists());
    e.respond(&a.id, &a.fingerprint, false, "deny").unwrap();
    assert!(!ws.path().join("file.txt").exists());
}
#[test]
fn approved_write_conflicts_and_checkpoint_restore_protects_user_edits() {
    let (_drive, ws, e, s, _provider, _events) = fixture(
        vec![
            FakeTurn::tool_call("fs_write", r#"{"path":"file.txt","content":"new"}"#),
            FakeTurn::text("done"),
        ],
        Mode::Agent,
    );
    std::fs::write(ws.path().join("file.txt"), "before").unwrap();
    let r = e.start(&s, "write", "once", None).unwrap();
    wait(&e, &r.id);
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    e.respond(&a.id, &a.fingerprint, true, "allow").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(ws.path().join("file.txt")).unwrap(),
        "new"
    );
    let checkpoint = e.store().get(&s).unwrap().checkpoints.remove(0);
    e.restore(&s, &checkpoint.id, &checkpoint.after_digest)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(ws.path().join("file.txt")).unwrap(),
        "before"
    );
    assert!(
        e.restore(&s, &checkpoint.id, &checkpoint.after_digest)
            .is_err()
    );
    std::fs::write(ws.path().join("file.txt"), "user change").unwrap();
    assert!(
        e.restore(&s, &checkpoint.id, &checkpoint.after_digest)
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(ws.path().join("file.txt")).unwrap(),
        "user change"
    );
}
#[test]
fn cancellation_and_provider_failure_have_terminal_states() {
    let (_drive, _ws, e, s, _provider, _events) = fixture(
        vec![
            FakeTurn::text("late").with_delay(Duration::from_secs(3)),
            FakeTurn::error(aidex_providers::ProviderError::Protocol(
                "fixture failure".into(),
            )),
        ],
        Mode::Chat,
    );
    let r = e.start(&s, "hi", "first", None).unwrap();
    while _provider.requests().is_empty() {
        std::thread::sleep(Duration::from_millis(5));
    }
    e.cancel(&r.id).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Cancelled);
    std::thread::sleep(Duration::from_millis(50));
    let r = e.start(&s, "again", "second", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
}
#[test]
fn path_escape_is_never_executed() {
    let (_drive, _ws, e, s, _provider, _events) = fixture(
        vec![FakeTurn::tool_call(
            "fs_write",
            r#"{"path":"../escape.txt","content":"bad"}"#,
        )],
        Mode::Agent,
    );
    let r = e.start(&s, "write", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
    assert!(e.store().run(&r.id).unwrap().pending_calls.is_empty());
}
#[test]
fn shell_requires_approval() {
    let (_drive, _ws, e, s, _provider, _events) = fixture(
        vec![
            FakeTurn::tool_call("shell_exec", r#"{"shell":true,"command":"echo hello"}"#),
            FakeTurn::text("denied"),
        ],
        Mode::Agent,
    );
    let r = e.start(&s, "shell", "once", None).unwrap();
    {
        let stopped = wait(&e, &r.id);
        assert_eq!(
            stopped.status,
            RunStatus::WaitingApproval,
            "{:?}",
            stopped.error
        );
    };
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    assert_eq!(a.tool_name, "shell_exec");
    e.respond(&a.id, &a.fingerprint, false, "deny").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
}
#[test]
fn child_completion_never_completes_root_and_cancel_cascades() {
    let (_drive, _ws, e, s, p, _events) = fixture(
        vec![
            FakeTurn::text("root").with_delay(Duration::from_secs(3)),
            FakeTurn::text("child"),
            FakeTurn::text("other child").with_delay(Duration::from_secs(3)),
        ],
        Mode::Agent,
    );
    let root = e.start(&s, "parent", "root", None).unwrap();
    while p.requests().is_empty() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let child = e
        .start(&s, "child", "child", Some(root.id.clone()))
        .unwrap();
    assert_eq!(wait(&e, &child.id).status, RunStatus::Completed);
    assert_eq!(e.store().run(&root.id).unwrap().status, RunStatus::Running);
    assert_eq!(e.store().list().unwrap()[0].status, "running");
    assert_eq!(
        e.child_result(&root.id, &child.id).unwrap().content,
        "child"
    );
    assert!(e.child_result("different-parent", &child.id).is_err());
    let second = e
        .start(&s, "second", "second", Some(root.id.clone()))
        .unwrap();
    e.cancel(&root.id).unwrap();
    assert_eq!(wait(&e, &root.id).status, RunStatus::Cancelled);
    assert_eq!(wait(&e, &second.id).status, RunStatus::Cancelled);
    assert_eq!(
        e.store().run(&child.id).unwrap().parent_run_id,
        Some(root.id)
    );
}
#[test]
fn configured_fallback_only_retries_retryable_inference_and_applies_instructions() {
    let (_drive, _ws, e, s, p, events) = fixture(
        vec![
            FakeTurn::error(aidex_providers::ProviderError::unavailable(
                "fixture transient",
            )),
            FakeTurn::text("fallback response"),
        ],
        Mode::Chat,
    );
    e.store()
        .update(&s, |s| {
            s.configuration.fallback_model = Some("fixture/small".into());
            s.configuration.instructions = "Follow user guidance".into();
            Ok(())
        })
        .unwrap();
    let run = e.start(&s, "prompt", "once", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    assert_eq!(p.requests().len(), 2);
    assert!(
        p.requests()[0]
            .messages
            .iter()
            .any(|m| m.content == "Follow user guidance")
    );
    assert!(events.names().contains(&"model.fallback"));
}
#[test]
fn waiting_approval_restart_requires_explicit_resume_without_repeating_effect() {
    let (_drive, ws, e, s, p, _events) = fixture(
        vec![
            FakeTurn::tool_call("fs_write", r#"{"path":"file.txt","content":"new"}"#),
            FakeTurn::text("continued"),
            FakeTurn::text("next turn"),
        ],
        Mode::Agent,
    );
    let run = e.start(&s, "write", "once", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::WaitingApproval);
    let a = e.approvals(Some(&run.id)).unwrap().remove(0);
    let paths = aidex_drive::DrivePaths::open(_drive.path()).unwrap();
    let registry = Arc::new(
        ProviderRegistry::load(
            &paths,
            Arc::new(NoVault::default()),
            Arc::new(OfflineMode::new(false)),
        )
        .unwrap(),
    );
    registry.register_shared(p.clone()).unwrap();
    let e = NativeEngine::new(
        e.store().clone(),
        registry,
        Arc::new(aidex_workspace::WorkspaceRegistry::open(&paths).unwrap()),
        _events.clone(),
        _drive.path().join("restart-scratch"),
    );
    assert!(e.respond(&a.id, &a.fingerprint, true, "approve").is_err());
    e.resume(&run.id).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    assert!(!ws.path().join("file.txt").exists());
    assert_eq!(p.requests().len(), 2);
    let next = e
        .start(&s, "continue the conversation", "next", None)
        .unwrap();
    assert_eq!(wait(&e, &next.id).status, RunStatus::Completed);
    let requests = p.requests();
    let resumed = &requests[1].messages;
    let next_messages = &requests[2].messages;
    let expired_results: Vec<_> = resumed
        .iter()
        .filter(|message| message.tool_call_id.is_some())
        .collect();
    assert!(!expired_results.is_empty());
    for result in expired_results {
        assert!(
            next_messages
                .iter()
                .any(|message| message.tool_call_id == result.tool_call_id
                    && message.content == result.content),
            "next turn must retain the interrupted tool result"
        );
    }
}
#[test]
fn drive_policy_denial_never_executes_an_auto_edit() {
    let (drive, ws, e, s, _p, _events) = fixture(
        vec![FakeTurn::tool_call(
            "fs_write",
            r#"{"path":"file.txt","content":"new"}"#,
        )],
        Mode::Agent,
    );
    e.store()
        .update(&s, |s| {
            s.configuration.permission_mode =
                aidex_native_sessions::PermissionMode::AutoAcceptEdits;
            Ok(())
        })
        .unwrap();
    let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
    std::fs::write(paths.policies().join("deny-write.toml"),"schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"deny-native-writes\"\naction=\"fs.write\"\nverdict=\"deny\"\n").unwrap();
    let r = e.start(&s, "write", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
    assert!(!ws.path().join("file.txt").exists());
}
#[test]
fn approved_file_plan_detects_changes_after_approval_was_requested() {
    let (_drive, ws, e, s, _p, _events) = fixture(
        vec![FakeTurn::tool_call(
            "fs_write",
            r#"{"path":"file.txt","content":"new"}"#,
        )],
        Mode::Agent,
    );
    std::fs::write(ws.path().join("file.txt"), "before").unwrap();
    let r = e.start(&s, "write", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::WaitingApproval);
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    std::fs::write(ws.path().join("file.txt"), "user change").unwrap();
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
    assert_eq!(
        std::fs::read_to_string(ws.path().join("file.txt")).unwrap(),
        "user change"
    );
}
#[cfg(windows)]
#[test]
fn approved_shell_returns_promptly_and_cancel_terminates_the_real_process() {
    // Delay the first marker so approval must return before a workspace effect.
    // Use .NET methods: a fresh sandbox profile has no PowerShell module cache,
    // and cmdlet discovery can scan the runner's large installed-module catalog.
    let (drive, ws, e, s, _p, _events) = fixture(
        vec![
            FakeTurn::tool_call(
                "shell_exec",
                r#"{"shell":true,"command":"[System.Threading.Thread]::Sleep(6000); [System.IO.File]::WriteAllText('started.txt', 'started'); [System.Threading.Thread]::Sleep(10000); [System.IO.File]::WriteAllText('finished.txt', 'finished')"}"#,
            ),
            FakeTurn::text("done"),
        ],
        Mode::Agent,
    );
    e.store()
        .update(&s, |s| {
            s.configuration.permission_mode = aidex_native_sessions::PermissionMode::FullAccess;
            Ok(())
        })
        .unwrap();
    let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
    std::fs::write(paths.policies().join("ask-shell.toml"),"schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"review-shell\"\naction=\"shell.exec\"\nverdict=\"require_approval\"\napproval_scopes=[\"once\"]\n").unwrap();
    let r = e.start(&s, "shell", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::WaitingApproval);
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    let start = Instant::now();
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "approval RPC blocked on shell execution"
    );
    let until = Instant::now() + Duration::from_secs(30);
    while !ws.path().join("started.txt").exists() {
        let state = e.store().run(&r.id).unwrap();
        assert!(state.status.active(), "{:?}", state.error);
        assert!(
            Instant::now() < until,
            "process did not start within 30 seconds: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    e.cancel(&r.id).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Cancelled);
    std::thread::sleep(Duration::from_millis(250));
    assert!(!ws.path().join("finished.txt").exists());
}
#[test]
fn changing_policy_invalidates_an_existing_approval() {
    let (drive, ws, e, s, _p, _events) = fixture(
        vec![FakeTurn::tool_call(
            "fs_write",
            r#"{"path":"file.txt","content":"new"}"#,
        )],
        Mode::Agent,
    );
    let r = e.start(&s, "write", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::WaitingApproval);
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
    std::fs::write(paths.policies().join("deny.toml"),"schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"deny-write\"\naction=\"fs.write\"\nverdict=\"deny\"\n").unwrap();
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
    assert!(!ws.path().join("file.txt").exists());
}
#[test]
fn model_policy_denial_does_not_invoke_or_fallback() {
    let (drive, _ws, e, s, p, _events) =
        fixture(vec![FakeTurn::text("should not run")], Mode::Chat);
    e.store()
        .update(&s, |s| {
            s.configuration.fallback_model = Some("fixture/small".into());
            Ok(())
        })
        .unwrap();
    let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
    std::fs::write(paths.policies().join("deny-model.toml"),"schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"deny-model\"\naction=\"model.invoke\"\nverdict=\"deny\"\n").unwrap();
    let r = e.start(&s, "prompt", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
    assert!(p.requests().is_empty());
}
#[test]
fn step_budget_stops_before_unexecuted_tool() {
    let (_drive, ws, e, s, _p, _events) = fixture(
        vec![FakeTurn::tool_call(
            "fs_write",
            r#"{"path":"file.txt","content":"new"}"#,
        )],
        Mode::Agent,
    );
    e.store()
        .update(&s, |s| {
            s.configuration.max_steps = 1;
            Ok(())
        })
        .unwrap();
    let r = e.start(&s, "write", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
    assert!(!ws.path().join("file.txt").exists());
}
#[test]
fn checkpoint_removes_created_file_and_restores_deleted_file_without_overwriting_user_edits() {
    let (_drive, ws, e, s, _p, _events) = fixture(
        vec![
            FakeTurn::tool_call("fs_write", r#"{"path":"created.txt","content":"new"}"#),
            FakeTurn::text("done"),
        ],
        Mode::Agent,
    );
    let r = e.start(&s, "create", "once", None).unwrap();
    wait(&e, &r.id);
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    e.respond(&a.id, &a.fingerprint, true, "allow").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    let cp = e.store().get(&s).unwrap().checkpoints.remove(0);
    e.restore(&s, &cp.id, &cp.after_digest).unwrap();
    assert!(!ws.path().join("created.txt").exists());
}
#[test]
fn native_file_deletion_always_requires_approval_and_can_be_undone() {
    let (_drive, ws, e, s, _p, _events) = fixture(
        vec![
            FakeTurn::tool_call("fs_delete", r#"{"path":"file.txt"}"#),
            FakeTurn::text("done"),
        ],
        Mode::Agent,
    );
    std::fs::write(ws.path().join("file.txt"), "original").unwrap();
    e.store()
        .update(&s, |s| {
            s.configuration.permission_mode = aidex_native_sessions::PermissionMode::FullAccess;
            Ok(())
        })
        .unwrap();
    let r = e.start(&s, "delete", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::WaitingApproval);
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    e.respond(&a.id, &a.fingerprint, true, "allow").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert!(!ws.path().join("file.txt").exists());
    let cp = e.store().get(&s).unwrap().checkpoints.remove(0);
    assert_eq!(cp.after_digest, "absent");
    std::fs::write(ws.path().join("file.txt"), "user-created").unwrap();
    assert!(e.restore(&s, &cp.id, &cp.after_digest).is_err());
    assert_eq!(
        std::fs::read_to_string(ws.path().join("file.txt")).unwrap(),
        "user-created"
    );
    std::fs::remove_file(ws.path().join("file.txt")).unwrap();
    e.restore(&s, &cp.id, &cp.after_digest).unwrap();
    assert_eq!(
        std::fs::read_to_string(ws.path().join("file.txt")).unwrap(),
        "original"
    );
}

#[test]
fn known_model_output_and_remaining_context_clamp_the_user_upper_bound() {
    let (_d, _ws, e, s, p, _events) = fixture_with_limits(
        vec![FakeTurn::text("small result")],
        Mode::Chat,
        2048,
        Some(128),
    );
    let r = e.start(&s, "hi", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert_eq!(p.requests()[0].max_tokens, Some(128));
    assert_eq!(e.store().get(&s).unwrap().configuration.max_tokens, 4096);
    let (_d2, _ws2, e2, s2, p2, _events2) = fixture_with_limits(
        vec![FakeTurn::text("bounded result")],
        Mode::Chat,
        512,
        None,
    );
    let r = e2.start(&s2, "hi", "once", None).unwrap();
    assert_eq!(wait(&e2, &r.id).status, RunStatus::Completed);
    let request = &p2.requests()[0];
    assert!(request.estimated_input_tokens() + request.max_tokens.unwrap() <= 512);
}

#[test]
fn catalog_context_minimums_neither_clamp_nor_refuse_a_request() {
    // About 1,100 estimated input tokens.
    let long = "x".repeat(4000);
    // Below a catalog minimum the output budget is not cut to what remains.
    let (_d, _ws, e, s, p, _events) = fixture_with_source(
        vec![FakeTurn::text("catalog result")],
        Mode::Chat,
        2048,
        None,
        CapabilitySource::Catalog,
    );
    let r = e.start(&s, &long, "once", None).unwrap();
    let done = wait(&e, &r.id);
    assert_eq!(done.status, RunStatus::Completed, "{:?}", done.error);
    let request = &p.requests()[0];
    let configured = e.store().get(&s).unwrap().configuration.max_tokens;
    assert!(request.estimated_input_tokens() + configured > 2048);
    assert_eq!(request.max_tokens, Some(configured));

    // Above it the harness still sends the request; only the provider (here a
    // fake whose real limit is that minimum) may refuse it.
    let (_d1, _ws1, e1, s1, p1, _events1) = fixture_with_source(
        vec![FakeTurn::text("over the minimum")],
        Mode::Chat,
        512,
        None,
        CapabilitySource::Catalog,
    );
    let r = e1.start(&s1, &long, "once", None).unwrap();
    assert_eq!(wait(&e1, &r.id).status, RunStatus::Failed);
    let sent = p1.requests();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].estimated_input_tokens() > 512);
    assert_eq!(sent[0].max_tokens, Some(configured));

    // A reported window is a real ceiling: the same request is refused unsent.
    let (_d2, _ws2, e2, s2, p2, _events2) = fixture_with_source(
        vec![FakeTurn::text("never sent")],
        Mode::Chat,
        512,
        None,
        CapabilitySource::Discovered,
    );
    let r = e2.start(&s2, &long, "once", None).unwrap();
    let run = wait(&e2, &r.id);
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error
            .as_deref()
            .is_some_and(|error| error.contains("context window"))
    );
    assert!(p2.requests().is_empty());
}
