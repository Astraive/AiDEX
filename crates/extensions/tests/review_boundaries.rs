//! Regression fixtures for user selection, asynchronous lifecycle, and vault revisions.
use aidex_core::EntityId;
use aidex_drive::{DriveLayout, DrivePaths};
use aidex_extensions::*;
use aidex_tools::{CancelToken, ExecutionPermit, PermitIssuer, ToolContext, ToolId, ToolRegistry};
use aidex_vault::{
    ConsumerScope, KdfParams, SecretBytes, SecretKind, SecretName, SecretPolicy, SecretString,
    VaultService, VaultServiceConfig,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const MARKER: &str = "fixture-private-token-481bc2";
struct FixtureConnector {
    tools: Vec<McpTool>,
    result: Value,
    calls: Arc<AtomicUsize>,
}
struct FixtureConnection {
    tools: Vec<McpTool>,
    result: Value,
    calls: Arc<AtomicUsize>,
}
impl McpConnector for FixtureConnector {
    fn connect(
        &self,
        _: &McpServerConfig,
        _: &CallScope,
        _: Arc<dyn ExtensionSecrets>,
        _: &CancelToken,
    ) -> Result<Box<dyn McpConnection>> {
        Ok(Box::new(FixtureConnection {
            tools: self.tools.clone(),
            result: self.result.clone(),
            calls: self.calls.clone(),
        }))
    }
}
impl McpConnection for FixtureConnection {
    fn initialize(&mut self, _: &CancelToken) -> Result<()> {
        Ok(())
    }
    fn list_tools(&mut self, _: &CancelToken) -> Result<Vec<McpTool>> {
        Ok(self.tools.clone())
    }
    fn call_tool(&mut self, _: &str, _: Value, _: &CancelToken) -> Result<Value> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.result.clone())
    }
    fn disconnect(&mut self) {}
}
fn tool() -> McpTool {
    McpTool {
        name: "echo".into(),
        description: "Echo".into(),
        input_schema: json!({"type":"object","additionalProperties":false}),
        output_schema: None,
        annotations: None,
    }
}
fn config() -> McpServerConfig {
    McpServerConfig {
        id: "fixture-server".into(),
        name: "Fixture".into(),
        enabled: true,
        trusted: true,
        workspace_ids: vec!["workspace".into()],
        profiles: vec!["supervised".into()],
        transport: McpTransport::Https {
            url: "https://example.com/mcp".into(),
        },
        secret_refs: BTreeMap::from([("Authorization".into(), "MCP_TOKEN".into())]),
        timeout_ms: 1000,
        max_output_bytes: 4096,
    }
}
fn scope(path: &std::path::Path) -> CallScope {
    CallScope {
        workspace_id: "workspace".into(),
        workspace_root: path.to_owned(),
        profile: "supervised".into(),
        full_access: false,
        network_allowed: true,
        selected_skill_ids: vec![],
    }
}
fn vault(paths: &DrivePaths) -> Arc<VaultService> {
    paths.ensure_dir(&paths.vault()).unwrap();
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
        .init(&SecretString::from("fixture-password".to_owned()), false)
        .unwrap();
    put(&vault, MARKER);
    vault
}
fn put(vault: &VaultService, text: &str) {
    vault
        .put_secret(
            &SecretName::new("MCP_TOKEN").unwrap(),
            SecretKind::ApiKey,
            SecretBytes::from(text.to_owned()),
            SecretPolicy::new([ConsumerScope::mcp("fixture-server").unwrap()]).unwrap(),
        )
        .unwrap();
}
fn registry(service: &Arc<ExtensionService>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for tool in service.tool_adapters() {
        registry.register(tool).unwrap();
    }
    registry
}
fn context(path: &std::path::Path) -> ToolContext {
    ToolContext::new(
        EntityId::new("session").unwrap(),
        EntityId::new("workspace").unwrap(),
        EntityId::new("call").unwrap(),
        Arc::new(aidex_workspace::ConfinedRoot::new(path).unwrap()),
        Arc::new(aidex_events::NullEventSink),
    )
}

#[test]
fn real_vault_rotation_invalidates_an_approved_mcp_call_before_transport() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_owned(), DriveLayout::default());
    let vault = vault(&paths);
    let calls = Arc::new(AtomicUsize::new(0));
    let service = ExtensionService::with_connector(
        paths,
        Arc::new(VaultSecrets(vault.clone())),
        Arc::new(FixtureConnector {
            tools: vec![tool()],
            result: json!({"content":[]}),
            calls: calls.clone(),
        }),
    )
    .unwrap();
    service.upsert_mcp(config()).unwrap();
    let scope = scope(dir.path());
    let summary = service
        .connect_mcp("fixture-server", &scope, &CancelToken::new())
        .unwrap();
    service.bind_scope("session", scope.clone()).unwrap();
    let registry = registry(&service);
    let ctx = context(dir.path());
    let args = json!({"server_id":"fixture-server","tool_name":"echo","arguments":{},"catalog_revision":summary.catalog_revision});
    let plan = registry
        .plan(&ctx, &ToolId::new("mcp.call").unwrap(), args.clone())
        .unwrap();
    assert!(
        !serde_json::to_string(plan.state())
            .unwrap()
            .contains(MARKER)
    );
    put(&vault, "rotated-private-token");
    assert!(
        registry
            .execute(
                &ctx,
                &plan,
                ExecutionPermit::for_plan(&plan, PermitIssuer::Development)
            )
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        registry
            .plan(
                &ctx.for_call(EntityId::new("next_call").unwrap()),
                &ToolId::new("mcp.call").unwrap(),
                args
            )
            .is_err()
    );
    service
        .connect_mcp("fixture-server", &scope, &CancelToken::new())
        .unwrap();
    vault.lock();
    assert!(
        service
            .connect_mcp("fixture-server", &scope, &CancelToken::new())
            .is_err()
    );
}

#[test]
fn credential_reflections_are_rejected_in_catalogs_and_redacted_in_results() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_owned(), DriveLayout::default());
    let vault = vault(&paths);
    for index in 0..3 {
        let mut reflected = tool();
        match index {
            0 => reflected.description = MARKER.into(),
            1 => {
                reflected.input_schema = json!({"type":"object","properties":{"token":{"type":"string","default":MARKER}}})
            }
            _ => {
                reflected.input_schema =
                    json!({"type":"object","properties":{(MARKER):{"type":"string"}}})
            }
        }
        let service = ExtensionService::with_connector(
            paths.clone(),
            Arc::new(VaultSecrets(vault.clone())),
            Arc::new(FixtureConnector {
                tools: vec![reflected],
                result: json!({"content":[]}),
                calls: Arc::new(AtomicUsize::new(0)),
            }),
        )
        .unwrap();
        service.upsert_mcp(config()).unwrap();
        if index == 0 {
            service
                .connect_mcp_async("fixture-server", &scope(dir.path()))
                .unwrap();
            let summary = complete(&service);
            assert!(!summary.connected);
            assert_eq!(
                summary.last_error.as_deref(),
                Some("MCP server returned an unsupported response")
            );
        } else {
            assert!(
                service
                    .connect_mcp("fixture-server", &scope(dir.path()), &CancelToken::new())
                    .is_err()
            );
        }
        assert!(
            !serde_json::to_string(&service.list_mcp().unwrap())
                .unwrap()
                .contains(MARKER)
        );
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let service=ExtensionService::with_connector(paths,Arc::new(VaultSecrets(vault)),Arc::new(FixtureConnector{tools:vec![tool()],result:json!({"content":[{"type":"text","text":MARKER}],"structuredContent":{(MARKER):MARKER}}),calls:calls.clone()})).unwrap();
    service.upsert_mcp(config()).unwrap();
    let scope = scope(dir.path());
    let summary = service
        .connect_mcp("fixture-server", &scope, &CancelToken::new())
        .unwrap();
    service.bind_scope("session", scope).unwrap();
    let registry = registry(&service);
    let ctx = context(dir.path());
    let plan=registry.plan(&ctx,&ToolId::new("mcp.call").unwrap(),json!({"server_id":"fixture-server","tool_name":"echo","arguments":{},"catalog_revision":summary.catalog_revision})).unwrap();
    let output = registry
        .execute(
            &ctx,
            &plan,
            ExecutionPermit::for_plan(&plan, PermitIssuer::Development),
        )
        .unwrap();
    assert!(
        !serde_json::to_string(&output.content)
            .unwrap()
            .contains(MARKER)
    );
    assert_eq!(output.content["result"]["content"][0]["text"], "[REDACTED]");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

struct BlockingConnector {
    started: mpsc::Sender<CancelToken>,
    release: Mutex<VecDeque<mpsc::Receiver<()>>>,
    closed: mpsc::Sender<()>,
}
struct BlockingConnection {
    started: mpsc::Sender<CancelToken>,
    release: mpsc::Receiver<()>,
    closed: mpsc::Sender<()>,
}
impl McpConnector for BlockingConnector {
    fn connect(
        &self,
        _: &McpServerConfig,
        _: &CallScope,
        _: Arc<dyn ExtensionSecrets>,
        _: &CancelToken,
    ) -> Result<Box<dyn McpConnection>> {
        Ok(Box::new(BlockingConnection {
            started: self.started.clone(),
            release: self.release.lock().unwrap().pop_front().unwrap(),
            closed: self.closed.clone(),
        }))
    }
}
impl McpConnection for BlockingConnection {
    fn initialize(&mut self, cancel: &CancelToken) -> Result<()> {
        self.started.send(cancel.clone()).unwrap();
        self.release.recv().unwrap();
        Ok(())
    }
    fn list_tools(&mut self, _: &CancelToken) -> Result<Vec<McpTool>> {
        Ok(vec![tool()])
    }
    fn call_tool(&mut self, _: &str, _: Value, _: &CancelToken) -> Result<Value> {
        panic!("unexpected call")
    }
    fn disconnect(&mut self) {
        let _ = self.closed.send(());
    }
}
fn complete(service: &ExtensionService) -> McpServerSummary {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let summary = service.server_summary("fixture-server").unwrap();
        if !summary.connecting {
            return summary;
        }
        assert!(Instant::now() < deadline, "connection job did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}
#[test]
fn asynchronous_connect_returns_before_initialization_and_disconnect_all_cancels_publication() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_owned(), DriveLayout::default());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    let service = ExtensionService::with_connector(
        paths,
        Arc::new(NoSecrets),
        Arc::new(BlockingConnector {
            started: started_tx,
            release: Mutex::new(VecDeque::from([release_rx])),
            closed: closed_tx,
        }),
    )
    .unwrap();
    let mut c = config();
    c.secret_refs.clear();
    service.upsert_mcp(c).unwrap();
    let summary = service
        .connect_mcp_async("fixture-server", &scope(dir.path()))
        .unwrap();
    assert!(summary.connecting);
    assert!(!summary.connected);
    let cancel = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    service.disconnect_all();
    assert!(cancel.is_cancelled());
    let summary = service.server_summary("fixture-server").unwrap();
    assert!(!summary.connecting);
    assert!(summary.last_error.is_some());
    release_tx.send(()).unwrap();
    closed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!complete(&service).connected);
}
#[test]
fn asynchronous_connect_publishes_authoritative_completion() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_owned(), DriveLayout::default());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (closed_tx, _closed_rx) = mpsc::channel();
    let service = ExtensionService::with_connector(
        paths,
        Arc::new(NoSecrets),
        Arc::new(BlockingConnector {
            started: started_tx,
            release: Mutex::new(VecDeque::from([release_rx])),
            closed: closed_tx,
        }),
    )
    .unwrap();
    let mut c = config();
    c.secret_refs.clear();
    service.upsert_mcp(c).unwrap();
    service
        .connect_mcp_async("fixture-server", &scope(dir.path()))
        .unwrap();
    started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    release_tx.send(()).unwrap();
    let summary = complete(&service);
    assert!(summary.connected);
    assert!(!summary.connecting);
    assert!(summary.last_error.is_none());
    assert_eq!(summary.tools[0].name, "echo");
}

#[test]
fn replacement_job_cancels_old_generation_without_overwriting_new_status() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_owned(), DriveLayout::default());
    let (started_tx, started_rx) = mpsc::channel();
    let (old_release_tx, old_release_rx) = mpsc::channel();
    let (new_release_tx, new_release_rx) = mpsc::channel();
    let (closed_tx, closed_rx) = mpsc::channel();
    let service = ExtensionService::with_connector(
        paths,
        Arc::new(NoSecrets),
        Arc::new(BlockingConnector {
            started: started_tx,
            release: Mutex::new(VecDeque::from([old_release_rx, new_release_rx])),
            closed: closed_tx,
        }),
    )
    .unwrap();
    let mut c = config();
    c.secret_refs.clear();
    service.upsert_mcp(c).unwrap();
    service
        .connect_mcp_async("fixture-server", &scope(dir.path()))
        .unwrap();
    let old = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    service
        .connect_mcp_async("fixture-server", &scope(dir.path()))
        .unwrap();
    assert!(old.is_cancelled());
    old_release_tx.send(()).unwrap();
    closed_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let new = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!new.is_cancelled());
    let pending = service.server_summary("fixture-server").unwrap();
    assert!(pending.connecting);
    assert!(!pending.connected);
    assert!(pending.last_error.is_none());
    new_release_tx.send(()).unwrap();
    assert!(complete(&service).connected);
}
