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
        Arc,
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
        Arc::new(MemoryEventSink::new()),
        drive.path().join("scratch"),
    );
    (drive, ws, engine, session.id)
}
struct Boundary {
    effects: AtomicUsize,
    unsafe_ref: bool,
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
