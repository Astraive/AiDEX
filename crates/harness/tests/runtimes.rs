//! Injected external boundary through real encrypted history and approval state.
use aidex_core::CancelToken;
use aidex_events::MemoryEventSink;
use aidex_harness::NativeEngine;
use aidex_native_sessions::*;
use aidex_providers::{ChatMessage, NoVault, OfflineMode, ProviderRegistry};
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
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
    let session=store.create(serde_json::from_value(json!({"runtime_id":"fixture-runtime","workspace_id":workspace.id,"provider_id":"unused","model_id":"runtime-model","mode":"agent"})).unwrap()).unwrap();
    let engine = NativeEngine::new(
        store,
        registry,
        workspaces,
        Arc::new(MemoryEventSink::new()),
        drive.path().join("scratch"),
    );
    (drive, ws, engine, session.id)
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
            runtime_ref: Some(json!({"thread_id":"durable-thread"})),
            message: Some(ChatMessage::assistant("continued")),
        })
    }
}

#[test]
fn external_followup_uses_saved_reference_and_resume_operation() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(Continuity {
        resume: true,
        requests: Mutex::new(vec![]),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "initial", "initial", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    let second = e.start(&s, "continue", "continue", None).unwrap();
    assert_eq!(wait(&e, &second.id).status, RunStatus::Completed);
    let requests = runtime.requests.lock().unwrap();
    assert_eq!(requests[0].operation, RuntimeOperation::Start);
    assert_eq!(requests[1].operation, RuntimeOperation::Resume);
    assert_eq!(
        requests[1].runtime_ref,
        Some(json!({"thread_id":"durable-thread"}))
    );
    assert_eq!(requests[1].model.as_deref(), Some("runtime-model"));
}

#[test]
fn external_followup_without_resume_capability_fails_explicitly() {
    let (_d, _ws, e, s) = fixture();
    let runtime = Arc::new(Continuity {
        resume: false,
        requests: Mutex::new(vec![]),
    });
    e.register_runtime("fixture-runtime", runtime.clone())
        .unwrap();
    let first = e.start(&s, "initial", "initial", None).unwrap();
    assert_eq!(wait(&e, &first.id).status, RunStatus::Completed);
    assert!(
        e.start(&s, "continue", "continue", None)
            .unwrap_err()
            .contains("resume")
    );
    assert_eq!(runtime.requests.lock().unwrap().len(), 1);
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
