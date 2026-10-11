use aidex_core::{CancelToken, WorkspacePath};
use aidex_events::MemoryEventSink;
use aidex_harness::{EmptyPromptContext, NativeEngine, PromptContext};
use aidex_native_sessions::*;
use aidex_providers::{ChatMessage, NoVault, OfflineMode, ProviderRegistry};
use aidex_runtime_bridge::BridgeConnection;
use aidex_tools::*;
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
struct Holding {
    request: Mutex<Option<RuntimeRequest>>,
    changed: Condvar,
    released: Mutex<bool>,
    executions: AtomicUsize,
    bridge: bool,
    vendor_approval: AtomicBool,
    defer_cancel: AtomicBool,
}
impl Holding {
    fn request(&self) -> RuntimeRequest {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut request = self.request.lock().unwrap();
        loop {
            if let Some(value) = request.as_ref() {
                return value.clone();
            }
            assert!(Instant::now() < deadline);
            request = self
                .changed
                .wait_timeout(request, Duration::from_millis(20))
                .unwrap()
                .0;
        }
    }
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.changed.notify_all();
    }
}
impl RuntimeExecutor for Holding {
    fn descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            id: "sdk-fixture".into(),
            name: "fixture".into(),
            modes: vec![Mode::Agent],
            permission_modes: vec![PermissionMode::FullAccess, PermissionMode::Supervised],
            supports_resume: true,
            supports_approvals: true,
        }
    }
    fn supports_mcp_bridge(&self) -> bool {
        self.bridge
    }
    fn validate(&self, _: &RuntimeRequest) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn execute(
        &self,
        request: &RuntimeRequest,
        event: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        *self.request.lock().unwrap() = Some(request.clone());
        self.changed.notify_all();
        let mut released = self.released.lock().unwrap();
        while !*released
            && (!cancel.is_stopped() || self.defer_cancel.load(Ordering::SeqCst))
            && !self.vendor_approval.load(Ordering::SeqCst)
        {
            released = self
                .changed
                .wait_timeout(released, Duration::from_millis(20))
                .unwrap()
                .0;
        }
        drop(released);
        if self.vendor_approval.load(Ordering::SeqCst) && !cancel.is_stopped() {
            assert_eq!(
                event(&RuntimeEvent::ApprovalRequest {
                    request_id: "vendor-overlap".into(),
                    tool_name: "vendor_write".into(),
                    arguments: json!({"path":"different.txt"}),
                    summary: "vendor approval".into(),
                    runtime_ref: json!({"thread_id":"fixture-native"})
                }),
                RuntimeControl::Pause
            );
            return Ok(RuntimeResponse {
                status: RuntimeStatus::WaitingApproval,
                runtime_ref: Some(json!({"thread_id":"fixture-native"})),
                message: None,
            });
        }
        Ok(RuntimeResponse {
            status: RuntimeStatus::Completed,
            runtime_ref: Some(json!({"thread_id":"fixture-native"})),
            message: Some(ChatMessage::assistant("complete")),
        })
    }
}
struct SourceTool {
    descriptor: ToolDescriptor,
    reads: Arc<AtomicUsize>,
}
impl Tool for SourceTool {
    fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }
    fn plan(&self, context: &ToolContext, input: Value) -> Result<ToolPlan, ToolError> {
        let path = WorkspacePath::new("source.md").unwrap();
        let source = context
            .root
            .resolve(&path, aidex_workspace::Access::Read)
            .map_err(ToolError::Workspace)?;
        let bytes = std::fs::read(&source).unwrap();
        let facts = PlanFacts {
            paths: vec![PlanPath {
                path: "source.md".into(),
                display: source.display().to_string(),
                access: PathAccess::Read,
                inside_workspace: true,
                exists: true,
                kind: Some(EntryKind::File),
                size: Some(bytes.len() as u64),
                sha256: Some(aidex_native_sessions::digest(&bytes)),
                sensitive: false,
            }],
            ..Default::default()
        };
        ToolPlan::new(
            context,
            &self.descriptor,
            PlanParts::new(input, "read selected source").with_facts(facts),
        )
    }
    fn execute(
        &self,
        context: &ToolContext,
        _: &ToolPlan,
        _: &ExecutionPermit,
    ) -> Result<ToolOutput, ToolError> {
        context.cancel.check()?;
        let path = context
            .root
            .resolve(
                &WorkspacePath::new("source.md").unwrap(),
                aidex_workspace::Access::Read,
            )
            .map_err(ToolError::Workspace)?;
        let content = std::fs::read_to_string(path).unwrap();
        self.reads.fetch_add(1, Ordering::SeqCst);
        ToolOutput::typed("PRIVATE_BRIDGE_CONTENT", &json!({"content":content}))
    }
}
struct Fixture {
    drive: tempfile::TempDir,
    _workspace: tempfile::TempDir,
    engine: Arc<NativeEngine>,
    session: String,
    runtime: Arc<Holding>,
    reads: Arc<AtomicUsize>,
    events: Arc<MemoryEventSink>,
    vault: Arc<VaultService>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.runtime.release();
        self.engine.shutdown();
    }
}
fn fixture(skill: bool, bridge: bool) -> Fixture {
    fixture_with_context(skill, bridge, Arc::new(EmptyPromptContext))
}
fn fixture_with_context(skill: bool, bridge: bool, context: Arc<dyn PromptContext>) -> Fixture {
    let drive = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
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
        .init(&SecretString::from("fixture passphrase".to_owned()), false)
        .unwrap();
    let store = Arc::new(
        SessionStore::open(paths.sessions().join("native.sqlite3"), vault.clone()).unwrap(),
    );
    let workspaces = Arc::new(aidex_workspace::WorkspaceRegistry::open(&paths).unwrap());
    let selected = workspaces.add_host(workspace.path(), None).unwrap();
    let registry = Arc::new(
        ProviderRegistry::load(
            &paths,
            Arc::new(NoVault::default()),
            Arc::new(OfflineMode::new(false)),
        )
        .unwrap(),
    );
    let reads = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::with_builtin_tools();
    tools.register(Box::new(SourceTool{descriptor:ToolDescriptor{id:ToolId::new(if skill{"skill.read"}else{"web.fetch"}).unwrap(),version:1,description:"fixture source".into(),input_schema:json!({"type":"object","properties":if skill{json!({"id":{"type":"string"}})}else{json!({"url":{"type":"string"}})},"additionalProperties":false}),output_schema:json!({"type":"object"}),risk:RiskProfile::read_only(if skill{aidex_policy::Action::FsRead}else{aidex_policy::Action::McpCall}),capabilities:vec![]},reads:reads.clone()})).unwrap();
    std::fs::write(
        workspace.path().join("source.md"),
        "PRIVATE_BRIDGE_CONTENT: apply selected guidance",
    )
    .unwrap();
    let events = Arc::new(MemoryEventSink::new());
    let engine = NativeEngine::with_tools(
        store.clone(),
        registry,
        workspaces,
        events.clone(),
        drive.path().join("scratch"),
        tools,
        context,
    );
    let session=store.create(serde_json::from_value(json!({"runtime_id":"sdk-fixture","workspace_id":selected.id,"mode":"agent","provider_id":"unused","model_id":"fixture-model","permission_mode":"full_access","max_steps":64,"skill_ids":if skill{vec!["selected-guide"]}else{vec![]} })).unwrap()).unwrap();
    let runtime = Arc::new(Holding {
        request: Mutex::new(None),
        changed: Condvar::new(),
        released: Mutex::new(false),
        executions: AtomicUsize::new(0),
        bridge,
        vendor_approval: AtomicBool::new(false),
        defer_cancel: AtomicBool::new(false),
    });
    engine
        .register_runtime("sdk-fixture", runtime.clone())
        .unwrap();
    Fixture {
        drive,
        _workspace: workspace,
        engine,
        session: session.id,
        runtime,
        reads,
        events,
        vault,
    }
}
fn http(connection: &BridgeConnection, body: Value) -> Value {
    let target = connection.endpoint().strip_prefix("http://").unwrap();
    let (address, path) = target.split_once('/').unwrap();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let text = body.to_string();
    write!(stream,"POST /{path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",connection.authorization_header(),text.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap_or(Value::Null)
}
fn init(connection: &BridgeConnection) {
    assert!(http(connection,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}))["result"].is_object());
    http(
        connection,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    );
}
fn wait_approval(f: &Fixture, run: &str) -> Approval {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(approval) = f
            .engine
            .approvals(Some(run))
            .unwrap()
            .into_iter()
            .find(|approval| approval.status == "pending")
        {
            return approval;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn ask_policy(f: &Fixture, verdict: &str) {
    let paths = aidex_drive::DrivePaths::open(f.drive.path()).unwrap();
    let approval = if verdict == "require_approval" {
        "approval_scopes=[\"once\"]\n"
    } else {
        ""
    };
    std::fs::write(paths.policies().join("bridge.toml"),format!("schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"bridge-gate\"\naction=\"mcp.call\"\nverdict=\"{verdict}\"\n{approval}")).unwrap();
}
#[test]
fn sdk_bridge_approval_is_bound_and_does_not_restart_vendor_actor() {
    let f = fixture(false, true);
    ask_policy(&f, "require_approval");
    let run = f
        .engine
        .start(&f.session, "use source", "once", None)
        .unwrap();
    let request = f.runtime.request();
    let connection = request.bridge.clone().unwrap();
    init(&connection);
    let call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{"url":"https://example.com"}}});
    let copy = connection.clone();
    let body = call.clone();
    let worker = std::thread::spawn(move || http(&copy, body));
    let approval = wait_approval(&f, &run.id);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    assert!(
        f.engine
            .respond(&approval.id, "wrong", true, "wrong")
            .is_err()
    );
    f.engine
        .respond(&approval.id, &approval.fingerprint, true, "approve")
        .unwrap();
    let output = worker.join().unwrap();
    assert_eq!(output["result"]["isError"], false);
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);
    assert_eq!(http(&connection, call), output);
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);
    assert_eq!(f.runtime.executions.load(Ordering::SeqCst), 1);
    assert!(
        !serde_json::to_string(&f.events.events())
            .unwrap()
            .contains("PRIVATE_BRIDGE_CONTENT")
    );
    assert!(
        !serde_json::to_string(&request)
            .unwrap()
            .contains(&connection.authorization_header())
    );
    f.runtime.release();
    let end = Instant::now() + Duration::from_secs(5);
    while f.engine.store().run(&run.id).unwrap().status.active() {
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        f.engine.store().run(&run.id).unwrap().status,
        RunStatus::Completed
    );
    assert_eq!(
        f.engine
            .store()
            .get(&f.session)
            .unwrap()
            .messages
            .last()
            .unwrap()
            .content,
        "complete"
    );
}
#[test]
fn denied_bridge_approval_executes_nothing() {
    let f = fixture(false, true);
    ask_policy(&f, "require_approval");
    let run = f
        .engine
        .start(&f.session, "use source", "once", None)
        .unwrap();
    let connection = f.runtime.request().bridge.unwrap();
    init(&connection);
    let worker = std::thread::spawn(move || {
        http(
            &connection,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
        )
    });
    let approval = wait_approval(&f, &run.id);
    f.engine
        .respond(&approval.id, &approval.fingerprint, false, "deny")
        .unwrap();
    assert_eq!(worker.join().unwrap()["result"]["isError"], true);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    f.runtime.release();
}
#[test]
fn cancellation_expires_pending_bridge_without_effects() {
    let f = fixture(false, true);
    ask_policy(&f, "require_approval");
    let run = f
        .engine
        .start(&f.session, "use source", "once", None)
        .unwrap();
    let connection = f.runtime.request().bridge.unwrap();
    init(&connection);
    let worker = std::thread::spawn(move || {
        http(
            &connection,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
        )
    });
    let approval = wait_approval(&f, &run.id);
    f.engine.cancel(&run.id).unwrap();
    assert_eq!(worker.join().unwrap()["result"]["isError"], true);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "stale")
            .is_err()
    );
    f.runtime.release();
}
#[test]
fn hard_policy_denial_never_becomes_bridge_approval_or_effect() {
    let f = fixture(false, true);
    ask_policy(&f, "deny");
    let run = f
        .engine
        .start(&f.session, "use source", "once", None)
        .unwrap();
    let connection = f.runtime.request().bridge.unwrap();
    init(&connection);
    let output = http(
        &connection,
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
    );
    assert_eq!(output["result"]["isError"], true);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    assert!(f.engine.approvals(Some(&run.id)).unwrap().is_empty());
    f.runtime.release();
}
#[test]
fn selected_full_skill_content_reaches_unsupported_sdk_without_function_hint() {
    let f = fixture(true, false);
    let run = f
        .engine
        .start(&f.session, "follow selected skill", "once", None)
        .unwrap();
    let request = f.runtime.request();
    assert!(request.instructions.contains("PRIVATE_BRIDGE_CONTENT"));
    assert!(!request.instructions.contains("load with skill_read"));
    assert!(request.bridge.is_none());
    assert!(f.reads.load(Ordering::SeqCst) > 0);
    f.runtime.release();
    f.engine.cancel(&run.id).unwrap();
}

fn selected_source_policy(f: &Fixture, verdict: &str) {
    let paths = aidex_drive::DrivePaths::open(f.drive.path()).unwrap();
    let approval = if verdict == "require_approval" {
        "approval_scopes=[\"once\"]\n"
    } else {
        ""
    };
    std::fs::write(paths.policies().join("selected-source.toml"),format!("schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"selected-source\"\naction=\"fs.read\"\nverdict=\"{verdict}\"\n{approval}[rules.when]\npaths=[\"source.md\"]\n")).unwrap();
}
#[test]
fn selected_source_hard_denial_prevents_sdk_prompt_and_read() {
    let f = fixture(true, true);
    selected_source_policy(&f, "deny");
    assert!(
        f.engine
            .start(&f.session, "use selected guide", "selected-denied", None)
            .is_err()
    );
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.runtime.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.engine.store().get(&f.session).unwrap().runs[0].status,
        RunStatus::Failed
    );
}
#[test]
fn selected_source_approval_is_deferred_to_actual_scoped_tool_call() {
    let f = fixture(true, true);
    selected_source_policy(&f, "require_approval");
    let run = f
        .engine
        .start(&f.session, "use selected guide", "selected-ask", None)
        .unwrap();
    let request = f.runtime.request();
    assert!(!request.instructions.contains("PRIVATE_BRIDGE_CONTENT"));
    assert!(
        request
            .instructions
            .contains("Load it with the offered skill_read")
    );
    let connection = request.bridge.unwrap();
    init(&connection);
    let worker = std::thread::spawn(move || {
        http(
            &connection,
            json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"skill_read","arguments":{"id":"selected-guide"}}}),
        )
    });
    let approval = wait_approval(&f, &run.id);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    f.engine
        .respond(
            &approval.id,
            &approval.fingerprint,
            true,
            "read selected guide",
        )
        .unwrap();
    assert_eq!(worker.join().unwrap()["result"]["isError"], false);
    assert_eq!(f.reads.load(Ordering::SeqCst), 1);
}
#[test]
fn restart_invalidates_waiting_bridge_approval_without_replaying() {
    let f = fixture(false, true);
    ask_policy(&f, "require_approval");
    let run = f
        .engine
        .start(&f.session, "use source", "restart", None)
        .unwrap();
    let connection = f.runtime.request().bridge.unwrap();
    init(&connection);
    let worker = std::thread::spawn(move || {
        http(
            &connection,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
        )
    });
    let approval = wait_approval(&f, &run.id);
    let paths = aidex_drive::DrivePaths::open(f.drive.path()).unwrap();
    let restarted =
        SessionStore::open(paths.sessions().join("native.sqlite3"), f.vault.clone()).unwrap();
    assert_eq!(
        restarted.run(&run.id).unwrap().status,
        RunStatus::Interrupted
    );
    assert_eq!(
        restarted.get(&f.session).unwrap().approvals[0].status,
        "expired"
    );
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "stale")
            .is_err()
    );
    f.engine.shutdown();
    assert_eq!(worker.join().unwrap()["result"]["isError"], true);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
}
#[test]
fn unlock_reconciles_stopped_waiter_without_resuming_or_effects() {
    let f = fixture(false, true);
    // Keep the stopped SDK actor present until reconciliation. The production
    // unlock handler also accepts a cancellation already committed after unlock.
    f.runtime.defer_cancel.store(true, Ordering::SeqCst);
    ask_policy(&f, "require_approval");
    let run = f
        .engine
        .start(&f.session, "use source", "vault-lock", None)
        .unwrap();
    let connection = f.runtime.request().bridge.unwrap();
    init(&connection);
    let worker = std::thread::spawn(move || {
        http(
            &connection,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
        )
    });
    let approval = wait_approval(&f, &run.id);
    f.vault.lock();
    f.engine.cancel_all();
    assert_eq!(worker.join().unwrap()["result"]["isError"], true);
    f.vault
        .unlock(&SecretString::from("fixture passphrase".to_owned()))
        .unwrap();
    f.engine.reconcile_after_unlock().unwrap();
    assert_eq!(
        f.engine.store().run(&run.id).unwrap().status,
        RunStatus::Interrupted
    );
    assert_eq!(
        f.engine.approvals(Some(&run.id)).unwrap()[0].status,
        "expired"
    );
    assert!(
        f.engine
            .respond(&approval.id, &approval.fingerprint, true, "stale")
            .is_err()
    );
    assert_eq!(f.runtime.executions.load(Ordering::SeqCst), 1);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn vendor_approval_cannot_overlap_pending_bridge_decision() {
    let f = fixture(false, true);
    ask_policy(&f, "require_approval");
    let run = f
        .engine
        .start(&f.session, "use source", "overlap", None)
        .unwrap();
    let connection = f.runtime.request().bridge.unwrap();
    init(&connection);
    let worker = std::thread::spawn(move || {
        http(
            &connection,
            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"web_fetch","arguments":{}}}),
        )
    });
    let approval = wait_approval(&f, &run.id);
    f.runtime.vendor_approval.store(true, Ordering::SeqCst);
    f.runtime.changed.notify_all();
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        if !f.engine.store().run(&run.id).unwrap().status.active() {
            break;
        }
        assert!(Instant::now() < end);
        std::thread::sleep(Duration::from_millis(10));
    }
    let current = f.engine.store().run(&run.id).unwrap();
    assert_eq!(current.status, RunStatus::Failed);
    let approvals = f.engine.approvals(Some(&run.id)).unwrap();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].id, approval.id);
    assert_eq!(approvals[0].status, "expired");
    assert_eq!(worker.join().unwrap()["result"]["isError"], true);
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
}

struct FullCatalog;
impl PromptContext for FullCatalog {
    fn for_run(
        &self,
        _: &SessionDetail,
        _: &Run,
        _: &aidex_workspace::ConfinedRoot,
    ) -> Result<Vec<ChatMessage>, String> {
        Ok(vec![ChatMessage::system(
            "Discovered catalogue entry\n".repeat(2000),
        )])
    }
}
#[test]
fn selected_content_is_prioritized_over_full_discovery_catalog() {
    let f = fixture_with_context(true, true, Arc::new(FullCatalog));
    f.engine
        .start(&f.session, "use selected guide", "selected-budget", None)
        .unwrap();
    assert!(
        f.runtime
            .request()
            .instructions
            .contains("PRIVATE_BRIDGE_CONTENT")
    );
}
#[test]
fn external_model_policy_denial_prevents_context_read_and_sdk_launch() {
    let f = fixture(true, true);
    let paths = aidex_drive::DrivePaths::open(f.drive.path()).unwrap();
    std::fs::write(paths.policies().join("deny-model.toml"),"schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"deny-model\"\naction=\"model.invoke\"\nverdict=\"deny\"\n").unwrap();
    assert!(
        f.engine
            .start(&f.session, "use guide", "model-denied", None)
            .is_err()
    );
    assert_eq!(f.reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.runtime.executions.load(Ordering::SeqCst), 0);
    assert_eq!(
        f.engine.store().get(&f.session).unwrap().runs[0].status,
        RunStatus::Failed
    );
}
