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
    let store =
        Arc::new(SessionStore::open(paths.sessions().join("native.sqlite3"), vault).unwrap());
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
    (drive, ws, engine, session.id)
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
