//! Daemon wiring of external runtimes: managed accounts, the vault lock, the
//! emergency stop, going offline, the vault keep-alive, and loading a runtime
//! before a run continues. Account peers are the adapters' Python fixtures
//! (`AIDEX_TEST_PYTHON`, as in CI) under the runtime sandbox; runs that must
//! stay active use an in-process runtime that blocks until it is cancelled,
//! and runs that wait for an approval one that pauses for it.

use super::*;
use crate::{
    lifecycle::ShutdownSignal,
    router::{CallContext, ClientInfo},
};
use aidex_core::CancelToken;
use aidex_events::{Cursor, EventFilter};
use aidex_native_sessions::{
    Mode, PermissionMode, RUNTIME_DEFAULT_MODEL, RUNTIME_PROVIDER_ID, RunStatus, RuntimeControl,
    RuntimeDescriptor, RuntimeEvent, RuntimeExecutor, RuntimeOperation, RuntimeRequest,
    RuntimeResponse, RuntimeStatus,
};
use aidex_providers::ChatMessage;
use aidex_vault::KdfParams;
use serde_json::Value;
use std::{
    path::Path,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

const PASSPHRASE: &str = "fixture passphrase only";
/// What the account peer writes as its credential or prints, in any scenario.
const CREDENTIAL_TEXT: [&str; 3] = [
    "fixture-opaque-secret",
    "temporary-opaque-secret",
    "arbitrary-secret-that-must-never-be-returned",
];
/// The in-process runtime's id.
const BLOCKING: &str = "blocking-fixture";

/// Fields drop in order: the handlers, then the services, then the folders.
struct Fixture {
    router: Router,
    context: CallContext,
    services: Arc<Services>,
    workspace_id: String,
    _workspace: tempfile::TempDir,
    drive: tempfile::TempDir,
    /// Every RPC result, to check that none carries credential text.
    results: std::sync::Mutex<Vec<String>>,
}

impl Fixture {
    fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let result = self.router.call(&self.context, method, params);
        let text = match &result {
            Ok(value) => value.to_string(),
            Err(error) => serde_json::to_string(error).unwrap(),
        };
        self.results.lock().unwrap().push(text);
        result
    }

    fn ok(&self, method: &str, params: Value) -> Value {
        self.call(method, params)
            .unwrap_or_else(|error| panic!("{method}: {error:?}"))
    }

    /// The refusal's code and `data.reason`.
    fn refused(&self, method: &str, params: Value) -> (Option<ErrorCode>, String) {
        let error = self.call(method, params).unwrap_err();
        let reason = error
            .data
            .as_ref()
            .and_then(|data| data["reason"].as_str())
            .unwrap_or_default()
            .to_owned();
        (error.kind(), reason)
    }

    fn manager(&self) -> &Arc<RuntimeManager> {
        self.services.runtime().unwrap()
    }

    fn engine_runtimes(&self) -> Vec<String> {
        self.services
            .native()
            .runtime_descriptors()
            .into_iter()
            .map(|descriptor| descriptor.id)
            .collect()
    }

    fn run_status(&self, id: &str) -> RunStatus {
        self.services.native().store().run(id).unwrap().status
    }

    fn wait_inactive(&self, id: &str) -> RunStatus {
        wait_until("the run ends", || !self.run_status(id).active());
        self.run_status(id)
    }

    /// A conversation on runtime `runtime_id`, in Agent mode on the fixture
    /// project.
    fn runtime_session(&self, runtime_id: &str) -> String {
        let created = self.ok(
            "conversation.create",
            json!({
                "runtime_id": runtime_id,
                "workspace_id": self.workspace_id,
                "provider_id": RUNTIME_PROVIDER_ID,
                "model_id": RUNTIME_DEFAULT_MODEL,
                "mode": "agent",
                "permission_mode": "full_access",
            }),
        );
        created["id"].as_str().unwrap().to_owned()
    }

    fn send(&self, session: &str, command: &str) -> String {
        let run = self.ok(
            "message.send",
            json!({"session_id": session, "content": "hello", "command_id": command}),
        );
        run["id"].as_str().unwrap().to_owned()
    }

    /// Add `run` to `session` as the store would have saved it, with no thread.
    fn insert_run(&self, session: &str, status: &str, runtime_ref: Option<Value>) -> String {
        let id = aidex_native_sessions::id("run").unwrap();
        let mut run = json!({
            "id": id, "session_id": session, "parent_run_id": null, "status": status,
            "steps": 0, "error": null, "created_at": 0, "updated_at": 0,
        });
        if let Some(reference) = runtime_ref {
            run["runtime_ref"] = reference;
        }
        let run = serde_json::from_value(run).unwrap();
        self.services
            .native()
            .store()
            .update(session, |detail| {
                detail.runs.push(run);
                Ok(())
            })
            .unwrap();
        id
    }

    /// Every RPC result so far and the whole event log, as text.
    fn everything_returned(&self) -> String {
        let events = self
            .services
            .events()
            .replay(&EventFilter::all(), Cursor::ZERO, 256)
            .unwrap();
        let mut text = self.results.lock().unwrap().join("\n");
        text.push_str(&serde_json::to_string(&events.events).unwrap());
        text
    }

    fn assert_no_credential_text(&self) {
        let text = self.everything_returned();
        for marker in CREDENTIAL_TEXT {
            assert!(!text.contains(marker), "{marker} was returned");
        }
    }
}

fn fixture() -> Fixture {
    let drive = crate::services::testing::temp_drive();
    let shutdown = ShutdownSignal::new();
    let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
    services
        .vault()
        .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
    let context = CallContext::detached(ClientInfo::new("runtime-wiring-test", "1"), shutdown);
    let mut router = Router::new();
    register(&mut router, &services).unwrap();
    crate::methods::vault::register(&mut router, &services).unwrap();
    crate::methods::native::register(&mut router, &services).unwrap();
    crate::methods::emergency::register(&mut router, &services).unwrap();
    crate::methods::models::register(&mut router, &services).unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let workspace_id = services
        .workspaces()
        .add_host(workspace.path(), None)
        .unwrap()
        .id
        .to_string();
    let fixture = Fixture {
        router,
        context,
        services,
        workspace_id,
        _workspace: workspace,
        drive,
        results: std::sync::Mutex::new(Vec::new()),
    };
    fixture.ok("vault.init", json!({"passphrase": PASSPHRASE}));
    fixture
}

fn python() -> String {
    std::env::var("AIDEX_TEST_PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "C:/Users/gskne/AppData/Local/Programs/Python/Python312/python.exe".into()
        } else {
            "/usr/bin/python3".into()
        }
    })
}

/// A Python fixture of `aidex-adapters`.
fn adapter_fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../adapters/tests")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

/// A Codex runtime whose official CLI is the adapters' account peer, in
/// `scenario` (`pending` waits for the browser; the default connects).
fn account_runtime(id: &str, scenario: Option<&str>) -> Value {
    let mut argv = vec!["-u".to_owned(), adapter_fixture("account_peer.py")];
    argv.extend(scenario.map(str::to_owned));
    json!({"id": id, "driver": "codex", "executable": python(), "argv": argv})
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// An external runtime that works until its run is cancelled.
#[derive(Default)]
struct Blocking {
    launches: AtomicUsize,
    cancelled: AtomicUsize,
}

impl Blocking {
    /// Assert that the runtime saw its run cancelled once. The run's status
    /// can settle before the runtime's next check, so this waits for it.
    fn assert_cancelled_once(&self) {
        wait_until("the runtime sees the cancel", || {
            self.cancelled.load(Ordering::SeqCst) > 0
        });
        assert_eq!(self.cancelled.load(Ordering::SeqCst), 1);
    }
}

impl RuntimeExecutor for Blocking {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: BLOCKING.into(),
            name: "Blocking fixture".into(),
            modes: vec![Mode::Chat, Mode::Plan, Mode::Agent],
            permission_modes: vec![
                PermissionMode::Supervised,
                PermissionMode::AutoAcceptEdits,
                PermissionMode::Auto,
                PermissionMode::FullAccess,
            ],
            supports_resume: false,
            supports_approvals: false,
        }
    }

    fn validate(&self, _: &RuntimeRequest) -> Result<(), aidex_adapters::RuntimeError> {
        Ok(())
    }

    fn execute(
        &self,
        _: &RuntimeRequest,
        _: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, aidex_adapters::RuntimeError> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(60);
        while !cancel.is_stopped() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.cancelled.fetch_add(1, Ordering::SeqCst);
        Err(aidex_adapters::RuntimeError {
            code: "cancelled".into(),
            message: "Fixture runtime cancelled".into(),
        })
    }
}

/// Register the blocking runtime, then start a run on it and wait until it works.
fn blocking_run(f: &Fixture) -> (Arc<Blocking>, String) {
    let runtime = Arc::new(Blocking::default());
    f.services
        .native()
        .register_runtime(BLOCKING, runtime.clone())
        .unwrap();
    let session = f.runtime_session(BLOCKING);
    let run = f.send(&session, "blocking-1");
    wait_until("the runtime works", || {
        runtime.launches.load(Ordering::SeqCst) == 1
    });
    (runtime, run)
}

/// A resumable reference, as adapters save one.
fn resumable_reference(runtime: &str) -> Value {
    json!({
        "runtime_id": runtime,
        "native_session_id": "thread-fixture",
        "native_branch_id": "branch-fixture",
        "snapshot_digest": "snapshot-fixture",
    })
}

#[test]
fn account_methods_need_an_unlocked_vault_and_a_known_codex_runtime() {
    let f = fixture();
    f.ok("runtime.add", account_runtime("codex-account", None));
    let missing = f.drive.path().join("missing").join("claude.exe");
    f.ok(
        "runtime.add",
        json!({"id": "claude-fixture", "driver": "claude_code", "executable": missing.to_string_lossy()}),
    );
    let source = f.drive.path().join("auth.json");
    let calls = |id: &str| {
        vec![
            (RUNTIME_ACCOUNT_STATUS, json!({"id": id})),
            (
                RUNTIME_PROFILE_IMPORT,
                json!({"id": id, "source": source.to_string_lossy()}),
            ),
            (RUNTIME_LOGIN_START, json!({"id": id})),
            (RUNTIME_LOGIN_STATUS, json!({"id": id, "flow_id": "flow"})),
            (RUNTIME_LOGIN_CANCEL, json!({"id": id, "flow_id": "flow"})),
            (RUNTIME_LOGIN_LOGOUT, json!({"id": id})),
        ]
    };
    for (method, params) in calls("absent") {
        assert_eq!(
            f.refused(method, params),
            (Some(ErrorCode::NotFound), "not_found".into()),
            "{method}"
        );
    }
    // Only Codex has a verified official login.
    for (method, params) in calls("claude-fixture") {
        if matches!(method, RUNTIME_LOGIN_STATUS | RUNTIME_LOGIN_CANCEL) {
            continue;
        }
        assert_eq!(
            f.refused(method, params),
            (Some(ErrorCode::Unavailable), "auth_unsupported".into()),
            "{method}"
        );
    }
    // A flow of another runtime is unknown to this one.
    assert_eq!(
        f.refused(
            RUNTIME_LOGIN_CANCEL,
            json!({"id": "codex-account", "flow_id": "flow"})
        ),
        (Some(ErrorCode::NotFound), "not_found".into())
    );
    assert!(f.manager().process_registry().is_empty(), "nothing ran");

    f.ok("vault.lock", Value::Null);
    for id in ["codex-account", "absent"] {
        for (method, params) in calls(id) {
            assert_eq!(
                f.refused(method, params),
                (Some(ErrorCode::VaultLocked), "vault_locked".into()),
                "{method} {id}"
            );
        }
    }
    f.assert_no_credential_text();
}

/// A link to `file` (or a junction to its directory where this host allows
/// no file link), at `link`. `false` when no link can be made here.
fn make_link(file: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(file, link).is_ok()
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(file, link).is_ok()
            || std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(file.parent().unwrap())
                .output()
                .is_ok_and(|output| output.status.success())
    }
}

#[test]
fn profile_import_refuses_relative_linked_and_oversize_sources_without_their_contents() {
    let f = fixture();
    f.ok("runtime.add", account_runtime("codex-account", None));
    let import = |source: &Path| {
        f.call(
            RUNTIME_PROFILE_IMPORT,
            json!({"id": "codex-account", "source": source.to_string_lossy()}),
        )
    };
    let folder = tempfile::tempdir().unwrap();
    let marker = "IMPORT-MARKER-FILE-CONTENT-7731";
    let credential = |token: &str| json!({"tokens": {"access_token": token}}).to_string();

    let relative = import(Path::new("auth.json")).unwrap_err();
    assert_eq!(relative.kind(), Some(ErrorCode::InvalidArgument));
    assert_eq!(relative.data.unwrap()["reason"], "invalid_request");

    let oversize = folder.path().join("oversize.json");
    std::fs::write(
        &oversize,
        format!("{}{}", credential(marker), " ".repeat(1024 * 1024)),
    )
    .unwrap();
    let no_evidence = folder.path().join("plain.json");
    std::fs::write(&no_evidence, json!({"note": marker}).to_string()).unwrap();
    let mut refused = vec![import(&oversize), import(&no_evidence)];
    let linked_dir = tempfile::tempdir().unwrap();
    let target = linked_dir.path().join("auth.json");
    std::fs::write(&target, credential(marker)).unwrap();
    let link = folder.path().join("linked");
    if make_link(&target, &link) {
        refused.push(import(&link));
    } else {
        eprintln!("skipped: this host can make neither a file link nor a junction");
    }
    for result in refused {
        let error = result.unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidArgument), "{error:?}");
        assert_eq!(error.data.as_ref().unwrap()["reason"], "profile_invalid");
        let text = serde_json::to_string(&error).unwrap();
        assert!(!text.contains(marker), "{text}");
    }
    assert!(f.manager().get("codex-account").unwrap().profile.is_none());

    // A confirmed credential becomes the runtime's encrypted profile.
    let valid = folder.path().join("auth.json");
    std::fs::write(&valid, credential("fixture-opaque-secret")).unwrap();
    let status = import(folder.path()).unwrap();
    assert_eq!(status["authenticated"], true, "{status}");
    assert_eq!(status["status"], "authenticated_cached");
    let profile = f.manager().get("codex-account").unwrap().profile;
    assert!(profile.is_some());
    assert_eq!(status["profile"], json!(profile));
    assert_eq!(f.engine_runtimes(), ["codex-account"]);
    f.assert_no_credential_text();
}

#[test]
fn a_connected_login_is_reported_during_an_external_run_and_nothing_returns_credentials() {
    let f = fixture();
    f.ok("runtime.add", account_runtime("codex-account", None));
    let started = f.ok(RUNTIME_LOGIN_START, json!({"id": "codex-account"}));
    assert_eq!(started["state"], "pending");
    let flow = started["flow_id"].as_str().unwrap().to_owned();

    // An active external run: the engine refuses to swap its runtimes now.
    let session = f.runtime_session("codex-account");
    let active = f.insert_run(&session, "running", None);
    assert_eq!(
        f.refused(
            RUNTIME_ADD,
            json!({"id": "other", "driver": "codex", "executable": python()})
        )
        .0,
        Some(ErrorCode::Conflict)
    );

    let mut status = Value::Null;
    wait_until("the login connects", || {
        status = f.ok(
            RUNTIME_LOGIN_STATUS,
            json!({"id": "codex-account", "flow_id": flow}),
        );
        status["state"] != "pending"
    });
    assert_eq!(status["state"], "connected", "{status}");
    assert_eq!(status["authenticated"], true);
    assert!(status["authorization_url"].is_null());
    assert!(f.manager().get("codex-account").unwrap().profile.is_some());
    let account = f.ok(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-account"}));
    assert_eq!(account["authenticated"], true, "{account}");
    assert_eq!(account["credential_present"], true);

    // Signing out waits for the run to end, then removes the profile.
    assert_eq!(
        f.refused(RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-account"}))
            .0,
        Some(ErrorCode::Conflict)
    );
    f.services
        .native()
        .store()
        .update(&session, |detail| {
            detail.runs[0].status = RunStatus::Completed;
            Ok(())
        })
        .unwrap();
    assert!(!f.run_status(&active).active());
    let signed_out = f.ok(RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-account"}));
    assert_eq!(signed_out["status"], "signed_out", "{signed_out}");
    assert_eq!(signed_out["authenticated"], false);

    // A failed login reports a fixed message, not what the CLI printed.
    let mut failing = account_runtime("codex-account", Some("fail"));
    failing["profile"] = Value::Null;
    f.ok(RUNTIME_UPDATE, failing);
    let flow = f.ok(RUNTIME_LOGIN_START, json!({"id": "codex-account"}))["flow_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_until("the login fails", || {
        f.ok(
            RUNTIME_LOGIN_STATUS,
            json!({"id": "codex-account", "flow_id": flow}),
        )["state"]
            == "failed"
    });
    f.assert_no_credential_text();
}

#[test]
fn locking_the_vault_cancels_a_pending_login() {
    let f = fixture();
    f.ok(
        "runtime.add",
        account_runtime("codex-account", Some("pending")),
    );
    for direct in [false, true] {
        let started = f.ok(RUNTIME_LOGIN_START, json!({"id": "codex-account"}));
        let flow = started["flow_id"].as_str().unwrap().to_owned();
        assert!(f.manager().has_pending_logins());
        wait_until("the login process runs", || {
            !f.manager().process_registry().is_empty()
        });
        if direct {
            // As the idle auto-lock does.
            f.services.vault().lock();
        } else {
            f.ok("vault.lock", Value::Null);
        }
        // The lock cleanup ran before the lock returned.
        assert!(!f.manager().has_pending_logins());
        assert!(f.manager().process_registry().is_empty());
        f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
        let status = f.ok(
            RUNTIME_LOGIN_STATUS,
            json!({"id": "codex-account", "flow_id": flow}),
        );
        assert_eq!(status["state"], "cancelled", "{status}");
        assert!(status["authorization_url"].is_null());
        assert!(f.manager().get("codex-account").unwrap().profile.is_none());
    }
    f.assert_no_credential_text();
}

#[test]
fn the_runtime_gate_follows_offline_mode_the_vault_and_the_emergency_stop() {
    let f = fixture();
    let gate = crate::services::runtime_gate_for(&f.services);
    assert!(gate());
    f.ok("network.offline", json!({"enabled": true}));
    assert!(!gate());
    f.ok("network.offline", json!({"enabled": false}));
    assert!(gate());
    f.ok("emergency.stop", json!({}));
    assert!(!gate());
    f.ok("emergency.resume", Value::Null);
    assert!(gate());
    f.ok("vault.lock", Value::Null);
    assert!(!gate());
    f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
    assert!(gate());
}

#[test]
fn emergency_stop_ends_runtime_work_and_holds_launches_until_resume() {
    let f = fixture();
    f.ok(
        "runtime.add",
        account_runtime("codex-account", Some("pending")),
    );
    let mut models = json!({"id": "codex-models", "driver": "codex", "executable": python()});
    models["argv"] = json!(["-u", adapter_fixture("peer.py"), "codex", "normal"]);
    f.ok("runtime.add", models);
    let flow = f.ok(RUNTIME_LOGIN_START, json!({"id": "codex-account"}))["flow_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (runtime, run) = blocking_run(&f);

    let report = f.ok("emergency.stop", json!({}));
    assert_eq!(report["vault_locked"], false);
    assert_eq!(f.wait_inactive(&run), RunStatus::Cancelled);
    runtime.assert_cancelled_once();
    assert!(!f.manager().has_pending_logins());
    assert!(f.manager().process_registry().is_empty());
    assert_eq!(
        f.ok(
            RUNTIME_LOGIN_STATUS,
            json!({"id": "codex-account", "flow_id": flow})
        )["state"],
        "cancelled"
    );
    // No vendor process starts while the stop is in effect...
    assert_eq!(
        f.refused(RUNTIME_MODELS, json!({"id": "codex-models"})),
        (Some(ErrorCode::Unavailable), "offline_refused".into())
    );
    assert_eq!(
        f.refused(RUNTIME_LOGIN_START, json!({"id": "codex-account"})),
        (Some(ErrorCode::Unavailable), "offline_refused".into())
    );
    // ...and the runtimes work again once it is lifted.
    assert_eq!(f.ok("emergency.resume", Value::Null)["resumed"], true);
    let listed = f.ok(RUNTIME_MODELS, json!({"id": "codex-models"}));
    assert_eq!(listed["models"][0]["id"], "fixture", "{listed}");
    f.assert_no_credential_text();
}

#[test]
fn an_emergency_stop_the_broker_fails_still_ends_agent_work_and_locks_the_vault() {
    let f = fixture();
    let (runtime, run) = blocking_run(&f);
    // Another writer holds the approval database, so the broker's stop fails
    // once its busy timeout passes, after it froze.
    let holder = rusqlite::Connection::open(&f.services.broker().config().approvals_path).unwrap();
    holder.busy_timeout(Duration::from_secs(5)).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let failed = f.call("emergency.stop", json!({"lock_vault": true}));
    holder.execute_batch("ROLLBACK").unwrap();
    assert!(failed.is_err(), "{failed:?}");
    assert_eq!(f.services.vault().status().state, VaultState::Locked);
    wait_until("the run thread ends", || !f.services.native().has_active());
    assert_eq!(runtime.cancelled.load(Ordering::SeqCst), 1);
    assert!(f.manager().process_registry().is_empty());
    // The stop stays in effect until it is lifted.
    assert!(!crate::services::runtime_gate_for(&f.services)());
    f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
    assert!(!crate::services::runtime_gate_for(&f.services)());
    assert!(!f.run_status(&run).active());
}

#[test]
fn going_offline_cancels_external_runs_but_not_local_ones() {
    use aidex_providers::{
        FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
        ProviderId,
    };
    let f = fixture();
    let fake = Arc::new(
        FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
            .with_model(ModelDescriptor::new(
                ModelRef::new(
                    ProviderId::new("fixture").unwrap(),
                    ModelId::new("small").unwrap(),
                ),
                Locality::Local,
                2048,
            ))
            .with_script(
                FakeScript::new()
                    .then(FakeTurn::text("local answer").with_delay(Duration::from_secs(4))),
            ),
    );
    f.services
        .models()
        .registry()
        .register_shared(fake.clone())
        .unwrap();
    let local = f.ok(
        "conversation.create",
        json!({"provider_id": "fixture", "model_id": "small"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let local_run = f.send(&local, "local-1");
    wait_until("the local model is asked", || !fake.requests().is_empty());
    let (runtime, external_run) = blocking_run(&f);

    f.ok("network.offline", json!({"enabled": true}));
    assert_eq!(f.wait_inactive(&external_run), RunStatus::Cancelled);
    runtime.assert_cancelled_once();
    assert!(f.run_status(&local_run).active(), "a local run keeps going");
    assert_eq!(f.wait_inactive(&local_run), RunStatus::Completed);
}

#[test]
fn an_executing_run_holds_the_vault_open_and_the_idle_lock_follows_its_end() {
    let f = fixture();
    f.services
        .vault()
        .set_idle_timeout(Some(Duration::from_secs(2)));
    let (_runtime, run) = blocking_run(&f);
    assert!(f.services.native().has_executing());
    // Nothing but the keep-alive uses the vault while the runtime works.
    std::thread::sleep(Duration::from_millis(4500));
    assert_eq!(
        f.services.vault().status().state,
        VaultState::Unlocked,
        "an executing run is vault use"
    );
    assert!(f.run_status(&run).active());
    f.ok("run.cancel", json!({"id": run}));
    assert_eq!(f.wait_inactive(&run), RunStatus::Cancelled);
    wait_until("the idle lock", || {
        f.services.vault().status().state == VaultState::Locked
    });
}

/// The vault's idle period is `idle`, and nothing uses the vault: wait until
/// the idle auto-lock happens, then unlock it again (which settles the runs
/// the lock stopped) with no idle period left to race the rest of the test.
fn idle_lock_and_unlock(f: &Fixture, idle: Duration) {
    f.services.vault().set_idle_timeout(Some(idle));
    wait_until("the idle lock", || {
        f.services.vault().status().state == VaultState::Locked
    });
    f.services.vault().set_idle_timeout(None);
    f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
}

#[test]
fn a_native_run_waiting_for_an_approval_lets_the_idle_lock_happen_and_stays_resumable() {
    use aidex_providers::{
        FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
        ProviderId, ToolCallSupport,
    };
    let f = fixture();
    let fake = Arc::new(
        FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
            .with_model(
                ModelDescriptor::new(
                    ModelRef::new(
                        ProviderId::new("fixture").unwrap(),
                        ModelId::new("small").unwrap(),
                    ),
                    Locality::Local,
                    65536,
                )
                .with_tool_calls(ToolCallSupport::Native),
            )
            .with_script(
                FakeScript::new()
                    .then(FakeTurn::tool_call(
                        "fs_write",
                        r#"{"path":"effect.txt","content":"new"}"#,
                    ))
                    .then(FakeTurn::text("resumed")),
            ),
    );
    f.services
        .models()
        .registry()
        .register_shared(fake)
        .unwrap();
    let session = f.ok(
        "conversation.create",
        json!({
            "provider_id": "fixture", "model_id": "small", "mode": "agent",
            "permission_mode": "supervised", "workspace_id": f.workspace_id,
        }),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = f.send(&session, "write-1");
    wait_until("the run waits for its approval", || {
        f.run_status(&run) == RunStatus::WaitingApproval
    });
    assert!(!f.services.native().has_executing());
    // A person who walks away from the approval: the vault locks as usual,
    // and its cleanup stops the run.
    idle_lock_and_unlock(&f, Duration::from_secs(1));
    assert_eq!(f.run_status(&run), RunStatus::Interrupted);
    let approvals = f.ok("approval.list", json!({"run_id": run}));
    assert_eq!(
        approvals["approvals"][0]["status"], "expired",
        "{approvals}"
    );
    // An explicit resume continues it, without the unapproved write.
    f.ok("run.resume", json!({"id": run}));
    assert_eq!(f.wait_inactive(&run), RunStatus::Completed);
    assert!(!f._workspace.path().join("effect.txt").exists());
}

/// The in-process runtime that pauses for an approval.
const APPROVING: &str = "approving-fixture";

/// An external runtime that pauses for one approval, as a vendor process
/// does, and completes once its run is resumed.
#[derive(Default)]
struct Approving {
    resumed: AtomicUsize,
}

impl RuntimeExecutor for Approving {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: APPROVING.into(),
            name: "Approving fixture".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![
                PermissionMode::Supervised,
                PermissionMode::AutoAcceptEdits,
                PermissionMode::Auto,
                PermissionMode::FullAccess,
            ],
            supports_resume: true,
            supports_approvals: true,
        }
    }

    fn validate(&self, _: &RuntimeRequest) -> Result<(), aidex_adapters::RuntimeError> {
        Ok(())
    }

    fn execute(
        &self,
        request: &RuntimeRequest,
        on: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, aidex_adapters::RuntimeError> {
        let reference = resumable_reference(APPROVING);
        if request.operation == RuntimeOperation::Resume {
            self.resumed.fetch_add(1, Ordering::SeqCst);
            return Ok(RuntimeResponse {
                status: RuntimeStatus::Completed,
                runtime_ref: Some(reference),
                message: Some(ChatMessage::assistant("resumed")),
            });
        }
        let control = on(&RuntimeEvent::ApprovalRequest {
            request_id: "request-1".into(),
            tool_name: "shell".into(),
            arguments: json!({"command": "echo fixture"}),
            summary: "run a command".into(),
            runtime_ref: reference.clone(),
        });
        if control != RuntimeControl::Pause {
            return Err(aidex_adapters::RuntimeError {
                code: "not_paused".into(),
                message: "Fixture runtime was not paused".into(),
            });
        }
        Ok(RuntimeResponse {
            status: RuntimeStatus::WaitingApproval,
            runtime_ref: Some(reference),
            message: None,
        })
    }
}

#[test]
fn an_external_run_paused_for_an_approval_lets_the_idle_lock_happen_and_stays_resumable() {
    let f = fixture();
    let runtime = Arc::new(Approving::default());
    f.services
        .native()
        .register_runtime(APPROVING, runtime.clone())
        .unwrap();
    let session = f.runtime_session(APPROVING);
    let run = f.send(&session, "approving-1");
    wait_until("the runtime pauses for its approval", || {
        f.run_status(&run) == RunStatus::WaitingApproval
    });
    // The paused runtime is still live (a vendor process would be), but it
    // is not executing.
    assert!(f.services.native().has_active());
    assert!(!f.services.native().has_executing());
    idle_lock_and_unlock(&f, Duration::from_secs(1));
    assert!(!f.services.native().has_active());
    assert_eq!(f.run_status(&run), RunStatus::Interrupted);
    let approvals = f.ok("approval.list", json!({"run_id": run}));
    assert_eq!(
        approvals["approvals"][0]["status"], "expired",
        "{approvals}"
    );
    assert_eq!(f.ok("run.get", json!({"id": run}))["has_runtime_ref"], true);
    // An explicit resume continues the runtime's own thread.
    f.ok("run.resume", json!({"id": run}));
    assert_eq!(f.wait_inactive(&run), RunStatus::Completed);
    assert_eq!(runtime.resumed.load(Ordering::SeqCst), 1);
}

/// A Codex runtime whose official CLI is the adapters' account peer behind a
/// wrapper that adds a line to the returned log for every launch.
fn counted_account_runtime(id: &str, folder: &Path) -> (Value, std::path::PathBuf) {
    let log = folder.join("launches.log");
    let wrapper = folder.join("counting_peer.py");
    std::fs::write(
        &wrapper,
        "import runpy, sys\n\
         log, peer = sys.argv[1], sys.argv[2]\n\
         with open(log, 'a') as f:\n    f.write('launch\\n')\n\
         sys.argv = [peer] + sys.argv[3:]\n\
         runpy.run_path(peer, run_name='__main__')\n",
    )
    .unwrap();
    let argv = [
        "-u".to_owned(),
        wrapper.to_string_lossy().into_owned(),
        log.to_string_lossy().into_owned(),
        adapter_fixture("account_peer.py"),
    ];
    let config = json!({"id": id, "driver": "codex", "executable": python(), "argv": argv});
    (config, log)
}

fn launches(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |text| text.lines().count())
}

#[test]
fn account_commands_start_no_official_cli_while_offline_or_during_an_emergency_stop() {
    let f = fixture();
    let folder = tempfile::tempdir().unwrap();
    let (config, log) = counted_account_runtime("codex-account", folder.path());
    f.ok("runtime.add", config.clone());
    let credential = folder.path().join("auth.json");
    std::fs::write(
        &credential,
        json!({"tokens": {"access_token": "fixture-opaque-secret"}}).to_string(),
    )
    .unwrap();
    let import = || {
        f.refused(
            RUNTIME_PROFILE_IMPORT,
            json!({"id": "codex-account", "source": credential.to_string_lossy()}),
        )
    };
    let imported = f.ok(
        RUNTIME_PROFILE_IMPORT,
        json!({"id": "codex-account", "source": credential.to_string_lossy()}),
    );
    assert_eq!(imported["status"], "authenticated_cached", "{imported}");
    let confirmed = f.ok(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-account"}));
    assert_eq!(confirmed["authenticated"], true, "{confirmed}");
    let launched = launches(&log);
    assert!(launched >= 1, "the official CLI confirmed the import");
    // A runtime without a profile, and one whose profile the CLI never
    // confirmed in this daemon.
    let mut fresh = config.clone();
    fresh["id"] = json!("codex-fresh");
    f.ok("runtime.add", fresh);
    let mut unconfirmed = config.clone();
    unconfirmed["id"] = json!("codex-unconfirmed");
    unconfirmed["profile"] = json!("account-unconfirmed");
    f.ok("runtime.add", unconfirmed);

    let paused = |why: &str| {
        // The account's last confirmed status, even for a configuration whose
        // CLI the adapter never asked (its own status cache misses).
        let account = f.ok(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-account"}));
        assert_eq!(account["authenticated"], true, "{why}: {account}");
        assert_eq!(account["status"], "authenticated_cached", "{why}");
        let health = f.ok(RUNTIME_HEALTH, json!({"id": "codex-account"}));
        assert_eq!(health["installed"], true, "{why}: {health}");
        assert_eq!(health["protocol_ready"], false, "{why}");
        assert_eq!(health["authenticated"], true, "{why}");
        let fresh = f.ok(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-fresh"}));
        assert_eq!(fresh["credential_present"], false, "{why}: {fresh}");
        assert_eq!(fresh["status"], "auth_required", "{why}");
        let refused = (Some(ErrorCode::Unavailable), "offline_refused".to_owned());
        assert_eq!(
            f.refused(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-unconfirmed"})),
            refused,
            "{why}"
        );
        let health = f.ok(RUNTIME_HEALTH, json!({"id": "codex-unconfirmed"}));
        assert_eq!(health["status"], "installed_auth_unverified", "{why}");
        assert_eq!(import(), refused, "{why}");
        assert_eq!(
            f.refused(RUNTIME_LOGIN_START, json!({"id": "codex-account"})),
            refused,
            "{why}"
        );
        assert_eq!(
            f.refused(RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-account"})),
            refused,
            "{why}"
        );
        // Unknown runtimes and other drivers are refused as before.
        assert_eq!(
            f.refused(RUNTIME_ACCOUNT_STATUS, json!({"id": "absent"})).1,
            "not_found",
            "{why}"
        );
        assert_eq!(launches(&log), launched, "{why}: the official CLI ran");
        assert!(f.manager().process_registry().is_empty(), "{why}");
    };

    f.ok("network.offline", json!({"enabled": true}));
    let mut changed = f.ok(RUNTIME_CONFIG_GET, json!({"id": "codex-account"}));
    changed["argv"]
        .as_array_mut()
        .unwrap()
        .push(json!("variant"));
    f.ok(RUNTIME_UPDATE, changed);
    paused("offline");
    f.ok("network.offline", json!({"enabled": false}));
    f.ok("emergency.stop", json!({}));
    paused("emergency stop");
    f.ok("emergency.resume", Value::Null);

    // Once the gate opens the official CLI runs again.
    let signed_out = f.ok(RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-account"}));
    assert_eq!(signed_out["status"], "signed_out", "{signed_out}");
    assert_eq!(launches(&log), launched + 1);
    f.assert_no_credential_text();
}

#[test]
fn a_vault_lock_from_outside_any_rpc_stops_a_run_within_two_seconds() {
    let f = fixture();
    let (runtime, run) = blocking_run(&f);
    let locked = Instant::now();
    f.services.vault().lock();
    while f.services.native().has_active() {
        assert!(locked.elapsed() < Duration::from_secs(2), "still running");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(runtime.cancelled.load(Ordering::SeqCst), 1);
    // The final state needed the record key; the unlock settles it.
    f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
    let status = f.ok("run.get", json!({"id": run}))["status"].clone();
    assert!(status == "cancelled" || status == "interrupted", "{status}");
}

#[test]
fn unlock_settles_a_run_left_waiting_on_an_approval_the_lock_dropped() {
    let f = fixture();
    let session = f.ok(
        "conversation.create",
        json!({"provider_id": "fixture", "model_id": "small", "mode": "agent", "workspace_id": f.workspace_id}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = f.insert_run(&session, "waiting_approval", None);
    let approval = aidex_native_sessions::id("approval").unwrap();
    let pending = serde_json::from_value(json!({
        "id": approval, "run_id": run, "tool_name": "fs.write",
        "arguments": {"path": "effect.txt"}, "fingerprint": "fixture", "status": "pending",
    }))
    .unwrap();
    f.services
        .native()
        .store()
        .update(&session, |detail| {
            detail.approvals.push(pending);
            Ok(())
        })
        .unwrap();
    // As the idle auto-lock does: nothing can be saved after the lock.
    f.services.vault().lock();
    f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
    assert_eq!(f.ok("run.get", json!({"id": run}))["status"], "interrupted");
    let approvals = f.ok("approval.list", json!({"run_id": run}));
    assert_eq!(
        approvals["approvals"][0]["status"], "expired",
        "{approvals}"
    );
    assert!(!f._workspace.path().join("effect.txt").exists());
}

#[test]
fn run_resume_and_approval_respond_load_a_runtime_the_engine_lost() {
    let f = fixture();
    f.services.models().offline().set_offline(true).unwrap();
    let executable = f.drive.path().join("missing").join("codex.exe");
    f.ok(
        "runtime.add",
        json!({"id": "codex-fixture", "driver": "codex", "executable": executable.to_string_lossy()}),
    );
    let session = f.runtime_session("codex-fixture");
    let interrupted = f.insert_run(
        &session,
        "interrupted",
        Some(resumable_reference("codex-fixture")),
    );

    f.services
        .native()
        .synchronize_runtimes(Vec::new())
        .unwrap();
    let _ = f.call("run.resume", json!({"id": interrupted}));
    assert_eq!(f.engine_runtimes(), ["codex-fixture"]);
    f.wait_inactive(&interrupted);

    // While a run of the runtime waits for an approval the engine refuses a
    // swap; the missing runtime is added on its own.
    f.services
        .native()
        .synchronize_runtimes(Vec::new())
        .unwrap();
    let waiting = f.insert_run(
        &session,
        "waiting_approval",
        Some(resumable_reference("codex-fixture")),
    );
    let approval = aidex_native_sessions::id("approval").unwrap();
    let pending = serde_json::from_value(json!({
        "id": approval, "run_id": waiting, "tool_name": "shell", "arguments": {},
        "fingerprint": "fixture", "status": "pending", "runtime_id": "codex-fixture",
    }))
    .unwrap();
    f.services
        .native()
        .store()
        .update(&session, |detail| {
            detail.approvals.push(pending);
            Ok(())
        })
        .unwrap();
    assert!(
        f.call(
            "approval.respond",
            json!({"id": approval, "fingerprint": "other", "approved": true, "command_id": "respond-1"}),
        )
        .is_err()
    );
    assert_eq!(f.engine_runtimes(), ["codex-fixture"]);
}

#[test]
fn switching_mode_permission_or_instructions_ends_the_runtime_thread() {
    let f = fixture();
    f.services.models().offline().set_offline(true).unwrap();
    let executable = f.drive.path().join("missing").join("codex.exe");
    f.ok(
        "runtime.add",
        json!({"id": "codex-fixture", "driver": "codex", "executable": executable.to_string_lossy()}),
    );
    let session = f.runtime_session("codex-fixture");
    let current =
        std::cell::RefCell::new(f.ok("agent.configuration.get", json!({"session_id": session})));
    let reference = resumable_reference("codex-fixture");
    // A saved thread: the conversation's reference and an interrupted run's.
    let bind = || {
        let run = f.insert_run(&session, "interrupted", Some(reference.clone()));
        f.services
            .native()
            .store()
            .update(&session, |detail| {
                detail.runtime_ref = Some(reference.clone());
                Ok(())
            })
            .unwrap();
        run
    };
    let set = |change: &dyn Fn(&mut Value)| {
        let mut next = current.borrow().clone();
        change(&mut next);
        f.ok(
            "agent.configuration.set",
            json!({"session_id": session, "configuration": next}),
        );
        *current.borrow_mut() = next;
    };
    // Whether the conversation keeps a reference, and how many runs do.
    let thread = || {
        let detail = f.services.native().store().get(&session).unwrap();
        (
            detail.runtime_ref.is_some(),
            detail
                .runs
                .iter()
                .filter(|r| r.runtime_ref.is_some())
                .count(),
        )
    };

    let run = bind();
    assert_eq!(f.ok("run.get", json!({"id": run}))["has_runtime_ref"], true);
    // A new title keeps the thread.
    set(&|config| config["title"] = json!("renamed"));
    assert_eq!(thread(), (true, 1));
    // Agent to Chat, and back: neither continues the other mode's thread.
    set(&|config| config["mode"] = json!("chat"));
    assert_eq!(thread(), (false, 0));
    assert_eq!(
        f.ok("run.get", json!({"id": run}))["has_runtime_ref"],
        false
    );
    bind();
    set(&|config| config["mode"] = json!("agent"));
    assert_eq!(thread(), (false, 0));
    bind();
    set(&|config| config["permission_mode"] = json!("supervised"));
    assert_eq!(thread(), (false, 0));
    bind();
    set(&|config| config["instructions"] = json!("Answer briefly."));
    assert_eq!(thread(), (false, 0));
}

/// A runtime of the adapters' protocol peer (`peer.py`) for `driver` in
/// `scenario` (`approval` pauses for one approval; `normal` completes).
fn peer_runtime(id: &str, driver: &str, scenario: &str) -> Value {
    json!({
        "id": id, "driver": driver, "executable": python(),
        "argv": ["-u", adapter_fixture("peer.py"), driver, scenario],
    })
}

/// Wait until the runtime manager holds no launch parked for an approval and
/// no temporary home (the homes' removal can lag their release slightly).
fn wait_released(f: &Fixture, why: &str) {
    wait_until(why, || {
        f.manager().parked_runs() == 0
            && f.manager().temporary_homes() == 0
            && f.manager().process_registry().is_empty()
    });
}

#[test]
fn a_vendor_process_paused_for_an_approval_is_released_by_a_lock_a_stop_or_going_offline() {
    let f = fixture();
    f.ok(
        "runtime.add",
        peer_runtime("acp-approval", "acp", "approval"),
    );
    let pause = |command: &str| {
        let session = f.runtime_session("acp-approval");
        let run = f.send(&session, command);
        wait_until("the vendor process pauses for its approval", || {
            f.run_status(&run) == RunStatus::WaitingApproval
        });
        // The paused launch keeps its home: the restored native state.
        assert_eq!(f.manager().parked_runs(), 1);
        assert!(f.manager().temporary_homes() >= 1);
        run
    };

    // The idle auto-lock (the walk-away case) releases it with the lock.
    let run = pause("approval-lock");
    f.services.vault().lock();
    wait_released(&f, "the lock releases the paused launch");
    f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
    assert_eq!(f.run_status(&run), RunStatus::Interrupted);
    // The documented continuation reaches the vendor again instead of
    // failing on the released launch (`runtime_busy`).
    f.ok("run.resume", json!({"id": run}));
    wait_until("the resumed run pauses again or ends", || {
        !matches!(f.run_status(&run), RunStatus::Running | RunStatus::Pending)
    });
    let resumed = f.services.native().store().run(&run).unwrap();
    assert_ne!(resumed.status, RunStatus::Failed, "{:?}", resumed.error);
    // Cancelling a paused run releases its launch at once.
    if resumed.status.active() {
        f.ok("run.cancel", json!({"id": run}));
        assert_eq!(f.wait_inactive(&run), RunStatus::Cancelled);
    }
    wait_released(&f, "the cancel releases the paused launch");

    // An emergency stop.
    let run = pause("approval-stop");
    f.ok("emergency.stop", json!({}));
    wait_released(&f, "the emergency stop releases the paused launch");
    assert_eq!(f.wait_inactive(&run), RunStatus::Cancelled);
    f.ok("emergency.resume", Value::Null);

    // Going offline.
    let run = pause("approval-offline");
    f.ok("network.offline", json!({"enabled": true}));
    wait_released(&f, "going offline releases the paused launch");
    assert_eq!(f.wait_inactive(&run), RunStatus::Cancelled);
    f.assert_no_credential_text();
}

/// The in-process runtime that stops only when the runtime gate closes.
const GATE_BOUND: &str = "gate-bound-fixture";

/// An external runtime that fails as soon as the runtime gate closes without
/// looking at its cancel token, as a vendor process the gate's watcher kills.
struct GateBound {
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
    launches: AtomicUsize,
}

impl RuntimeExecutor for GateBound {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: GATE_BOUND.into(),
            ..Blocking::default().descriptor()
        }
    }

    fn validate(&self, _: &RuntimeRequest) -> Result<(), aidex_adapters::RuntimeError> {
        Ok(())
    }

    fn execute(
        &self,
        _: &RuntimeRequest,
        _: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        _: &CancelToken,
    ) -> Result<RuntimeResponse, aidex_adapters::RuntimeError> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let until = Instant::now() + Duration::from_secs(60);
        while (self.gate)() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(aidex_adapters::RuntimeError {
            code: "offline_refused".into(),
            message: "Fixture vendor process ended by the closed gate".into(),
        })
    }
}

#[test]
fn an_emergency_stop_or_going_offline_settles_external_runs_as_cancelled() {
    for trigger in ["emergency.stop", "network.offline"] {
        let f = fixture();
        let runtime = Arc::new(GateBound {
            gate: crate::services::runtime_gate_for(&f.services),
            launches: AtomicUsize::new(0),
        });
        f.services
            .native()
            .register_runtime(GATE_BOUND, runtime.clone())
            .unwrap();
        let session = f.runtime_session(GATE_BOUND);
        let run = f.send(&session, "gate-1");
        wait_until("the runtime works", || {
            runtime.launches.load(Ordering::SeqCst) == 1
        });
        match trigger {
            "emergency.stop" => f.ok(trigger, json!({})),
            _ => f.ok(trigger, json!({"enabled": true})),
        };
        assert_eq!(f.wait_inactive(&run), RunStatus::Cancelled, "{trigger}");
        let events = f
            .services
            .events()
            .replay(&EventFilter::all(), Cursor::ZERO, 256)
            .unwrap()
            .events;
        let names = events.iter().map(|e| e.name()).collect::<Vec<_>>();
        assert!(names.contains(&"task.cancelled"), "{trigger}: {names:?}");
        assert!(!names.contains(&"task.failed"), "{trigger}: {names:?}");
    }
}

/// A Codex runtime whose official CLI is the account peer behind a wrapper
/// that makes `login status` and `logout` take `delay` seconds while the file
/// [`slow_marker`] of `folder` exists.
fn slow_account_runtime(id: &str, folder: &Path, delay: f64) -> Value {
    let wrapper = folder.join("slow_peer.py");
    std::fs::write(
        &wrapper,
        "import os, runpy, sys, time\n\
         marker, delay, peer = sys.argv[1], float(sys.argv[2]), sys.argv[3]\n\
         if os.path.exists(marker) and ('status' in sys.argv or 'logout' in sys.argv):\n    time.sleep(delay)\n\
         sys.argv = [peer] + sys.argv[4:]\n\
         runpy.run_path(peer, run_name='__main__')\n",
    )
    .unwrap();
    let argv = [
        "-u".to_owned(),
        wrapper.to_string_lossy().into_owned(),
        slow_marker(folder).to_string_lossy().into_owned(),
        delay.to_string(),
        adapter_fixture("account_peer.py"),
    ];
    json!({"id": id, "driver": "codex", "executable": python(), "argv": argv})
}

/// While this file exists, [`slow_account_runtime`]'s account commands are slow.
fn slow_marker(folder: &Path) -> std::path::PathBuf {
    folder.join("slow")
}

/// A native conversation on a local fake model that answers at once.
fn native_conversation(f: &Fixture) -> String {
    use aidex_providers::{
        FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
        ProviderId,
    };
    let fake = Arc::new(
        FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
            .with_model(ModelDescriptor::new(
                ModelRef::new(
                    ProviderId::new("fixture").unwrap(),
                    ModelId::new("small").unwrap(),
                ),
                Locality::Local,
                2048,
            ))
            .with_script(FakeScript::new().then(FakeTurn::text("local answer"))),
    );
    f.services
        .models()
        .registry()
        .register_shared(fake)
        .unwrap();
    f.ok(
        "conversation.create",
        json!({"provider_id": "fixture", "model_id": "small"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A credential file the account peer accepts, in `folder`.
fn fixture_credential(folder: &Path) -> std::path::PathBuf {
    let credential = folder.join("auth.json");
    std::fs::write(
        &credential,
        json!({"tokens": {"access_token": "fixture-opaque-secret"}}).to_string(),
    )
    .unwrap();
    credential
}

#[test]
fn an_import_runs_the_official_cli_without_holding_up_other_conversations() {
    let f = fixture();
    let folder = tempfile::tempdir().unwrap();
    // Two runtimes configured with the same account profile.
    let mut config = slow_account_runtime("codex-account", folder.path(), 3.0);
    config["profile"] = json!("account-shared");
    f.ok("runtime.add", config.clone());
    config["id"] = json!("codex-twin");
    f.ok("runtime.add", config);
    let account_session = f.runtime_session("codex-account");
    let twin_session = f.runtime_session("codex-twin");
    let native = native_conversation(&f);
    let credential = fixture_credential(folder.path());
    std::fs::write(slow_marker(folder.path()), b"").unwrap();
    std::thread::scope(|scope| {
        let import = scope.spawn(|| {
            f.call(
                RUNTIME_PROFILE_IMPORT,
                json!({"id": "codex-account", "source": credential.to_string_lossy()}),
            )
        });
        wait_until("the official CLI checks the credential", || {
            !f.manager().process_registry().is_empty()
        });
        // Another conversation goes on meanwhile...
        let started = Instant::now();
        let run = f.send(&native, "native-1");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "message.send waited {:?} for the import",
            started.elapsed()
        );
        assert!(
            !f.manager().process_registry().is_empty(),
            "the import was still running"
        );
        // ...while the runtimes whose account changes take no turn.
        for session in [&account_session, &twin_session] {
            assert_eq!(
                f.refused(
                    "message.send",
                    json!({"session_id": session, "content": "hi", "command_id": "during"})
                ),
                (Some(ErrorCode::Conflict), "runtime_busy".into())
            );
        }
        assert_eq!(
            f.refused(RUNTIME_REMOVE, json!({"id": "codex-twin"})),
            (Some(ErrorCode::Conflict), "runtime_busy".into())
        );
        assert_eq!(
            f.refused(RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-account"})),
            (Some(ErrorCode::Conflict), "runtime_busy".into())
        );
        assert_eq!(f.wait_inactive(&run), RunStatus::Completed);
        let imported = import.join().unwrap().unwrap();
        assert_eq!(imported["status"], "authenticated_cached", "{imported}");
    });
    // The engine has the runtimes again, with their new account.
    let mut runtimes = f.engine_runtimes();
    runtimes.sort();
    assert_eq!(runtimes, ["codex-account", "codex-twin"]);
    assert!(f.manager().get("codex-account").unwrap().profile.is_some());
    assert_eq!(
        f.services.runtime_sync().loaded(),
        Some(f.manager().generation())
    );
    f.assert_no_credential_text();
}

#[test]
fn going_offline_ends_an_official_cli_command_already_running() {
    let f = fixture();
    let folder = tempfile::tempdir().unwrap();
    f.ok(
        "runtime.add",
        slow_account_runtime("codex-account", folder.path(), 15.0),
    );
    // A signed-in account, so its status runs the official CLI's `login
    // status` (the import leaves no cached status).
    let credential = fixture_credential(folder.path());
    f.ok(
        RUNTIME_PROFILE_IMPORT,
        json!({"id": "codex-account", "source": credential.to_string_lossy()}),
    );
    std::fs::write(slow_marker(folder.path()), b"").unwrap();
    for (method, params) in [
        (RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-account"})),
        (RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-account"})),
    ] {
        std::thread::scope(|scope| {
            let command = scope.spawn(|| f.call(method, params.clone()));
            wait_until("the official CLI runs", || {
                !f.manager().process_registry().is_empty()
            });
            let offline = Instant::now();
            f.ok("network.offline", json!({"enabled": true}));
            assert!(f.manager().process_registry().is_empty(), "{method}");
            // The account command ends with its process, not after its
            // timeout (15 s, then a 5 s kill grace).
            let _ = command.join().unwrap();
            assert!(
                offline.elapsed() < Duration::from_secs(5),
                "{method}: the official CLI kept running for {:?}",
                offline.elapsed()
            );
        });
        f.ok("network.offline", json!({"enabled": false}));
    }
    f.assert_no_credential_text();
}

#[test]
fn a_runtime_change_the_engine_has_not_seen_is_loaded_before_the_next_turn() {
    let f = fixture();
    f.ok("runtime.add", account_runtime("codex-account", None));
    let session = f.runtime_session("codex-account");
    let other = f.runtime_session(BLOCKING);
    let folder = tempfile::tempdir().unwrap();
    let credential = fixture_credential(folder.path());
    // An external run of another runtime is active, so the engine cannot
    // reload its runtimes while the account binds outside any daemon call (as
    // a login's worker does once it connects).
    let active = f.insert_run(&other, "running", None);
    f.manager()
        .import_profile("codex-account", &credential)
        .unwrap();
    f.ok(RUNTIME_LIST, Value::Null);
    assert_ne!(
        f.services.runtime_sync().loaded(),
        Some(f.manager().generation())
    );
    let launched_error = |run: &str| {
        f.wait_inactive(run);
        f.services
            .native()
            .store()
            .run(run)
            .unwrap()
            .error
            .unwrap_or_default()
    };
    // The next turn replaces just this runtime's executor with the one made
    // for the new account instead of launching the one the engine had
    // (`account_changed`), while the other run goes on.
    let run = f.send(&session, "during-other-run");
    let error = launched_error(&run);
    assert!(!error.contains("account_changed"), "{error}");
    assert!(f.run_status(&active).active());
    f.services
        .native()
        .store()
        .update(&other, |detail| {
            detail.runs[0].status = RunStatus::Completed;
            Ok(())
        })
        .unwrap();
    // Once no external run is active, the next turn loads every runtime.
    let run = f.send(&session, "after-other-run");
    assert_eq!(
        f.services.runtime_sync().loaded(),
        Some(f.manager().generation())
    );
    let error = launched_error(&run);
    assert!(!error.contains("account_changed"), "{error}");
}

#[test]
fn engine_refusals_after_an_idle_lock_are_vault_locked_and_otherwise_runtime_busy() {
    let f = fixture();
    let config = account_runtime("codex-account", None);
    f.ok("runtime.add", config.clone());
    let session = f.runtime_session("codex-account");
    // A runtime without a managed profile has nothing to sign out.
    assert_eq!(
        f.refused(RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-account"})),
        (
            Some(ErrorCode::InvalidArgument),
            "invalid_request".to_owned()
        )
    );
    // A run left waiting for an approval: the lock cannot save its cancel.
    f.insert_run(&session, "waiting_approval", None);
    let mut other = config.clone();
    other["id"] = json!("other");
    let busy = (Some(ErrorCode::Conflict), "runtime_busy".to_owned());
    assert_eq!(
        f.refused(RUNTIME_LOGIN_START, json!({"id": "codex-account"})),
        busy
    );
    assert_eq!(
        f.refused(RUNTIME_REMOVE, json!({"id": "codex-account"})),
        busy
    );
    assert_eq!(f.refused(RUNTIME_CONFIG_SAVE, other.clone()), busy);
    // As the idle auto-lock does.
    f.services.vault().lock();
    let locked = (Some(ErrorCode::VaultLocked), "vault_locked".to_owned());
    assert_eq!(
        f.refused(RUNTIME_LOGIN_START, json!({"id": "codex-account"})),
        locked
    );
    assert_eq!(
        f.refused(RUNTIME_REMOVE, json!({"id": "codex-account"})),
        locked
    );
    assert_eq!(f.refused(RUNTIME_CONFIG_SAVE, other), locked);
}

#[test]
fn account_statuses_the_cli_confirmed_through_health_or_a_login_are_kept() {
    let f = fixture();
    f.ok("runtime.add", account_runtime("codex-account", None));
    let folder = tempfile::tempdir().unwrap();
    let credential = fixture_credential(folder.path());
    f.ok(
        RUNTIME_PROFILE_IMPORT,
        json!({"id": "codex-account", "source": credential.to_string_lossy()}),
    );
    // A daemon whose cache never saw the import (as after a restart) learns
    // the account's status from a health check.
    let mut router = Router::new();
    register(&mut router, &f.services).unwrap();
    crate::methods::models::register(&mut router, &f.services).unwrap();
    let call = |method: &str, params: Value| router.call(&f.context, method, params).unwrap();
    let health = call(RUNTIME_HEALTH, json!({"id": "codex-account"}));
    assert_eq!(health["authenticated"], true, "{health}");
    call("network.offline", json!({"enabled": true}));
    let health = call(RUNTIME_HEALTH, json!({"id": "codex-account"}));
    assert_eq!(health["authenticated"], true, "{health}");
    assert_eq!(health["status"], "installed_auth_unverified");
    let account = call(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-account"}));
    assert_eq!(account["status"], "authenticated_cached", "{account}");
    call("network.offline", json!({"enabled": false}));

    // A login that connected is the CLI's confirmation too.
    f.ok("runtime.add", account_runtime("codex-login", None));
    let flow = f.ok(RUNTIME_LOGIN_START, json!({"id": "codex-login"}))["flow_id"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_until("the login connects", || {
        f.ok(
            RUNTIME_LOGIN_STATUS,
            json!({"id": "codex-login", "flow_id": flow}),
        )["state"]
            == "connected"
    });
    f.ok("network.offline", json!({"enabled": true}));
    let account = f.ok(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-login"}));
    assert_eq!(account["status"], "authenticated_cached", "{account}");
    assert_eq!(account["authenticated"], true);
    // A signed-out account reports a documented health code while offline.
    f.ok("network.offline", json!({"enabled": false}));
    f.ok(RUNTIME_LOGIN_LOGOUT, json!({"id": "codex-login"}));
    f.ok("network.offline", json!({"enabled": true}));
    let health = f.ok(RUNTIME_HEALTH, json!({"id": "codex-login"}));
    assert_eq!(health["status"], "auth_required", "{health}");
    assert_eq!(health["authenticated"], false);
    // A removed runtime's confirmed status goes with it.
    f.ok("network.offline", json!({"enabled": false}));
    f.ok(RUNTIME_REMOVE, json!({"id": "codex-login"}));
    let mut again = account_runtime("codex-login", None);
    again["profile"] = account["profile"].clone();
    f.ok("runtime.add", again);
    f.ok("network.offline", json!({"enabled": true}));
    assert_eq!(
        f.refused(RUNTIME_ACCOUNT_STATUS, json!({"id": "codex-login"})),
        (Some(ErrorCode::Unavailable), "offline_refused".into())
    );
    f.assert_no_credential_text();
}

/// The branch snapshots of runtime `id` on the drive.
fn branch_snapshots(f: &Fixture, id: &str) -> Vec<String> {
    let prefix = format!("runtime-{}-", aidex_native_sessions::digest(id.as_bytes()));
    std::fs::read_dir(f.drive.path().join("private"))
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(&prefix))
        .collect()
}

/// The snapshot a run's saved reference names.
fn run_snapshot(f: &Fixture, run: &str) -> String {
    let run = f.services.native().store().run(run).unwrap();
    run.runtime_ref.unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn an_ended_runtime_thread_and_a_deleted_conversation_take_their_snapshots_with_them() {
    let f = fixture();
    f.ok("runtime.add", peer_runtime("codex-snap", "codex", "normal"));
    let first = f.runtime_session("codex-snap");
    let second = f.runtime_session("codex-snap");
    let run = f.send(&first, "first-1");
    assert_eq!(f.wait_inactive(&run), RunStatus::Completed);
    let ended = run_snapshot(&f, &run);
    let other = f.send(&second, "second-1");
    assert_eq!(f.wait_inactive(&other), RunStatus::Completed);
    let kept = run_snapshot(&f, &other);
    assert_eq!(branch_snapshots(&f, "codex-snap").len(), 2);

    // New instructions end the first conversation's thread, and its snapshot.
    let mut config = f.ok("agent.configuration.get", json!({"session_id": first}));
    config["instructions"] = json!("Answer briefly.");
    f.ok(
        "agent.configuration.set",
        json!({"session_id": first, "configuration": config}),
    );
    assert_eq!(
        branch_snapshots(&f, "codex-snap"),
        std::slice::from_ref(&kept)
    );
    assert_ne!(ended, kept);

    // A new thread, and one a daemon from before branch snapshots saved.
    let run = f.send(&first, "first-2");
    assert_eq!(f.wait_inactive(&run), RunStatus::Completed);
    assert_eq!(branch_snapshots(&f, "codex-snap").len(), 2);
    let legacy = f.drive.path().join("private").join(format!(
        "runtime-{}.sealed",
        aidex_native_sessions::digest(format!("codex-snap:{first}").as_bytes())
    ));
    std::fs::write(&legacy, vec![0u8; 64]).unwrap();
    assert_eq!(
        f.ok("conversation.delete", json!({"id": first})),
        json!({"deleted": true})
    );
    // Only the other conversation's snapshot is left.
    assert_eq!(branch_snapshots(&f, "codex-snap"), [kept]);
    assert!(!legacy.exists());
}

#[test]
fn background_reads_let_the_idle_lock_happen_while_actions_hold_it_off() {
    use aidex_providers::{
        FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
        ProviderId, ToolCallSupport,
    };
    let f = fixture();
    let fake = Arc::new(
        FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
            .with_model(
                ModelDescriptor::new(
                    ModelRef::new(
                        ProviderId::new("fixture").unwrap(),
                        ModelId::new("small").unwrap(),
                    ),
                    Locality::Local,
                    65536,
                )
                .with_tool_calls(ToolCallSupport::Native),
            )
            .with_script(FakeScript::new().then(FakeTurn::tool_call(
                "fs_write",
                r#"{"path":"effect.txt","content":"new"}"#,
            ))),
    );
    f.services
        .models()
        .registry()
        .register_shared(fake)
        .unwrap();
    let session = f.ok(
        "conversation.create",
        json!({
            "provider_id": "fixture", "model_id": "small", "mode": "agent",
            "permission_mode": "supervised", "workspace_id": f.workspace_id,
        }),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let run = f.send(&session, "write-1");
    wait_until("the run waits for its approval", || {
        f.run_status(&run) == RunStatus::WaitingApproval
    });
    // A person's action (here a secret write) holds the idle lock off...
    f.services
        .vault()
        .set_idle_timeout(Some(Duration::from_millis(800)));
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(1600) {
        f.ok(
            "vault.secret.set",
            json!({"name": "fixture-action", "value": "fixture-value", "consumers": ["provider:fixture"]}),
        );
        std::thread::sleep(Duration::from_millis(150));
    }
    assert_eq!(f.services.vault().status().state, VaultState::Unlocked);
    // ...while the desktop's background polls, however frequent, do not.
    let polls = [
        ("vault.status", Value::Null),
        ("vault.secret.list", Value::Null),
        ("conversation.list", Value::Null),
        ("conversation.get", json!({"id": session})),
        ("approval.list", json!({"status": "pending", "limit": 1})),
        ("execution.status", Value::Null),
        ("run.get", json!({"id": run})),
        ("history.list", json!({"session_id": session})),
        ("checkpoint.list", json!({"session_id": session})),
    ];
    let deadline = Instant::now() + Duration::from_secs(20);
    while f.services.vault().status().state == VaultState::Unlocked {
        assert!(
            Instant::now() < deadline,
            "background polls held the vault open"
        );
        for (method, params) in &polls {
            let _ = f.call(method, params.clone());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // The lock cleanup stopped the waiting run, as when nobody polls.
    f.services.vault().set_idle_timeout(None);
    f.ok("vault.unlock", json!({"passphrase": PASSPHRASE}));
    assert_eq!(f.run_status(&run), RunStatus::Interrupted);
}
