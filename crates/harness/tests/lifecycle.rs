//! Run lifecycle across approvals, parents and children, and vault locks.
use aidex_core::CancelToken;
use aidex_events::{
    Cursor, EventEnvelope, EventError, EventKind, EventScope, EventSink, MemoryEventSink,
};
use aidex_harness::NativeEngine;
use aidex_native_sessions::*;
use aidex_providers::{
    ChatMessage, FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
    NoVault, OfflineMode, ProviderError, ProviderId, ProviderRegistry,
};
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use serde_json::json;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

const PASSPHRASE: &str = "fixture-passphrase";

type Hook = Box<dyn FnOnce() + Send>;

/// Records events and runs a hook once, right after a chosen run is
/// announced cancelled: inside the engine call that cancelled it.
#[derive(Default)]
struct Events {
    recorded: MemoryEventSink,
    on_cancelled: Mutex<Option<(String, Hook)>>,
}
impl Events {
    fn events(&self) -> Vec<EventEnvelope> {
        self.recorded.events()
    }
    fn on_cancelled(&self, run: &str, hook: impl FnOnce() + Send + 'static) {
        *self.on_cancelled.lock().unwrap() = Some((run.to_owned(), Box::new(hook)));
    }
}
impl EventSink for Events {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        let hook = match &event {
            EventKind::TaskCancelled { task_id } => self
                .on_cancelled
                .lock()
                .unwrap()
                .take_if(|(run, _)| run.as_str() == task_id.as_str())
                .map(|(_, hook)| hook),
            _ => None,
        };
        let cursor = self.recorded.emit(scope, event)?;
        if let Some(hook) = hook {
            hook();
        }
        Ok(cursor)
    }
}

struct Fixture {
    _drive: tempfile::TempDir,
    ws: tempfile::TempDir,
    vault: Arc<VaultService>,
    engine: Arc<NativeEngine>,
    session: String,
    provider: Arc<FakeProvider>,
    events: Arc<Events>,
}

/// A native agent session on a fake provider, or (`runtime`) a session run
/// by the external runtime `fixture-runtime`.
fn fixture(turns: Vec<FakeTurn>, runtime: bool) -> Fixture {
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
        .init(&SecretString::from(PASSPHRASE.to_owned()), false)
        .unwrap();
    let store = Arc::new(
        SessionStore::open(
            paths
                .ensure_dir(&paths.sessions())
                .unwrap()
                .join("native.sqlite3"),
            vault.clone(),
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
    let descriptor = ModelDescriptor::new(
        ModelRef::new(
            ProviderId::new("fixture").unwrap(),
            ModelId::new("small").unwrap(),
        ),
        Locality::Local,
        65536,
    )
    .with_tool_calls(aidex_providers::ToolCallSupport::Native);
    let provider = Arc::new(
        FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
            .with_model(descriptor)
            .with_script(turns.into_iter().collect::<FakeScript>()),
    );
    registry.register_shared(provider.clone()).unwrap();
    let events = Arc::new(Events::default());
    let engine = NativeEngine::new(
        store.clone(),
        registry,
        workspaces,
        events.clone(),
        drive.path().join("scratch"),
    );
    let configuration = if runtime {
        json!({"title":"fixture","workspace_id":workspace.id,"mode":"agent","runtime_id":"fixture-runtime","provider_id":RUNTIME_PROVIDER_ID,"model_id":"runtime-model","permission_mode":"supervised"})
    } else {
        json!({"title":"fixture","workspace_id":workspace.id,"mode":"agent","provider_id":"fixture","model_id":"small","permission_mode":"supervised"})
    };
    let session = store
        .create(serde_json::from_value(configuration).unwrap())
        .unwrap();
    Fixture {
        _drive: drive,
        ws,
        vault,
        engine,
        session: session.id,
        provider,
        events,
    }
}

impl Fixture {
    /// The run's state once it stops pending or running.
    fn settled(&self, id: &str) -> Run {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let r = self.engine.store().run(id).unwrap();
            if !matches!(r.status, RunStatus::Pending | RunStatus::Running) {
                return r;
            }
            assert!(Instant::now() < until, "run {id} never settled");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// Wait until no run has a working thread.
    fn idle(&self) {
        let until = Instant::now() + Duration::from_secs(5);
        while self.engine.has_active() {
            assert!(Instant::now() < until, "a run thread never stopped");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn wait_for_request(&self) {
        let until = Instant::now() + Duration::from_secs(5);
        while self.provider.requests().is_empty() {
            assert!(Instant::now() < until, "the model was never asked");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// The run's status as the store's plaintext metadata records it, which
    /// stays readable while the vault is locked.
    fn recorded_status(&self, id: &str) -> RunStatus {
        self.engine
            .store()
            .run_summaries()
            .unwrap()
            .into_iter()
            .find(|r| r.id == id)
            .unwrap()
            .status
    }
    fn lock(&self) {
        assert!(self.vault.lock().was_unlocked);
    }
    fn unlock(&self) {
        self.vault
            .unlock(&SecretString::from(PASSPHRASE.to_owned()))
            .unwrap();
    }
    /// Register, as `fixture-runtime`, a runtime that pauses every run for
    /// one approval.
    fn pausing_runtime(&self) -> Arc<ExpiringApproval> {
        let runtime = Arc::new(ExpiringApproval {
            store: self.engine.store().clone(),
            session: self.session.clone(),
            armed: AtomicBool::new(false),
            continued: AtomicUsize::new(0),
        });
        self.engine
            .register_runtime("fixture-runtime", runtime.clone())
            .unwrap();
        runtime
    }
    /// Start a run and wait until it is paused for its approval.
    fn paused(&self, command: &str, parent: Option<&str>) -> (Run, Approval) {
        let run = self
            .engine
            .start(&self.session, command, command, parent.map(str::to_owned))
            .unwrap();
        assert_eq!(self.settled(&run.id).status, RunStatus::WaitingApproval);
        let approval = self.engine.approvals(Some(&run.id)).unwrap().remove(0);
        (run, approval)
    }
    /// How often `run` was announced cancelled.
    fn cancellations(&self, run: &str) -> usize {
        self.events
            .events()
            .into_iter()
            .filter(|envelope| {
                matches!(&envelope.event, EventKind::TaskCancelled { task_id } if task_id.as_str() == run)
            })
            .count()
    }
}

fn write_call() -> FakeTurn {
    FakeTurn::tool_call("fs_write", r#"{"path":"file.txt","content":"new"}"#)
}

/// Asks for one approval; while `armed`, reading its descriptor expires
/// every pending approval of the session first, as a concurrent cancel,
/// interruption, or expiry would.
struct ExpiringApproval {
    store: Arc<SessionStore>,
    session: String,
    armed: AtomicBool,
    continued: AtomicUsize,
}
impl RuntimeExecutor for ExpiringApproval {
    fn descriptor(&self) -> RuntimeDescriptor {
        if self.armed.swap(false, Ordering::SeqCst) {
            self.store
                .update(&self.session, |s| {
                    for approval in &mut s.approvals {
                        if approval.status == "pending" {
                            approval.status = "expired".into();
                        }
                    }
                    Ok(())
                })
                .unwrap();
        }
        RuntimeDescriptor {
            id: "fixture-runtime".into(),
            name: "expiring approval".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::Supervised],
            supports_resume: true,
            supports_approvals: true,
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
        let reference = json!({"thread_id":"approval-thread"});
        if r.operation == RuntimeOperation::Approval {
            self.continued.fetch_add(1, Ordering::SeqCst);
            return Ok(RuntimeResponse {
                status: RuntimeStatus::Completed,
                runtime_ref: Some(reference),
                message: Some(ChatMessage::assistant("continued")),
            });
        }
        assert_eq!(
            on(&RuntimeEvent::ApprovalRequest {
                request_id: "request-1".into(),
                tool_name: "file_write".into(),
                arguments: json!({"path":"result.txt"}),
                summary: "write".into(),
                runtime_ref: reference.clone(),
            }),
            RuntimeControl::Pause
        );
        Ok(RuntimeResponse {
            status: RuntimeStatus::WaitingApproval,
            runtime_ref: Some(reference),
            message: None,
        })
    }
}

#[test]
fn an_approval_that_expires_while_it_is_answered_is_refused() {
    let f = fixture(vec![], true);
    let runtime = f.pausing_runtime();
    let (_, approval) = f.paused("write", None);
    // Expired after `respond` checked it, before the decision is recorded.
    runtime.armed.store(true, Ordering::SeqCst);
    let error = f
        .engine
        .respond(&approval.id, &approval.fingerprint, true, "approve")
        .unwrap_err();
    assert!(error.contains("changed or expired"), "{error}");
    assert!(!runtime.armed.load(Ordering::SeqCst));
    let session = f.engine.store().get(&f.session).unwrap();
    assert_eq!(session.approvals[0].status, "expired");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(runtime.continued.load(Ordering::SeqCst), 0);
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "approve-again")
            .is_err()
    );
    assert_eq!(runtime.continued.load(Ordering::SeqCst), 0);
}

#[test]
fn failed_parent_cancels_active_child_work() {
    let f = fixture(
        vec![
            FakeTurn::error(ProviderError::Protocol("parent failure".into()))
                .with_delay(Duration::from_millis(200)),
            FakeTurn::text("child must stop").with_delay(Duration::from_secs(3)),
        ],
        false,
    );
    let root = f
        .engine
        .start(&f.session, "parent", "failed-root", None)
        .unwrap();
    f.wait_for_request();
    let child = f
        .engine
        .start(
            &f.session,
            "child",
            "failed-root-child",
            Some(root.id.clone()),
        )
        .unwrap();
    assert_eq!(f.settled(&root.id).status, RunStatus::Failed);
    assert_eq!(
        f.engine.store().run(&child.id).unwrap().status,
        RunStatus::Cancelled
    );
    // The child's own thread stops later; its run stays cancelled and is
    // announced once.
    f.idle();
    assert_eq!(
        f.engine.store().run(&child.id).unwrap().status,
        RunStatus::Cancelled
    );
    assert_eq!(f.cancellations(&child.id), 1);
}

#[test]
fn an_approval_stopped_while_locked_is_interrupted_after_unlock() {
    let f = fixture(vec![write_call(), FakeTurn::text("replanned")], false);
    let run = f.engine.start(&f.session, "write", "write", None).unwrap();
    assert_eq!(f.settled(&run.id).status, RunStatus::WaitingApproval);
    let approval = f.engine.approvals(Some(&run.id)).unwrap().remove(0);
    // A run waiting on a plan the engine still holds is left alone.
    f.engine.reconcile_after_unlock().unwrap();
    assert_eq!(
        f.engine.store().run(&run.id).unwrap().status,
        RunStatus::WaitingApproval
    );
    f.lock();
    f.engine.cancel_all();
    // Its cancellation could not be saved.
    assert_eq!(f.recorded_status(&run.id), RunStatus::WaitingApproval);
    f.unlock();
    f.engine.reconcile_after_unlock().unwrap();
    let interrupted = f.engine.store().run(&run.id).unwrap();
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    assert!(
        interrupted
            .error
            .as_deref()
            .is_some_and(|e| e.contains("locked"))
    );
    assert!(interrupted.pending_calls.is_empty());
    assert_eq!(
        f.engine.approvals(Some(&run.id)).unwrap()[0].status,
        "expired"
    );
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "approve")
            .is_err()
    );
    assert!(!f.ws.path().join("file.txt").exists());
    // Reconciling again changes nothing; only an explicit resume continues.
    f.engine.reconcile_after_unlock().unwrap();
    assert_eq!(
        f.engine.store().run(&run.id).unwrap().status,
        RunStatus::Interrupted
    );
    f.engine.resume(&run.id).unwrap();
    assert_eq!(f.settled(&run.id).status, RunStatus::Completed);
    assert!(!f.ws.path().join("file.txt").exists());
}

#[test]
fn a_run_stopped_while_locked_is_interrupted_after_unlock() {
    let f = fixture(
        vec![FakeTurn::text("slow").with_delay(Duration::from_secs(3))],
        false,
    );
    let run = f.engine.start(&f.session, "slow", "slow", None).unwrap();
    f.wait_for_request();
    f.lock();
    f.engine.cancel_all();
    // The thread stops, but its final state cannot be saved.
    f.idle();
    assert_eq!(f.recorded_status(&run.id), RunStatus::Running);
    f.unlock();
    f.engine.reconcile_after_unlock().unwrap();
    assert_eq!(
        f.engine.store().run(&run.id).unwrap().status,
        RunStatus::Interrupted
    );
}

#[test]
fn has_active_is_true_only_while_a_run_is_live() {
    let f = fixture(
        vec![
            FakeTurn::text("hello").with_delay(Duration::from_millis(300)),
            write_call(),
        ],
        false,
    );
    assert!(!f.engine.has_active());
    let first = f.engine.start(&f.session, "hi", "first", None).unwrap();
    assert!(f.engine.has_active());
    assert_eq!(f.settled(&first.id).status, RunStatus::Completed);
    f.idle();
    // A native run waiting for approval has no thread.
    let second = f.engine.start(&f.session, "write", "second", None).unwrap();
    assert_eq!(f.settled(&second.id).status, RunStatus::WaitingApproval);
    f.idle();
    f.engine.cancel(&second.id).unwrap();
    assert!(!f.engine.has_active());
}

#[test]
fn cancelling_a_paused_runtime_refuses_its_approval() {
    let f = fixture(vec![], true);
    let runtime = f.pausing_runtime();
    let (run, approval) = f.paused("write", None);
    // Its process may still hold credentials.
    assert!(f.engine.has_active());
    assert_eq!(
        f.engine.cancel(&run.id).unwrap().status,
        RunStatus::Cancelled
    );
    assert!(!f.engine.has_active());
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "approve")
            .is_err()
    );
    assert_eq!(
        f.engine.approvals(Some(&run.id)).unwrap()[0].status,
        "expired"
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(runtime.continued.load(Ordering::SeqCst), 0);
    assert!(!f.engine.has_active());
    assert_eq!(f.cancellations(&run.id), 1);
}

#[test]
fn an_approval_answered_during_a_cancel_never_continues_the_runtime() {
    let f = fixture(vec![], true);
    let runtime = f.pausing_runtime();
    let (root, approval) = f.paused("root", None);
    let (child, _) = f.paused("child", Some(&root.id));
    // Approved once the root's tokens are cancelled, before its run is: the
    // continuation reuses the cancelled tokens and stops at once.
    let engine = f.engine.clone();
    let id = root.id.clone();
    let answered = Arc::new(AtomicBool::new(false));
    let approved = answered.clone();
    f.events.on_cancelled(&child.id, move || {
        let result = engine.respond(&approval.id, &approval.fingerprint, true, "approve");
        approved.store(result.is_ok(), Ordering::SeqCst);
        // Let the continuation settle the run before the cancel goes on.
        let until = Instant::now() + Duration::from_secs(5);
        while engine.store().run(&id).is_ok_and(|r| r.status.active()) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    assert_eq!(
        f.engine.cancel(&root.id).unwrap().status,
        RunStatus::Cancelled
    );
    assert!(answered.load(Ordering::SeqCst));
    f.idle();
    assert_eq!(runtime.continued.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.engine.store().run(&root.id).unwrap().status,
        RunStatus::Cancelled
    );
    assert_eq!(f.cancellations(&root.id), 1);
}

#[test]
fn a_paused_runtime_whose_cancel_cannot_be_saved_is_interrupted_after_unlock() {
    let f = fixture(vec![], true);
    let runtime = f.pausing_runtime();
    let (run, approval) = f.paused("write", None);
    f.lock();
    f.engine.cancel_all();
    // Its cancellation could not be saved: it stays counted until settled.
    assert_eq!(f.recorded_status(&run.id), RunStatus::WaitingApproval);
    assert!(f.engine.has_active());
    f.unlock();
    f.engine.reconcile_after_unlock().unwrap();
    let interrupted = f.engine.store().run(&run.id).unwrap();
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    assert!(
        interrupted
            .error
            .as_deref()
            .is_some_and(|e| e.contains("locked"))
    );
    assert!(!f.engine.has_active());
    assert_eq!(
        f.engine.approvals(Some(&run.id)).unwrap()[0].status,
        "expired"
    );
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "approve")
            .is_err()
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(runtime.continued.load(Ordering::SeqCst), 0);
}

#[test]
fn a_cancel_cut_short_by_a_lock_leaves_the_paused_runtime_to_reconciliation() {
    let f = fixture(vec![], true);
    let runtime = f.pausing_runtime();
    let (root, approval) = f.paused("root", None);
    let (child, _) = f.paused("child", Some(&root.id));
    // The vault locks after the child is cancelled, before the root is.
    let vault = f.vault.clone();
    f.events.on_cancelled(&child.id, move || {
        vault.lock();
    });
    assert!(f.engine.cancel(&root.id).is_err());
    assert_eq!(f.recorded_status(&child.id), RunStatus::Cancelled);
    assert_eq!(f.recorded_status(&root.id), RunStatus::WaitingApproval);
    // The root's paused runtime is still counted, with its tokens cancelled.
    assert!(f.engine.has_active());
    f.unlock();
    f.engine.reconcile_after_unlock().unwrap();
    assert_eq!(
        f.engine.store().run(&root.id).unwrap().status,
        RunStatus::Interrupted
    );
    assert!(!f.engine.has_active());
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "approve")
            .is_err()
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(runtime.continued.load(Ordering::SeqCst), 0);
}
