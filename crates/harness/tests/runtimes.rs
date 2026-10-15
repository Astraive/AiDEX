//! Injected external boundary through real encrypted history and approval state.
use aidex_core::CancelToken;
use aidex_events::{Cursor, EventError, EventKind, EventScope, EventSink, MemoryEventSink};
use aidex_harness::{CONTEXT_TOO_LARGE_FOR_RUNTIME, NativeEngine};
use aidex_native_sessions::*;
use aidex_providers::{ChatMessage, NoVault, OfflineMode, ProviderRegistry};
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<NativeEngine>,
    String,
) {
    fixture_with_model("runtime-model")
}
fn fixture_with_model(
    model: &str,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<NativeEngine>,
    String,
) {
    fixture_with_events(model, Arc::new(MemoryEventSink::new()))
}
fn fixture_with_events(
    model: &str,
    events: Arc<dyn EventSink>,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<NativeEngine>,
    String,
) {
    let (drive, ws, engine, session, _) = fixture_with_vault(model, events);
    (drive, ws, engine, session)
}
/// [`fixture_with_events`] with the vault the store seals with.
fn fixture_with_vault(
    model: &str,
    events: Arc<dyn EventSink>,
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<NativeEngine>,
    String,
    Arc<VaultService>,
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
            paths.vault(),
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
    let store = Arc::new(
        SessionStore::open(paths.sessions().join("native.sqlite3"), vault.clone()).unwrap(),
    );
    let workspaces = Arc::new(aidex_workspace::WorkspaceRegistry::open(&paths).unwrap());
    let workspace = workspaces.add_host(ws.path(), None).unwrap();
    let registry = Arc::new(
        ProviderRegistry::load(
            &paths,
            Arc::new(NoVault::default()),
            Arc::new(OfflineMode::new(false)),
        )
        .unwrap(),
    );
    let session=store.create(serde_json::from_value(json!({"runtime_id":"fixture-runtime","workspace_id":workspace.id,"provider_id":RUNTIME_PROVIDER_ID,"model_id":model,"mode":"agent"})).unwrap()).unwrap();
    let engine = NativeEngine::new(
        store,
        registry,
        workspaces,
        events,
        drive.path().join("scratch"),
    );
    (drive, ws, engine, session.id, vault)
}

/// A reference bound to a native branch and snapshot, as adapters save it.
fn bound_reference(r: &RuntimeRequest) -> Value {
    json!({
        "runtime_id": "fixture-runtime",
        "session_id": r.session_id,
        "run_id": r.run_id,
        "native_session_id": format!("thread-{}", r.run_id),
        "native_branch_id": r.run_id,
        "snapshot_digest": format!("snapshot-{}", r.run_id),
    })
}

/// Completes every request and records it. Its references are bound to a
/// native branch, or shaped like those saved before native continuation
/// (`legacy`).
struct Recorder {
    name: &'static str,
    resume: bool,
    legacy: bool,
    /// Refuse every `Resume` before starting anything, as adapters do when
    /// the saved evidence no longer matches.
    refuse_resume: bool,
    /// Refuse every `Resume` already when validating it.
    refuse_resume_validation: bool,
    requests: Mutex<Vec<RuntimeRequest>>,
}
impl Recorder {
    fn new(name: &'static str, resume: bool) -> Arc<Self> {
        Arc::new(Self {
            name,
            resume,
            legacy: false,
            refuse_resume: false,
            refuse_resume_validation: false,
            requests: Mutex::new(vec![]),
        })
    }
    fn requests(&self) -> Vec<RuntimeRequest> {
        self.requests.lock().unwrap().clone()
    }
    fn operations(&self) -> Vec<RuntimeOperation> {
        self.requests().iter().map(|r| r.operation).collect()
    }
}
impl RuntimeExecutor for Recorder {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: self.name.into(),
            modes: vec![Mode::Chat, Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: self.resume,
            supports_approvals: false,
        }
    }
    fn validate(&self, r: &RuntimeRequest) -> Result<(), RuntimeError> {
        if self.refuse_resume_validation && r.operation == RuntimeOperation::Resume {
            return Err(RuntimeError {
                code: "resume_unavailable".into(),
                message: "native branch identity is missing".into(),
            });
        }
        Ok(())
    }
    fn execute(
        &self,
        r: &RuntimeRequest,
        on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.requests.lock().unwrap().push(r.clone());
        if self.refuse_resume && r.operation == RuntimeOperation::Resume {
            return Err(RuntimeError {
                code: "resume_unavailable".into(),
                message: "native snapshot evidence does not match".into(),
            });
        }
        let reference = if self.legacy {
            json!({"thread_id": format!("thread-{}", r.run_id)})
        } else {
            let reference = bound_reference(r);
            assert_eq!(
                on(&RuntimeEvent::SessionStarted {
                    runtime_ref: reference.clone()
                }),
                RuntimeControl::Continue
            );
            reference
        };
        assert_eq!(
            on(&RuntimeEvent::TextDelta {
                text: "continued".into()
            }),
            RuntimeControl::Continue
        );
        Ok(RuntimeResponse {
            status: RuntimeStatus::Completed,
            runtime_ref: Some(reference),
            message: Some(ChatMessage::assistant("continued")),
        })
    }
}

#[test]
fn external_followup_uses_saved_reference_and_resume_operation() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Recorder::new("Fixture", true);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "initial", "initial", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    let second = e.start(&s, "continue", "continue", None).unwrap();
    assert_eq!(wait(&e, &second.id).status, RunStatus::Completed);
    let requests = runtime.requests();
    assert_eq!(
        runtime.operations(),
        [RuntimeOperation::Start, RuntimeOperation::Resume]
    );
    assert_eq!(requests[1].runtime_ref, Some(bound_reference(&requests[0])));
    assert_eq!(requests[1].model.as_deref(), Some("runtime-model"));
    // A resumed thread already holds the conversation: only the new message
    // is sent.
    assert_eq!(requests[1].messages.len(), 1);
    assert_eq!(requests[1].messages[0].content, "continue");
    assert_eq!(
        e.store().get(&s).unwrap().runtime_ref,
        Some(bound_reference(&requests[1]))
    );
}

#[test]
fn a_reference_saved_before_native_continuation_replays_the_transcript() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(Recorder {
        name: "Fixture",
        resume: true,
        legacy: true,
        refuse_resume: false,
        refuse_resume_validation: false,
        requests: Mutex::new(vec![]),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "initial", "initial", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    assert!(e.store().get(&s).unwrap().runtime_ref.is_some());
    let second = e.start(&s, "continue", "continue", None).unwrap();
    assert_eq!(wait(&e, &second.id).status, RunStatus::Completed);
    assert_eq!(
        runtime.operations(),
        [RuntimeOperation::Start, RuntimeOperation::Start]
    );
    let contents = runtime.requests()[1]
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>();
    assert_eq!(contents, ["initial", "continued", "continue"]);
}

#[test]
fn a_runtime_without_resume_continues_with_the_transcript() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Recorder::new("Pi", false);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    for (content, command) in [("initial", "initial"), ("continue", "continue")] {
        let run = e.start(&s, content, command, None).unwrap();
        assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    }
    assert_eq!(
        runtime.operations(),
        [RuntimeOperation::Start, RuntimeOperation::Start]
    );
    assert_eq!(runtime.requests()[1].messages.len(), 3);
    assert_eq!(
        e.store().get(&s).unwrap().messages.last().unwrap().content,
        "continued"
    );
}

#[test]
fn native_tool_diffs_in_the_transcript_reach_a_runtime_as_statistics() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Recorder::new("Pi", false);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    // The conversation ran a native write before switching to this runtime.
    let output = json!({
        "summary": "wrote notes.txt",
        "content": {"path": "notes.txt", "created": false, "changed": true},
        "diff": {"files": [{
            "path": "notes.txt",
            "kind": "modified",
            "additions": 1,
            "deletions": 1,
            "unified": "--- a/notes.txt\n+++ b/notes.txt\n-EARLIER_LINE\n+new\n",
            "binary": false,
            "truncated": false,
            "before_sha256": "before",
            "after_sha256": "after",
        }], "files_omitted": 0},
        "changes": [{"path": "notes.txt", "change": "modified"}],
        "warnings": [],
    })
    .to_string();
    let mut call = ChatMessage::assistant("");
    call.tool_calls.push(aidex_providers::ToolCall {
        id: "call_1".into(),
        name: "fs_write".into(),
        arguments: r#"{"path":"notes.txt","content":"new\n"}"#.into(),
        provider_metadata: None,
    });
    e.store()
        .update(&s, |session| {
            session.messages.push(ChatMessage::user("edit"));
            session.messages.push(call.clone());
            session
                .messages
                .push(ChatMessage::tool_result("call_1", output.clone()));
            Ok(())
        })
        .unwrap();
    let run = e.start(&s, "continue", "continue", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    let request = runtime.requests().remove(0);
    let result = request
        .messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("call_1"))
        .unwrap();
    assert!(!result.content.contains("EARLIER_LINE"));
    let view: Value = serde_json::from_str(&result.content).unwrap();
    assert_eq!(view["diff_stats"]["files"][0]["deletions"], 1);
    assert_eq!(view["content"]["path"], "notes.txt");
    // History is not rewritten: a result stored whole (before the model view) stays as stored.
    assert!(
        e.store().get(&s).unwrap().messages[2]
            .content
            .contains("EARLIER_LINE")
    );
}

#[test]
fn a_refused_resume_starts_the_followup_over_with_the_transcript() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(Recorder {
        name: "Fixture",
        resume: true,
        legacy: false,
        refuse_resume: true,
        refuse_resume_validation: false,
        requests: Mutex::new(vec![]),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "initial", "initial", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    let second = e.start(&s, "continue", "continue", None).unwrap();
    let second = wait(&e, &second.id);
    assert_eq!(second.status, RunStatus::Completed, "{:?}", second.error);
    let requests = runtime.requests();
    assert_eq!(
        runtime.operations(),
        [
            RuntimeOperation::Start,
            RuntimeOperation::Resume,
            RuntimeOperation::Start
        ]
    );
    // The refused reference is forgotten, and the whole conversation sent.
    assert_eq!(requests[2].runtime_ref, None);
    assert_eq!(requests[2].messages.len(), 3);
    assert_eq!(requests[2].max_steps, requests[1].max_steps);
    let session = e.store().get(&s).unwrap();
    assert_eq!(session.runtime_ref, Some(bound_reference(&requests[2])));
    assert_eq!(second.runtime_ref, Some(bound_reference(&requests[2])));
    assert_eq!(second.steps, 1);
    let contents = session
        .messages
        .iter()
        .map(|m| m.content.clone())
        .collect::<Vec<_>>();
    assert_eq!(contents, ["initial", "continued", "continue", "continued"]);
}

#[test]
fn a_resume_refused_at_validation_starts_the_followup_over() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(Recorder {
        name: "Fixture",
        resume: true,
        legacy: false,
        refuse_resume: false,
        refuse_resume_validation: true,
        requests: Mutex::new(vec![]),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "initial", "initial", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    let second = e.start(&s, "continue", "continue", None).unwrap();
    let second = wait(&e, &second.id);
    assert_eq!(second.status, RunStatus::Completed, "{:?}", second.error);
    assert_eq!(
        runtime.operations(),
        [RuntimeOperation::Start, RuntimeOperation::Start]
    );
    let requests = runtime.requests();
    assert_eq!(requests[1].runtime_ref, None);
    assert_eq!(requests[1].messages.len(), 3);
    assert_eq!(
        e.store().get(&s).unwrap().runtime_ref,
        Some(bound_reference(&requests[1]))
    );
}

#[test]
fn resuming_an_interrupted_run_never_replays_it() {
    // Refused when the runtime starts (as adapters refuse changed evidence),
    // and already when it validates the request.
    for at_validation in [false, true] {
        let (_d, _ws, e, s) = fixture();
        let runtime = Arc::new(Recorder {
            name: "Fixture",
            resume: true,
            legacy: false,
            refuse_resume: !at_validation,
            refuse_resume_validation: at_validation,
            requests: Mutex::new(vec![]),
        });
        e.register_runtime("fixture-runtime", runtime.clone())
            .unwrap();
        let run = e
            .store()
            .begin(&s, "write the file", "interrupted", None)
            .unwrap();
        let reference = json!({"runtime_id":"fixture-runtime","session_id":s,"run_id":run.id,"native_session_id":"thread","native_branch_id":run.id,"snapshot_digest":"snapshot"});
        e.store()
            .update(&s, |session| {
                let r = session.runs.iter_mut().find(|r| r.id == run.id).unwrap();
                r.status = RunStatus::Interrupted;
                r.runtime_ref = Some(reference.clone());
                session.runtime_ref = Some(reference.clone());
                Ok(())
            })
            .unwrap();
        e.resume(&run.id).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let resumed = loop {
            let r = e.store().run(&run.id).unwrap();
            if r.status == RunStatus::Interrupted {
                break r;
            }
            assert!(
                r.status.active() && Instant::now() < until,
                "{:?}",
                r.status
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        // At most one refused resume; nothing replayed, nothing left behind.
        if at_validation {
            assert!(runtime.operations().is_empty());
        } else {
            assert_eq!(runtime.operations(), [RuntimeOperation::Resume]);
            assert_eq!(runtime.requests()[0].messages.len(), 1);
        }
        assert!(
            resumed
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("resume_unavailable")),
            "{:?}",
            resumed.error
        );
        assert_eq!(resumed.steps, 0);
        let session = e.store().get(&s).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.runtime_ref, Some(reference));
        // A reference saved before native continuation is refused at once.
        e.store()
            .update(&s, |session| {
                session
                    .runs
                    .iter_mut()
                    .find(|r| r.id == run.id)
                    .unwrap()
                    .runtime_ref = Some(json!({"thread_id":"main-era"}));
                Ok(())
            })
            .unwrap();
        let calls = runtime.operations().len();
        assert!(e.resume(&run.id).is_err());
        assert_eq!(
            e.store().run(&run.id).unwrap().status,
            RunStatus::Interrupted
        );
        assert_eq!(runtime.operations().len(), calls);
    }
}

#[test]
fn a_forked_conversation_never_resumes_the_source_thread() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Recorder::new("Fixture", true);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "initial", "initial", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    let fork = e
        .store()
        .fork(
            &s,
            ForkRequest {
                configuration: e.store().get(&s).unwrap().configuration,
                title: None,
                command_id: None,
            },
        )
        .unwrap();
    let forked = e.start(&fork.id, "go on", "fork-1", None).unwrap();
    assert_eq!(wait(&e, &forked.id).status, RunStatus::Completed);
    let requests = runtime.requests();
    assert_eq!(requests[1].operation, RuntimeOperation::Start);
    assert_eq!(requests[1].runtime_ref, None);
    assert_eq!(requests[1].messages.len(), 3);
    // The source conversation still continues its own thread.
    let next = e.start(&s, "and here", "source-2", None).unwrap();
    assert_eq!(wait(&e, &next.id).status, RunStatus::Completed);
    let requests = runtime.requests();
    assert_eq!(requests[2].operation, RuntimeOperation::Resume);
    assert_eq!(requests[2].runtime_ref, Some(bound_reference(&requests[0])));
}

#[test]
fn a_long_conversation_resumed_on_an_argument_runtime_is_not_refused() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Recorder::new("Claude Code", true);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    // Each message fits in one command-line argument; together they do not
    // (30,000 UTF-16 units on Windows, 128 KiB on Linux).
    let size = if cfg!(windows) { 20_000 } else { 70_000 };
    let first = e.start(&s, &"a".repeat(size), "first", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    let second = e.start(&s, &"b".repeat(size), "second", None).unwrap();
    assert_eq!(wait(&e, &second.id).status, RunStatus::Completed);
    let requests = runtime.requests();
    assert_eq!(requests[1].operation, RuntimeOperation::Resume);
    assert_eq!(requests[1].messages.len(), 1);
    assert_eq!(requests[1].messages[0].content.len(), size);
    // Replaying the same conversation (a fork has no thread) is refused.
    if cfg!(any(windows, target_os = "linux")) {
        let fork = e
            .store()
            .fork(
                &s,
                ForkRequest {
                    configuration: e.store().get(&s).unwrap().configuration,
                    title: None,
                    command_id: None,
                },
            )
            .unwrap();
        let error = e.start(&fork.id, "c", "fork", None).unwrap_err();
        assert!(error.starts_with(CONTEXT_TOO_LARGE_FOR_RUNTIME), "{error}");
        assert_eq!(runtime.requests().len(), 2);
    }
}

#[test]
fn child_runs_stream_their_own_deltas() {
    let events = Arc::new(MemoryEventSink::new());
    let (_d, _ws, e, s) = fixture_with_events("runtime-model", events.clone());
    let runtime = Recorder::new("Fixture", true);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let root = e
        .store()
        .begin(&s, "root", "root-placeholder", None)
        .unwrap();
    let child = e
        .start(&s, "child", "child-delta", Some(root.id.clone()))
        .unwrap();
    assert_eq!(wait(&e, &child.id).status, RunStatus::Completed);
    e.cancel(&root.id).unwrap();
    let next = e.start(&s, "next", "next", None).unwrap();
    assert_eq!(wait(&e, &next.id).status, RunStatus::Completed);
    let run_deltas = events
        .events()
        .into_iter()
        .filter_map(|envelope| match envelope.event {
            EventKind::AgentRunDelta { agent_id, text } => Some((agent_id.to_string(), text)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        run_deltas,
        [
            (child.id.clone(), "continued".to_owned()),
            (next.id.clone(), "continued".to_owned())
        ]
    );
    // The root-only delta stays for existing clients.
    assert_eq!(
        events
            .names()
            .into_iter()
            .filter(|name| *name == "agent.delta")
            .count(),
        1
    );
    // A child's identity never replaces the conversation's.
    assert!(e.store().run(&child.id).unwrap().runtime_ref.is_some());
    assert_eq!(
        e.store().get(&s).unwrap().runtime_ref,
        e.store().run(&next.id).unwrap().runtime_ref
    );
}

#[test]
fn policy_denying_remote_model_invocation_starts_no_runtime() {
    let (d, _ws, e, s) = fixture();
    let runtime = Recorder::new("Fixture", true);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let paths = aidex_drive::DrivePaths::open(d.path()).unwrap();
    // A local-only rule: the runtime's SDK may route to a cloud model, so its
    // launch is evaluated as a remote invocation.
    std::fs::write(
        paths.policies().join("deny-model.toml"),
        "schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"deny-remote-model\"\naction=\"model.invoke\"\nverdict=\"deny\"\n[rules.when]\nmodel_locality=\"remote\"\n",
    )
    .unwrap();
    let error = e.start(&s, "prompt", "denied", None).unwrap_err();
    assert!(error.contains("policy denied"), "{error}");
    assert!(runtime.requests().is_empty());
    let session = e.store().get(&s).unwrap();
    assert_eq!(session.runs.len(), 1);
    assert_eq!(session.runs[0].status, RunStatus::Failed);
}

#[test]
fn policy_is_evaluated_again_before_an_approval_continues_the_runtime() {
    let (d, ws, e, s) = fixture();
    let boundary = Arc::new(Boundary {
        effects: AtomicUsize::new(0),
        unsafe_ref: false,
    });
    e.register_runtime("fixture-runtime", boundary.clone())
        .unwrap();
    let r = e.start(&s, "prompt", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::WaitingApproval);
    let paths = aidex_drive::DrivePaths::open(d.path()).unwrap();
    std::fs::write(
        paths.policies().join("deny-model.toml"),
        "schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"deny-model\"\naction=\"model.invoke\"\nverdict=\"deny\"\n",
    )
    .unwrap();
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    let run = wait(&e, &r.id);
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error
            .as_deref()
            .is_some_and(|e| e.contains("policy denied")),
        "{:?}",
        run.error
    );
    assert_eq!(boundary.effects.load(Ordering::SeqCst), 0);
    assert!(!ws.path().join("result.txt").exists());
}

#[test]
fn a_request_that_cannot_be_built_fails_the_stored_run() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Recorder::new("Fixture", true);
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    // A saved reference carrying credentials is refused when the request is
    // built, after the run is stored.
    e.store()
        .update(&s, |session| {
            session.runtime_ref = Some(json!({"access_token":"fixture-secret"}));
            Ok(())
        })
        .unwrap();
    let error = e.start(&s, "prompt", "unsafe", None).unwrap_err();
    assert!(error.contains("resume metadata"), "{error}");
    assert!(runtime.requests().is_empty());
    let run = &e.store().get(&s).unwrap().runs[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        !run.error
            .as_deref()
            .unwrap_or_default()
            .contains("fixture-secret")
    );
}

struct Continuity {
    resume: bool,
    requests: Mutex<Vec<RuntimeRequest>>,
}
impl RuntimeExecutor for Continuity {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "continuity".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: self.resume,
            supports_approvals: false,
        }
    }
    fn validate(&self, _: &RuntimeRequest) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn execute(
        &self,
        request: &RuntimeRequest,
        _: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(RuntimeResponse {
            status: RuntimeStatus::Completed,
            runtime_ref: None,
            message: Some(ChatMessage::assistant("continued")),
        })
    }
}

#[test]
fn a_response_without_a_reference_keeps_the_saved_one() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(Continuity {
        resume: true,
        requests: Mutex::new(vec![]),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let saved = json!({"thread_id":"durable-thread"});
    e.store()
        .update(&s, |session| {
            session.runtime_ref = Some(saved.clone());
            Ok(())
        })
        .unwrap();
    let run = e.start(&s, "prompt", "keeps", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    assert_eq!(e.store().get(&s).unwrap().runtime_ref, Some(saved.clone()));
    assert_eq!(e.store().run(&run.id).unwrap().runtime_ref, Some(saved));
}

struct Boundary {
    effects: AtomicUsize,
    unsafe_ref: bool,
}

struct EarlyIdentity {
    store: Arc<SessionStore>,
    unsafe_ref: bool,
    saw_saved: AtomicUsize,
    paused: AtomicUsize,
}
impl RuntimeExecutor for EarlyIdentity {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "early identity".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: true,
            supports_approvals: false,
        }
    }
    fn validate(&self, _: &RuntimeRequest) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn execute(
        &self,
        request: &RuntimeRequest,
        on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        let reference = if self.unsafe_ref {
            json!({"access_token":"fixture-secret"})
        } else {
            json!({"runtime_id":"fixture-runtime","native_session_id":"early-native-session","run_id":request.run_id,"session_id":request.session_id,"snapshot_digest":"sealed-snapshot"})
        };
        let control = on(&RuntimeEvent::SessionStarted {
            runtime_ref: reference.clone(),
        });
        if control == RuntimeControl::Pause {
            self.paused.fetch_add(1, Ordering::SeqCst);
        }
        if control == RuntimeControl::Continue
            && self
                .store
                .run(&request.run_id)
                .unwrap()
                .runtime_ref
                .as_ref()
                == Some(&reference)
        {
            self.saw_saved.fetch_add(1, Ordering::SeqCst);
        }
        Err(RuntimeError {
            code: "injected_stop".into(),
            message: "stop before prompting or effects".into(),
        })
    }
}

#[test]
fn native_identity_is_saved_before_runtime_callback_continues() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(EarlyIdentity {
        store: e.store().clone(),
        unsafe_ref: false,
        saw_saved: AtomicUsize::new(0),
        paused: AtomicUsize::new(0),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let run = e.start(&s, "prompt", "early-identity", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Failed);
    assert_eq!(runtime.saw_saved.load(Ordering::SeqCst), 1);
    let saved = e.store().run(&run.id).unwrap().runtime_ref.unwrap();
    assert_eq!(saved["native_session_id"], "early-native-session");
    assert_eq!(e.store().get(&s).unwrap().runtime_ref, Some(saved));
}

#[test]
fn child_native_identity_never_replaces_root_session_reference() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(EarlyIdentity {
        store: e.store().clone(),
        unsafe_ref: false,
        saw_saved: AtomicUsize::new(0),
        paused: AtomicUsize::new(0),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let root = e
        .store()
        .begin(&s, "root", "root-placeholder", None)
        .unwrap();
    let parent_reference = json!({"thread_id":"root-native-session"});
    e.store()
        .update(&s, |session| {
            session.runtime_ref = Some(parent_reference.clone());
            session
                .runs
                .iter_mut()
                .find(|run| run.id == root.id)
                .unwrap()
                .runtime_ref = Some(parent_reference.clone());
            Ok(())
        })
        .unwrap();
    let child = e
        .start(&s, "child", "child-identity", Some(root.id.clone()))
        .unwrap();
    assert_eq!(wait(&e, &child.id).status, RunStatus::Failed);
    assert!(e.store().run(&child.id).unwrap().runtime_ref.is_some());
    assert_eq!(
        e.store().get(&s).unwrap().runtime_ref,
        Some(parent_reference)
    );
    e.cancel(&root.id).unwrap();
}

#[test]
fn unsafe_native_start_identity_is_rejected_before_persistence() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(EarlyIdentity {
        store: e.store().clone(),
        unsafe_ref: true,
        saw_saved: AtomicUsize::new(0),
        paused: AtomicUsize::new(0),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let run = e.start(&s, "prompt", "unsafe-identity", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Failed);
    assert!(e.store().run(&run.id).unwrap().runtime_ref.is_none());
    assert!(e.store().get(&s).unwrap().runtime_ref.is_none());
    assert_eq!(runtime.saw_saved.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.paused.load(Ordering::SeqCst), 1);
}

/// Reports one identity, then a different one in the same launch.
struct ShiftingIdentity;
impl RuntimeExecutor for ShiftingIdentity {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "shifting identity".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: true,
            supports_approvals: false,
        }
    }
    fn validate(&self, _: &RuntimeRequest) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn execute(
        &self,
        r: &RuntimeRequest,
        on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        let first = bound_reference(r);
        let mut second = first.clone();
        second["native_session_id"] = json!("another-thread");
        for reference in [&first, &first] {
            assert_eq!(
                on(&RuntimeEvent::SessionStarted {
                    runtime_ref: reference.clone()
                }),
                RuntimeControl::Continue
            );
        }
        assert_eq!(
            on(&RuntimeEvent::SessionStarted {
                runtime_ref: second
            }),
            RuntimeControl::Pause
        );
        Ok(RuntimeResponse {
            status: RuntimeStatus::Completed,
            runtime_ref: Some(first),
            message: Some(ChatMessage::assistant("done")),
        })
    }
}

#[test]
fn a_changed_identity_within_one_launch_fails_the_run() {
    let (_d, _ws, e, s) = fixture();
    e.register_runtime("fixture-runtime", Arc::new(ShiftingIdentity))
        .unwrap();
    let run = e.start(&s, "prompt", "shifting", None).unwrap();
    let run = wait(&e, &run.id);
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error
            .as_deref()
            .is_some_and(|e| e.contains("identity changed"))
    );
}

/// Records, for each `approval.created`, the status of the run it is for.
#[derive(Default)]
struct ApprovalProbe {
    inner: MemoryEventSink,
    store: OnceLock<(Arc<SessionStore>, String)>,
    statuses: Mutex<Vec<RunStatus>>,
}
impl EventSink for ApprovalProbe {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        if let EventKind::ApprovalCreated { approval_id, .. } = &event
            && let Some((store, session)) = self.store.get()
        {
            let session = store.get(session).unwrap();
            let approval = session
                .approvals
                .iter()
                .find(|a| a.id == approval_id.as_str())
                .unwrap();
            let run = session
                .runs
                .iter()
                .find(|r| r.id == approval.run_id)
                .unwrap();
            self.statuses.lock().unwrap().push(run.status);
        }
        self.inner.emit(scope, event)
    }
}

#[test]
fn approval_created_is_announced_once_the_run_waits() {
    let probe = Arc::new(ApprovalProbe::default());
    let (_d, _ws, e, s) = fixture_with_events("runtime-model", probe.clone());
    probe
        .store
        .set((e.store().clone(), s.clone()))
        .unwrap_or_else(|_| panic!("probe store set twice"));
    let boundary = Arc::new(Boundary {
        effects: AtomicUsize::new(0),
        unsafe_ref: false,
    });
    e.register_runtime("fixture-runtime", boundary.clone())
        .unwrap();
    let r = e.start(&s, "prompt", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::WaitingApproval);
    // Reaching the store after the event: give the sink a moment.
    let until = Instant::now() + Duration::from_secs(5);
    while probe.statuses.lock().unwrap().is_empty() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        *probe.statuses.lock().unwrap(),
        [RunStatus::WaitingApproval]
    );
    // The approval saved the reference to continue from with the run.
    assert_eq!(
        e.store().run(&r.id).unwrap().runtime_ref,
        Some(json!({"thread_id":"fixture-thread"}))
    );
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert_eq!(boundary.effects.load(Ordering::SeqCst), 1);
}

impl RuntimeExecutor for Boundary {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "Fixture boundary".into(),
            modes: vec![Mode::Chat, Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: true,
            supports_approvals: true,
        }
    }
    fn validate(&self, r: &RuntimeRequest) -> Result<(), RuntimeError> {
        if r.workspace.is_none() {
            return Err(RuntimeError {
                code: "missing_workspace".into(),
                message: "workspace required".into(),
            });
        }
        Ok(())
    }
    fn execute(
        &self,
        r: &RuntimeRequest,
        on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        let reference = if self.unsafe_ref {
            json!({"access_token":"fixture-secret"})
        } else {
            json!({"thread_id":"fixture-thread"})
        };
        if r.approvals.is_empty() {
            assert_eq!(
                on(&RuntimeEvent::TextDelta {
                    text: "runtime progress".into()
                }),
                RuntimeControl::Continue
            );
            assert_eq!(
                on(&RuntimeEvent::ApprovalRequest {
                    request_id: "request-1".into(),
                    tool_name: "file_write".into(),
                    arguments: json!({"path":"result.txt","content":"approved"}),
                    summary: "write synthetic fixture".into(),
                    runtime_ref: reference.clone()
                }),
                RuntimeControl::Pause
            );
            return Ok(RuntimeResponse {
                status: RuntimeStatus::WaitingApproval,
                runtime_ref: Some(reference),
                message: None,
            });
        }
        assert_eq!(r.operation, RuntimeOperation::Approval);
        let approved = r.approvals[0].approved;
        if approved {
            self.effects.fetch_add(1, Ordering::SeqCst);
            std::fs::write(
                std::path::Path::new(&r.workspace.as_ref().unwrap().root).join("result.txt"),
                "approved",
            )
            .unwrap();
        }
        Ok(RuntimeResponse {
            status: RuntimeStatus::Completed,
            runtime_ref: Some(reference),
            message: Some(ChatMessage::assistant(if approved {
                "runtime complete"
            } else {
                "runtime denied"
            })),
        })
    }
}
fn wait(e: &NativeEngine, id: &str) -> Run {
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
fn unregistered_runtime_is_refused() {
    let (_d, _ws, e, s) = fixture();
    assert!(e.start(&s, "prompt", "once", None).is_err());
}
#[test]
fn runtime_approval_history_and_duplicate_commands_keep_one_effect() {
    let (_d, ws, e, s) = fixture();
    let boundary = Arc::new(Boundary {
        effects: AtomicUsize::new(0),
        unsafe_ref: false,
    });
    e.register_runtime("fixture-runtime", boundary.clone())
        .unwrap();
    let r = e.start(&s, "prompt", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::WaitingApproval);
    assert!(!ws.path().join("result.txt").exists());
    let a = e.approvals(Some(&r.id)).unwrap().remove(0);
    assert!(e.respond(&a.id, "wrong", true, "approve").is_err());
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert_eq!(
        e.store().get(&s).unwrap().messages.last().unwrap().content,
        "runtime complete"
    );
    assert_eq!(
        e.store().get(&s).unwrap().runtime_ref,
        Some(json!({"thread_id":"fixture-thread"}))
    );
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    assert_eq!(e.start(&s, "prompt", "once", None).unwrap().id, r.id);
    assert_eq!(boundary.effects.load(Ordering::SeqCst), 1);
}
#[test]
fn authentication_material_is_refused_as_runtime_resume_metadata() {
    let (_d, _ws, e, s) = fixture();
    e.register_runtime(
        "fixture-runtime",
        Arc::new(Boundary {
            effects: AtomicUsize::new(0),
            unsafe_ref: true,
        }),
    )
    .unwrap();
    let r = e.start(&s, "prompt", "once", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Failed);
    assert!(e.store().get(&s).unwrap().runtime_ref.is_none());
}
/// Completes at once and records the model each request asked for.
struct ModelProbe {
    models: std::sync::Mutex<Vec<Option<String>>>,
}
impl RuntimeExecutor for ModelProbe {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "Model probe".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: false,
            supports_approvals: false,
        }
    }
    fn validate(&self, _: &RuntimeRequest) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn execute(
        &self,
        r: &RuntimeRequest,
        _: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.models.lock().unwrap().push(r.model.clone());
        Ok(RuntimeResponse {
            status: RuntimeStatus::Completed,
            runtime_ref: None,
            message: Some(ChatMessage::assistant("probed")),
        })
    }
}
#[test]
fn default_model_sentinel_lets_the_runtime_choose_its_own_model() {
    for (configured, expected) in [
        (RUNTIME_DEFAULT_MODEL, None),
        ("gpt-5-codex", Some("gpt-5-codex".to_owned())),
    ] {
        let (_d, _ws, e, s) = fixture_with_model(configured);
        let probe = Arc::new(ModelProbe {
            models: std::sync::Mutex::new(Vec::new()),
        });
        e.register_runtime("fixture-runtime", probe.clone())
            .unwrap();
        let r = e.start(&s, "prompt", "once", None).unwrap();
        assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
        assert_eq!(*probe.models.lock().unwrap(), [expected]);
    }
}

type Execute = dyn Fn(
        &RuntimeRequest,
        &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError>
    + Send
    + Sync;
type Validate = dyn Fn(&RuntimeRequest) -> Result<(), RuntimeError> + Send + Sync;

/// A runtime whose answers each test scripts. It records its requests and
/// what the harness released ([`RuntimeExecutor::release`]) and forgot
/// ([`RuntimeExecutor::forget`]), and keeps a launch that paused for an
/// approval parked, as adapters keep its process, until it is continued or
/// released.
struct Scripted {
    execute: Box<Execute>,
    validate: Box<Validate>,
    requests: Mutex<Vec<RuntimeRequest>>,
    released: Mutex<Vec<String>>,
    forgotten: Mutex<Vec<Value>>,
    parked: Mutex<Vec<String>>,
}
impl Scripted {
    fn new(
        execute: impl Fn(
            &RuntimeRequest,
            &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
            &CancelToken,
        ) -> Result<RuntimeResponse, RuntimeError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Self::validating(|_| Ok(()), execute)
    }
    fn validating(
        validate: impl Fn(&RuntimeRequest) -> Result<(), RuntimeError> + Send + Sync + 'static,
        execute: impl Fn(
            &RuntimeRequest,
            &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
            &CancelToken,
        ) -> Result<RuntimeResponse, RuntimeError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            execute: Box::new(execute),
            validate: Box::new(validate),
            requests: Mutex::new(Vec::new()),
            released: Mutex::new(Vec::new()),
            forgotten: Mutex::new(Vec::new()),
            parked: Mutex::new(Vec::new()),
        })
    }
    fn requests(&self) -> Vec<RuntimeRequest> {
        self.requests.lock().unwrap().clone()
    }
    fn operations(&self) -> Vec<RuntimeOperation> {
        self.requests().iter().map(|r| r.operation).collect()
    }
    fn released(&self) -> Vec<String> {
        self.released.lock().unwrap().clone()
    }
    fn forgotten(&self) -> Vec<Value> {
        self.forgotten.lock().unwrap().clone()
    }
    fn parked(&self) -> Vec<String> {
        self.parked.lock().unwrap().clone()
    }
}
impl RuntimeExecutor for Scripted {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "Scripted".into(),
            modes: vec![Mode::Chat, Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: true,
            supports_approvals: true,
        }
    }
    fn validate(&self, r: &RuntimeRequest) -> Result<(), RuntimeError> {
        (self.validate)(r)
    }
    fn execute(
        &self,
        r: &RuntimeRequest,
        on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.requests.lock().unwrap().push(r.clone());
        let parked = self.parked.lock().unwrap().contains(&r.run_id);
        if r.operation == RuntimeOperation::Approval {
            if !parked {
                return Err(refusal("approval_expired"));
            }
            self.parked.lock().unwrap().retain(|id| *id != r.run_id);
        } else if parked {
            // As adapters refused a launch while one of the run was parked.
            return Err(refusal("runtime_busy"));
        }
        let response = (self.execute)(r, on, cancel);
        if response
            .as_ref()
            .is_ok_and(|response| response.status == RuntimeStatus::WaitingApproval)
        {
            self.parked.lock().unwrap().push(r.run_id.clone());
        }
        response
    }
    fn release(&self, run_id: &str) {
        self.released.lock().unwrap().push(run_id.into());
        self.parked.lock().unwrap().retain(|id| id != run_id);
    }
    fn forget(&self, runtime_ref: &Value) {
        self.forgotten.lock().unwrap().push(runtime_ref.clone());
    }
}

fn refusal(code: &str) -> RuntimeError {
    RuntimeError {
        code: code.into(),
        message: format!("fixture {code}"),
    }
}

/// The reference an adapter reports when a launch opens its native session:
/// a `Start` begins a branch named after its run, anything else continues
/// the branch of the reference it was given. The branch's snapshot file is
/// named after it.
fn branch_reference(r: &RuntimeRequest) -> Value {
    let branch = r
        .runtime_ref
        .as_ref()
        .filter(|_| r.operation != RuntimeOperation::Start)
        .and_then(|reference| reference["native_branch_id"].as_str())
        .map_or_else(|| r.run_id.clone(), str::to_owned);
    json!({
        "runtime_id": "fixture-runtime",
        "session_id": r.session_id,
        "run_id": r.run_id,
        "native_session_id": format!("thread-{branch}"),
        "native_branch_id": branch,
        "snapshot_digest": format!("digest-{}", r.run_id),
        "snapshot_id": format!("runtime-fixture-{branch}.sealed"),
    })
}

/// Report the launch's native identity.
fn session_started(
    r: &RuntimeRequest,
    on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
) -> Value {
    let reference = branch_reference(r);
    assert_eq!(
        on(&RuntimeEvent::SessionStarted {
            runtime_ref: reference.clone()
        }),
        RuntimeControl::Continue
    );
    reference
}

fn stream(on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl, text: &str) {
    assert_eq!(
        on(&RuntimeEvent::TextDelta { text: text.into() }),
        RuntimeControl::Continue
    );
}

/// Ask approval `request_id` for `arguments`; the harness must pause.
fn ask(
    on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
    request_id: &str,
    arguments: Value,
    reference: &Value,
) -> Result<RuntimeResponse, RuntimeError> {
    assert_eq!(
        on(&RuntimeEvent::ApprovalRequest {
            request_id: request_id.into(),
            tool_name: "file_write".into(),
            arguments,
            summary: "write".into(),
            runtime_ref: reference.clone(),
        }),
        RuntimeControl::Pause
    );
    Ok(RuntimeResponse {
        status: RuntimeStatus::WaitingApproval,
        runtime_ref: Some(reference.clone()),
        message: None,
    })
}

fn finish(reference: Value, text: &str) -> Result<RuntimeResponse, RuntimeError> {
    Ok(RuntimeResponse {
        status: RuntimeStatus::Completed,
        runtime_ref: Some(reference),
        message: Some(ChatMessage::assistant(text)),
    })
}

/// A root run of `s` the store records as interrupted with a bound
/// reference, as one cut by a restart.
fn interrupted_run(e: &NativeEngine, s: &str, content: &str) -> (Run, Value) {
    let run = e.store().begin(s, content, content, None).unwrap();
    let reference = json!({"runtime_id":"fixture-runtime","session_id":s,"run_id":run.id,"native_session_id":"thread","native_branch_id":run.id,"snapshot_digest":"snapshot","snapshot_id":"runtime-fixture-interrupted.sealed"});
    e.store()
        .update(s, |session| {
            let r = session.runs.iter_mut().find(|r| r.id == run.id).unwrap();
            r.status = RunStatus::Interrupted;
            r.runtime_ref = Some(reference.clone());
            session.runtime_ref = Some(reference.clone());
            Ok(())
        })
        .unwrap();
    (run, reference)
}

/// Wait until `run` has `status`, failing if it settles otherwise.
fn wait_for(e: &NativeEngine, run: &str, status: RunStatus) -> Run {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let r = e.store().run(run).unwrap();
        if r.status == status {
            return r;
        }
        assert!(
            Instant::now() < until && (r.status.active() || r.status == status),
            "{:?}: {:?}",
            r.status,
            r.error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn idle(e: &NativeEngine) {
    let until = Instant::now() + Duration::from_secs(5);
    while e.has_active() {
        assert!(Instant::now() < until, "a run thread never stopped");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_follow_up_after_an_incomplete_turn_replays_it() {
    for ending in ["failed", "cancelled", "interrupted"] {
        let (_d, _ws, e, s) = fixture();
        let runtime = Scripted::new(move |r, on, cancel| {
            let reference = session_started(r, on);
            if r.messages.last().unwrap().content == "rename foo to bar everywhere" {
                stream(on, "renamed half");
                if ending == "cancelled" {
                    while !cancel.is_stopped() {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    return Err(refusal("cancelled"));
                }
                return Err(refusal("injected_failure"));
            }
            finish(reference, "done")
        });
        e.register_runtime("fixture-runtime", runtime.clone())
            .unwrap();
        let first = e.start(&s, "hello", "first", None).unwrap();
        wait_for(&e, &first.id, RunStatus::Completed);
        let second = e
            .start(&s, "rename foo to bar everywhere", "second", None)
            .unwrap();
        if ending == "cancelled" {
            let until = Instant::now() + Duration::from_secs(5);
            while e.store().get(&s).unwrap().messages.last().unwrap().content != "renamed half" {
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(5));
            }
            e.cancel(&second.id).unwrap();
            wait_for(&e, &second.id, RunStatus::Cancelled);
        } else {
            wait_for(&e, &second.id, RunStatus::Failed);
        }
        idle(&e);
        if ending == "interrupted" {
            // As a lock and the unlock's reconciliation would leave it.
            e.store()
                .update(&s, |session| {
                    let run = session.runs.iter_mut().find(|r| r.id == second.id).unwrap();
                    run.status = RunStatus::Interrupted;
                    Ok(())
                })
                .unwrap();
        }
        let third = e.start(&s, "continue", "third", None).unwrap();
        wait_for(&e, &third.id, RunStatus::Completed);
        let request = runtime.requests().pop().unwrap();
        assert_eq!(request.operation, RuntimeOperation::Resume, "{ending}");
        // The thread continues from the cut turn's checkpoint, and receives
        // that turn's request and partial output before the new message.
        assert_eq!(
            request.runtime_ref.as_ref().unwrap()["run_id"],
            json!(second.id)
        );
        let contents = request
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            contents,
            ["rename foo to bar everywhere", "renamed half", "continue"],
            "{ending}"
        );
    }
}

#[test]
fn a_thread_the_runtime_cannot_reopen_is_not_followed_up_again() {
    // A protocol failure reopening the thread drops it; a transient refusal
    // keeps it.
    for (code, dropped) in [("protocol_error", true), ("offline_refused", false)] {
        let (_d, _ws, e, s) = fixture();
        let runtime = Scripted::new(move |r, on, _| {
            if r.operation == RuntimeOperation::Resume
                && r.messages.last().unwrap().content == "second"
            {
                return Err(refusal(code));
            }
            let reference = session_started(r, on);
            finish(reference, "answered")
        });
        e.register_runtime("fixture-runtime", runtime.clone())
            .unwrap();
        let first = e.start(&s, "first", "first", None).unwrap();
        wait_for(&e, &first.id, RunStatus::Completed);
        let saved = e.store().get(&s).unwrap().runtime_ref.unwrap();
        let second = e.start(&s, "second", "second", None).unwrap();
        let failed = wait_for(&e, &second.id, RunStatus::Failed);
        assert!(
            failed.error.as_deref().is_some_and(|e| e.contains(code)),
            "{:?}",
            failed.error
        );
        let session = e.store().get(&s).unwrap();
        let third = e.start(&s, "third", "third", None).unwrap();
        wait_for(&e, &third.id, RunStatus::Completed);
        if dropped {
            assert_eq!(session.runtime_ref, None);
            assert_eq!(failed.runtime_ref, None);
            assert_eq!(runtime.forgotten(), [saved]);
            assert_eq!(
                runtime.operations(),
                [
                    RuntimeOperation::Start,
                    RuntimeOperation::Resume,
                    RuntimeOperation::Start
                ]
            );
        } else {
            assert_eq!(session.runtime_ref, Some(saved));
            assert!(runtime.forgotten().is_empty());
            assert_eq!(
                runtime.operations(),
                [
                    RuntimeOperation::Start,
                    RuntimeOperation::Resume,
                    RuntimeOperation::Resume
                ]
            );
        }
    }
}

#[test]
fn a_resume_refused_for_now_leaves_the_run_interrupted() {
    // A pending official login (refused at validation) and a closed runtime
    // gate (refused when launching).
    for (code, at_validation) in [("auth_pending", true), ("offline_refused", false)] {
        let (_d, _ws, e, s) = fixture();
        let runtime = Scripted::validating(
            move |r| {
                if at_validation && r.operation == RuntimeOperation::Resume {
                    Err(refusal(code))
                } else {
                    Ok(())
                }
            },
            move |_, _, _| Err(refusal(code)),
        );
        e.register_runtime("fixture-runtime", runtime.clone())
            .unwrap();
        let (run, reference) = interrupted_run(&e, &s, "write the file");
        e.resume(&run.id).unwrap();
        let again = wait_for(&e, &run.id, RunStatus::Interrupted);
        assert!(
            again.error.as_deref().is_some_and(|e| e.starts_with(code)),
            "{:?}",
            again.error
        );
        assert_eq!(again.steps, 0);
        let session = e.store().get(&s).unwrap();
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.runtime_ref, Some(reference));
        idle(&e);
        // It can be resumed again later.
        e.resume(&run.id).unwrap();
        wait_for(&e, &run.id, RunStatus::Interrupted);
        idle(&e);
    }
}

#[test]
fn a_turn_the_closed_runtime_gate_ends_is_cancelled() {
    let events = Arc::new(MemoryEventSink::new());
    let (_d, _ws, e, s) = fixture_with_events("runtime-model", events.clone());
    let runtime = Scripted::new(|r, on, _| {
        session_started(r, on);
        stream(on, "working");
        Err(refusal("offline_refused"))
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let run = e.start(&s, "work", "gate", None).unwrap();
    wait_for(&e, &run.id, RunStatus::Cancelled);
    // Announced once the state is saved.
    idle(&e);
    let names = events.names();
    assert!(names.contains(&"task.cancelled"), "{names:?}");
    assert!(!names.contains(&"task.failed"));
}

#[test]
fn a_paused_runtime_is_released_whenever_the_run_does_not_continue_it() {
    let pausing = || {
        Scripted::new(|r, on, _| {
            if r.operation == RuntimeOperation::Approval {
                return finish(r.runtime_ref.clone().unwrap(), "continued");
            }
            let reference = session_started(r, on);
            ask(on, "0", json!({"path":"result.txt"}), &reference)
        })
    };
    // Cancelled while it waits.
    let (_d, _ws, e, s) = fixture();
    let runtime = pausing();
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let run = e.start(&s, "write", "cancelled", None).unwrap();
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    assert_eq!(runtime.parked(), std::slice::from_ref(&run.id));
    e.cancel(&run.id).unwrap();
    assert!(runtime.released().contains(&run.id));
    assert!(runtime.parked().is_empty());
    assert!(!e.has_active());

    // Stopped by a vault lock: released although the cancel cannot be saved,
    // and resumable after the unlock's reconciliation.
    let (_d, _ws, e, s, vault) =
        fixture_with_vault("runtime-model", Arc::new(MemoryEventSink::new()));
    let runtime = pausing();
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let run = e.start(&s, "write", "locked", None).unwrap();
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    assert!(vault.lock().was_unlocked);
    e.cancel_all();
    assert!(runtime.released().contains(&run.id));
    assert!(runtime.parked().is_empty());
    vault
        .unlock(&SecretString::from("fixture-passphrase".to_owned()))
        .unwrap();
    e.reconcile_after_unlock().unwrap();
    assert_eq!(
        e.store().run(&run.id).unwrap().status,
        RunStatus::Interrupted
    );
    assert!(!e.has_active());
    e.resume(&run.id).unwrap();
    // The resumed launch reaches the runtime and pauses again.
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    assert_eq!(runtime.operations().last(), Some(&RuntimeOperation::Resume));

    // Resumed while it waits: the parked launch is released first.
    let released = runtime.released().len();
    e.resume(&run.id).unwrap();
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    assert!(runtime.released().len() > released);
    assert_eq!(
        runtime.operations()[runtime.operations().len() - 2..],
        [RuntimeOperation::Resume, RuntimeOperation::Resume]
    );
    assert_eq!(runtime.parked(), std::slice::from_ref(&run.id));

    // Answered, but its continuation fails before reaching the runtime (no
    // step left): the runtime does not keep waiting for it.
    let (_d, _ws, e, s) = fixture();
    let runtime = pausing();
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    e.store()
        .update(&s, |session| {
            session.configuration.max_steps = 1;
            Ok(())
        })
        .unwrap();
    let run = e.start(&s, "write", "budget", None).unwrap();
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    let a = e.approvals(Some(&run.id)).unwrap().remove(0);
    e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
    let failed = wait_for(&e, &run.id, RunStatus::Failed);
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|e| e.contains("step budget")),
        "{:?}",
        failed.error
    );
    idle(&e);
    assert!(runtime.released().contains(&run.id));
    assert!(runtime.parked().is_empty());
    assert_eq!(runtime.operations(), [RuntimeOperation::Start]);
}

#[test]
fn a_resumed_run_may_reuse_an_earlier_launch_request_id() {
    let (_d, _ws, e, s) = fixture();
    let resumed = Arc::new(AtomicUsize::new(0));
    let launches = resumed.clone();
    let runtime = Scripted::new(move |r, on, _| match r.operation {
        RuntimeOperation::Approval => {
            // Only the decision just made is passed on.
            assert_eq!(r.approvals.len(), 1);
            let reference = r.runtime_ref.clone().unwrap();
            if launches.load(Ordering::SeqCst) == 0 && r.approvals[0].request_id == "0" {
                return ask(on, "1", json!({"second": true}), &reference);
            }
            finish(reference, "continued")
        }
        _ => {
            if r.operation == RuntimeOperation::Resume {
                launches.fetch_add(1, Ordering::SeqCst);
            }
            // A new process numbers its requests from 0 again, for other
            // arguments.
            let reference = session_started(r, on);
            ask(
                on,
                "0",
                json!({"launch": launches.load(Ordering::SeqCst)}),
                &reference,
            )
        }
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let run = e.start(&s, "write", "renumbered", None).unwrap();
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    let first = e.approvals(Some(&run.id)).unwrap().remove(0);
    e.respond(&first.id, &first.fingerprint, true, "approve-0")
        .unwrap();
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    // Resumed: the new launch asks "0" again, which is a new approval.
    e.resume(&run.id).unwrap();
    let waiting = wait_for(&e, &run.id, RunStatus::WaitingApproval);
    assert!(waiting.error.is_none(), "{:?}", waiting.error);
    let approvals = e.approvals(Some(&run.id)).unwrap();
    let statuses = approvals
        .iter()
        .map(|a| a.status.as_str())
        .collect::<Vec<_>>();
    assert_eq!(statuses, ["approved", "expired", "pending"]);
    let again = approvals.last().unwrap().clone();
    assert_eq!(again.runtime_request_id.as_deref(), Some("0"));
    assert_ne!(again.fingerprint, first.fingerprint);
    e.respond(&again.id, &again.fingerprint, true, "approve-again")
        .unwrap();
    wait_for(&e, &run.id, RunStatus::Completed);
    let continuation = runtime.requests().pop().unwrap();
    assert_eq!(continuation.approvals[0].fingerprint, again.fingerprint);
}

#[test]
fn an_approval_continuation_keeps_its_turn_in_one_message() {
    for with_message in [true, false] {
        let (_d, _ws, e, s) = fixture();
        let runtime = Scripted::new(move |r, on, _| {
            if r.operation == RuntimeOperation::Approval {
                stream(on, "after");
                return Ok(RuntimeResponse {
                    status: RuntimeStatus::Completed,
                    runtime_ref: r.runtime_ref.clone(),
                    // The adapter's message holds the whole turn.
                    message: with_message.then(|| ChatMessage::assistant("before after")),
                });
            }
            let reference = session_started(r, on);
            stream(on, "before ");
            ask(on, "0", json!({}), &reference)
        });
        e.register_runtime("fixture-runtime", runtime.clone())
            .unwrap();
        let run = e.start(&s, "prompt", "one-message", None).unwrap();
        wait_for(&e, &run.id, RunStatus::WaitingApproval);
        let a = e.approvals(Some(&run.id)).unwrap().remove(0);
        e.respond(&a.id, &a.fingerprint, true, "approve").unwrap();
        let done = wait_for(&e, &run.id, RunStatus::Completed);
        let contents = e
            .store()
            .get(&s)
            .unwrap()
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect::<Vec<_>>();
        assert_eq!(contents, ["prompt", "before after"], "{with_message}");
        assert_eq!(done.transcript.last().unwrap().content, "before after");
    }
}

/// While armed, cancels the run `run` (as a concurrent cancel would) when
/// its descriptor is read, which `resume` does before it makes the run
/// pending.
struct CancelsOnDescriptor {
    store: Arc<SessionStore>,
    session: String,
    run: Mutex<Option<String>>,
    executions: AtomicUsize,
}
impl RuntimeExecutor for CancelsOnDescriptor {
    fn descriptor(&self) -> RuntimeDescriptor {
        if let Some(run) = self.run.lock().unwrap().take() {
            self.store
                .update(&self.session, |s| {
                    s.runs.iter_mut().find(|r| r.id == run).unwrap().status = RunStatus::Cancelled;
                    Ok(())
                })
                .unwrap();
        }
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "cancels on descriptor".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: true,
            supports_approvals: false,
        }
    }
    fn validate(&self, _: &RuntimeRequest) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn execute(
        &self,
        r: &RuntimeRequest,
        _: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        finish(branch_reference(r), "relaunched")
    }
}

#[test]
fn a_run_cancelled_while_it_resumes_is_not_relaunched() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(CancelsOnDescriptor {
        store: e.store().clone(),
        session: s.clone(),
        run: Mutex::new(None),
        executions: AtomicUsize::new(0),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let (run, _) = interrupted_run(&e, &s, "write the file");
    *runtime.run.lock().unwrap() = Some(run.id.clone());
    let refused = e.resume(&run.id).unwrap_err();
    assert!(refused.contains("only an interrupted run"), "{refused}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(e.store().run(&run.id).unwrap().status, RunStatus::Cancelled);
    assert_eq!(runtime.executions.load(Ordering::SeqCst), 0);
}

#[test]
fn only_the_latest_root_run_resumes_and_never_beside_another() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Scripted::new(|r, on, _| {
        let reference = session_started(r, on);
        ask(on, "0", json!({}), &reference)
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let (older, reference) = interrupted_run(&e, &s, "older");
    let newer = e.start(&s, "newer", "newer", None).unwrap();
    wait_for(&e, &newer.id, RunStatus::WaitingApproval);
    // Not beside the active newer run on the same native thread...
    let refused = e.resume(&older.id).unwrap_err();
    assert!(refused.contains("active root run"), "{refused}");
    assert_eq!(
        e.store().run(&older.id).unwrap().status,
        RunStatus::Interrupted
    );
    // ...nor once it stopped: the older thread misses the newer turn.
    e.cancel(&newer.id).unwrap();
    let refused = e.resume(&older.id).unwrap_err();
    assert!(refused.contains("latest message"), "{refused}");
    assert_eq!(
        e.store().run(&older.id).unwrap().status,
        RunStatus::Interrupted
    );
    assert_eq!(runtime.operations(), [RuntimeOperation::Resume]);
    assert_ne!(e.store().get(&s).unwrap().runtime_ref, Some(reference));
}

#[test]
fn native_state_nothing_continues_is_forgotten() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Scripted::new(|r, on, _| {
        if r.operation == RuntimeOperation::Resume {
            // The saved thread no longer matches (another account).
            return Err(refusal("resume_unavailable"));
        }
        let reference = session_started(r, on);
        finish(reference, "answered")
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "first", "first", None).unwrap();
    wait_for(&e, &first.id, RunStatus::Completed);
    let refused = e.store().get(&s).unwrap().runtime_ref.unwrap();
    // A follow-up the runtime refuses starts a new thread; the refused one
    // is forgotten, since only finished runs still name it.
    let second = e.start(&s, "second", "second", None).unwrap();
    wait_for(&e, &second.id, RunStatus::Completed);
    assert_eq!(runtime.forgotten(), [refused]);
    let kept = e.store().get(&s).unwrap().runtime_ref.unwrap();
    // A finished child's thread is never continued.
    let root = e.store().begin(&s, "root", "root", None).unwrap();
    let child = e
        .start(&s, "child", "child", Some(root.id.clone()))
        .unwrap();
    let child = wait_for(&e, &child.id, RunStatus::Completed);
    assert_eq!(runtime.forgotten().len(), 2);
    assert_eq!(runtime.forgotten()[1], child.runtime_ref.unwrap());
    e.cancel(&root.id).unwrap();
    // The conversation's own thread is kept.
    assert!(!runtime.forgotten().contains(&kept));
}

#[test]
fn one_runtime_is_replaced_while_another_runtimes_run_is_active() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Scripted::new(|r, on, _| {
        let reference = session_started(r, on);
        ask(on, "0", json!({}), &reference)
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let run = e.start(&s, "write", "busy", None).unwrap();
    wait_for(&e, &run.id, RunStatus::WaitingApproval);
    /// Another runtime's descriptor.
    struct Other;
    impl RuntimeExecutor for Other {
        fn descriptor(&self) -> RuntimeDescriptor {
            RuntimeDescriptor {
                id: "other-runtime".into(),
                name: "Other".into(),
                modes: vec![Mode::Agent],
                permission_modes: vec![PermissionMode::Supervised],
                supports_resume: false,
                supports_approvals: false,
            }
        }
        fn validate(&self, _: &RuntimeRequest) -> Result<(), RuntimeError> {
            Ok(())
        }
        fn execute(
            &self,
            _: &RuntimeRequest,
            _: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
            _: &CancelToken,
        ) -> Result<RuntimeResponse, RuntimeError> {
            Err(refusal("unused"))
        }
    }
    // The full swap waits for every external run; replacing another
    // runtime's executor does not, but replacing the busy one's does.
    assert!(e.synchronize_runtimes(vec![Arc::new(Other)]).is_err());
    e.replace_runtime("other-runtime", Arc::new(Other)).unwrap();
    assert!(
        e.replace_runtime("fixture-runtime", runtime.clone())
            .is_err()
    );
    assert!(
        e.replace_runtime("fixture-runtime", Arc::new(Other))
            .is_err()
    );
    let ids = e
        .runtime_descriptors()
        .into_iter()
        .map(|d| d.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, ["fixture-runtime", "other-runtime"]);
    e.cancel(&run.id).unwrap();
    e.replace_runtime("fixture-runtime", runtime.clone())
        .unwrap();
}
