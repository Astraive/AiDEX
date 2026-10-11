use aidex_core::EntityId;
use aidex_drive::{DriveLayout, DrivePaths};
use aidex_extensions::*;
use aidex_tools::{CancelToken, ExecutionPermit, PermitIssuer, ToolContext, ToolId, ToolRegistry};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

struct Secrets(Arc<AtomicUsize>);
impl ExtensionSecrets for Secrets {
    fn version(&self, _: &str, _: &str) -> Result<aidex_vault::SecretVersion> {
        Ok(aidex_vault::SecretVersion {
            secret_ref: aidex_core::SecretRef {
                vault_id: EntityId::new("fixture_vault").unwrap(),
                secret_id: EntityId::new("fixture_secret").unwrap(),
            },
            revision: 1,
        })
    }
    fn resolve_versioned(
        &self,
        _: &str,
        _: &str,
        _: &aidex_vault::SecretVersion,
    ) -> Result<aidex_vault::SecretBytes> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(aidex_vault::SecretBytes::from("fixture-secret".to_owned()))
    }
}
struct Connector {
    starts: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}
struct Connection {
    calls: Arc<AtomicUsize>,
}
impl McpConnector for Connector {
    fn connect(
        &self,
        _: &McpServerConfig,
        _: &CallScope,
        _: Arc<dyn ExtensionSecrets>,
        _: &CancelToken,
    ) -> Result<Box<dyn McpConnection>> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Connection {
            calls: self.calls.clone(),
        }))
    }
}
impl McpConnection for Connection {
    fn initialize(&mut self, _: &CancelToken) -> Result<()> {
        Ok(())
    }
    fn list_tools(&mut self, _: &CancelToken) -> Result<Vec<McpTool>> {
        Ok(vec![McpTool {
            name: "echo".into(),
            description: "Echo a string".into(),
            input_schema: json!({"type":"object","properties":{"text":{"type":"string","maxLength":64}},"required":["text"],"additionalProperties":false}),
            output_schema: None,
            annotations: Some(json!({"readOnlyHint":true})),
        }])
    }
    fn call_tool(&mut self, _: &str, args: Value, cancel: &CancelToken) -> Result<Value> {
        if cancel.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"content":[{"type":"text","text":args["text"]}]}))
    }
    fn disconnect(&mut self) {}
}
fn config() -> McpServerConfig {
    McpServerConfig {
        id: "echo-server".into(),
        name: "Echo".into(),
        enabled: true,
        trusted: true,
        workspace_ids: vec!["workspace".into()],
        profiles: vec!["full_access".into()],
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
        workspace_root: path.to_path_buf(),
        profile: "full_access".into(),
        full_access: true,
        network_allowed: true,
        selected_skill_ids: vec![],
    }
}

#[test]
fn passive_catalog_and_plan_never_connect_or_resolve_secrets_and_execution_rechecks_gate() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_path_buf(), DriveLayout::default());
    let starts = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let secrets = Arc::new(AtomicUsize::new(0));
    let online = Arc::new(AtomicBool::new(true));
    let online_gate = online.clone();
    let service = ExtensionService::with_connector_and_gate(
        paths,
        Arc::new(Secrets(secrets.clone())),
        Arc::new(Connector {
            starts: starts.clone(),
            calls: calls.clone(),
        }),
        Arc::new(move || online_gate.load(Ordering::SeqCst)),
    )
    .unwrap();
    service.upsert_mcp(config()).unwrap();
    let scope = scope(dir.path());
    service.catalog_for(&scope).unwrap();
    service.list_mcp().unwrap();
    assert_eq!(starts.load(Ordering::SeqCst), 0);
    assert_eq!(secrets.load(Ordering::SeqCst), 0);
    let summary = service
        .connect_mcp("echo-server", &scope, &CancelToken::new())
        .unwrap();
    let resolved_at_connect = secrets.load(Ordering::SeqCst);
    service.bind_scope("session", scope.clone()).unwrap();
    let mut registry = ToolRegistry::new();
    for tool in service.tool_adapters() {
        registry.register(tool).unwrap();
    }
    let context = ToolContext::new(
        EntityId::new("session").unwrap(),
        EntityId::new("workspace").unwrap(),
        EntityId::new("call").unwrap(),
        Arc::new(aidex_workspace::ConfinedRoot::new(dir.path()).unwrap()),
        Arc::new(aidex_events::NullEventSink),
    );
    let args = json!({"server_id":"echo-server","tool_name":"echo","arguments":{"text":"hello"},"catalog_revision":summary.catalog_revision});
    let plan = registry
        .plan(&context, &ToolId::new("mcp.call").unwrap(), args.clone())
        .unwrap();
    assert_eq!(secrets.load(Ordering::SeqCst), resolved_at_connect);
    assert!(
        plan.intents()
            .iter()
            .any(|i| i.action == aidex_policy::Action::SecretUse)
    );
    let mut pinned = scope.clone();
    pinned.selected_skill_ids.push("selected-review".into());
    service.bind_scope("session", pinned).unwrap();
    let output = registry
        .execute(
            &context,
            &plan,
            ExecutionPermit::for_plan(&plan, PermitIssuer::Development),
        )
        .unwrap();
    assert_eq!(output.content["result"]["content"][0]["text"], "hello");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    service.bind_scope("other_session", scope.clone()).unwrap();
    let mut other = context.for_call(EntityId::new("other_call").unwrap());
    other.session_id = EntityId::new("other_session").unwrap();
    assert!(
        registry
            .plan(&other, &ToolId::new("mcp.call").unwrap(), args.clone())
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let next = context.for_call(EntityId::new("call_next").unwrap());
    let plan = registry
        .plan(&next, &ToolId::new("mcp.call").unwrap(), args)
        .unwrap();
    online.store(false, Ordering::SeqCst);
    assert!(
        registry
            .execute(
                &next,
                &plan,
                ExecutionPermit::for_plan(&plan, PermitIssuer::Development)
            )
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
#[test]
fn discovered_schemas_and_revision_bind_execution() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_path_buf(), DriveLayout::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let service = ExtensionService::with_connector(
        paths,
        Arc::new(NoSecrets),
        Arc::new(Connector {
            starts: Arc::new(AtomicUsize::new(0)),
            calls: calls.clone(),
        }),
    )
    .unwrap();
    let mut c = config();
    c.secret_refs.clear();
    service.upsert_mcp(c.clone()).unwrap();
    let scope = scope(dir.path());
    let summary = service
        .connect_mcp(&c.id, &scope, &CancelToken::new())
        .unwrap();
    service.bind_scope("session", scope).unwrap();
    let mut registry = ToolRegistry::new();
    for tool in service.tool_adapters() {
        registry.register(tool).unwrap();
    }
    let ctx = ToolContext::new(
        EntityId::new("session").unwrap(),
        EntityId::new("workspace").unwrap(),
        EntityId::new("call").unwrap(),
        Arc::new(aidex_workspace::ConfinedRoot::new(dir.path()).unwrap()),
        Arc::new(aidex_events::NullEventSink),
    );
    let args = |arguments| json!({"server_id":c.id,"tool_name":"echo","arguments":arguments,"catalog_revision":summary.catalog_revision});
    for invalid in [
        json!({}),
        json!({"text":12}),
        json!({"text":"ok","extra":true}),
    ] {
        assert!(
            registry
                .plan(&ctx, &ToolId::new("mcp.call").unwrap(), args(invalid))
                .is_err()
        )
    }
    let plan = registry
        .plan(
            &ctx,
            &ToolId::new("mcp.call").unwrap(),
            args(json!({"text":"ok"})),
        )
        .unwrap();
    let evaluation = aidex_policy::EvaluationContext::new(
        aidex_policy::Profile::Balanced,
        aidex_policy::PrivacyMode::Hybrid,
        aidex_policy::HostTrust::Trusted,
        aidex_policy::NetworkMode::Unrestricted,
    );
    for intent in plan.intents() {
        let decision =
            aidex_policy::evaluate(&aidex_policy::PolicySet::new(), &evaluation, &intent);
        assert!(!decision.verdict.is_deny(), "{}", decision.reason);
    }
    c.enabled = false;
    service.upsert_mcp(c).unwrap();
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
}

#[test]
fn plugin_scopes_enable_and_integrity_apply_to_contributed_skills() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_path_buf(), DriveLayout::default());
    let service = ExtensionService::with_connector(
        paths.clone(),
        Arc::new(NoSecrets),
        Arc::new(Connector {
            starts: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap();
    let manifest = PluginManifest {
        id: "review-tools".into(),
        name: "Review Tools".into(),
        version: "1.0.0".into(),
        description: "Review skills".into(),
        trusted: true,
        enabled: true,
        workspace_ids: vec!["workspace".into()],
        profiles: vec!["full_access".into()],
        skills: vec![PluginSkill {
            path: "skills/review/SKILL.md".into(),
        }],
        mcp_servers: vec![],
    };
    let text = "---\nname: review\ndescription: Review changed code\n---\nExplain defects.\n";
    service
        .install_plugin(
            manifest.clone(),
            BTreeMap::from([("skills/review/SKILL.md".into(), text.into())]),
        )
        .unwrap();
    let scope = scope(dir.path());
    assert_eq!(service.catalog_for(&scope).unwrap().skills.len(), 1);
    service.set_plugin_enabled(&manifest.id, false).unwrap();
    assert!(service.catalog_for(&scope).unwrap().skills.is_empty());
    service.set_plugin_enabled(&manifest.id, true).unwrap();
    std::fs::write(
        paths.plugins().join("review-tools/skills/review/SKILL.md"),
        text.replace("defects", "secrets"),
    )
    .unwrap();
    assert!(service.catalog_for(&scope).unwrap().skills.is_empty());
    std::fs::write(
        paths.plugins().join("review-tools/user-notes.txt"),
        "user data",
    )
    .unwrap();
    let mut bad = manifest;
    bad.id = "unsafe-package".into();
    bad.skills = vec![PluginSkill {
        path: "../escape/SKILL.md".into(),
    }];
    assert!(
        service
            .install_plugin(
                bad,
                BTreeMap::from([("../escape/SKILL.md".into(), text.into())])
            )
            .is_err()
    );
    assert!(!dir.path().join("escape").exists());
    assert!(service.remove_plugin("review-tools").unwrap());
    assert!(service.list_skills().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(paths.plugins().join("review-tools/user-notes.txt")).unwrap(),
        "user data"
    );
    assert!(
        paths
            .plugins()
            .join("review-tools/skills/review/SKILL.md")
            .exists()
    );
}

#[test]
fn shutdown_cancels_inflight_discovery_and_refuses_stale_publication() {
    use std::sync::{Mutex, mpsc};
    struct BlockingConnector {
        started: mpsc::Sender<()>,
        release: Mutex<Option<mpsc::Receiver<()>>>,
    }
    struct BlockingConnection {
        started: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
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
                release: self.release.lock().unwrap().take().unwrap(),
            }))
        }
    }
    impl McpConnection for BlockingConnection {
        fn initialize(&mut self, _: &CancelToken) -> Result<()> {
            self.started.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(())
        }
        fn list_tools(&mut self, _: &CancelToken) -> Result<Vec<McpTool>> {
            Ok(vec![])
        }
        fn call_tool(&mut self, _: &str, _: Value, _: &CancelToken) -> Result<Value> {
            panic!("unexpected call")
        }
        fn disconnect(&mut self) {}
    }
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_path_buf(), DriveLayout::default());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let service = ExtensionService::with_connector(
        paths,
        Arc::new(NoSecrets),
        Arc::new(BlockingConnector {
            started: started_tx,
            release: Mutex::new(Some(release_rx)),
        }),
    )
    .unwrap();
    let mut c = config();
    c.secret_refs.clear();
    service.upsert_mcp(c).unwrap();
    let scope = scope(dir.path());
    let worker = service.clone();
    let worker_scope = scope.clone();
    let cancel = CancelToken::new();
    let worker_cancel = cancel.clone();
    let handle = std::thread::spawn(move || {
        worker.connect_mcp("echo-server", &worker_scope, &worker_cancel)
    });
    started_rx.recv().unwrap();
    service.shutdown();
    assert!(cancel.is_cancelled());
    release_tx.send(()).unwrap();
    assert!(handle.join().unwrap().is_err());
    assert!(!service.list_mcp().unwrap()[0].connected);
    assert!(
        service
            .connect_mcp("echo-server", &scope, &CancelToken::new())
            .is_err()
    );
}

#[test]
fn model_cannot_guess_and_read_a_manual_only_skill() {
    let dir = tempfile::tempdir().unwrap();
    let paths = DrivePaths::from_layout(dir.path().to_path_buf(), DriveLayout::default());
    let service = ExtensionService::with_connector(
        paths,
        Arc::new(NoSecrets),
        Arc::new(Connector {
            starts: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap();
    let skill = service.import_skill(
        "---\nname: manual-only\ndescription: Selected by the user only\ndisable-model-invocation: true\n---\nManual instructions.\n",
        true, vec!["workspace".into()], vec!["full_access".into()],
    ).unwrap();
    service.bind_scope("session", scope(dir.path())).unwrap();
    let mut registry = ToolRegistry::new();
    for tool in service.tool_adapters() {
        registry.register(tool).unwrap();
    }
    let ctx = ToolContext::new(
        EntityId::new("session").unwrap(),
        EntityId::new("workspace").unwrap(),
        EntityId::new("call").unwrap(),
        Arc::new(aidex_workspace::ConfinedRoot::new(dir.path()).unwrap()),
        Arc::new(aidex_events::NullEventSink),
    );
    assert!(
        registry
            .plan(
                &ctx,
                &ToolId::new("skill.read").unwrap(),
                json!({"id":skill.id})
            )
            .is_err()
    );
    let mut selected = scope(dir.path());
    selected.selected_skill_ids.push(skill.id.clone());
    service.bind_scope("session", selected.clone()).unwrap();
    assert!(
        service
            .skill_context(&selected)
            .unwrap()
            .contains("manual-only")
    );
    let plan = registry
        .plan(
            &ctx,
            &ToolId::new("skill.read").unwrap(),
            json!({"id":skill.id}),
        )
        .unwrap();
    let evaluation = aidex_policy::EvaluationContext::new(
        aidex_policy::Profile::Balanced,
        aidex_policy::PrivacyMode::Hybrid,
        aidex_policy::HostTrust::Trusted,
        aidex_policy::NetworkMode::Unrestricted,
    );
    for intent in plan.intents() {
        let decision =
            aidex_policy::evaluate(&aidex_policy::PolicySet::new(), &evaluation, &intent);
        assert!(!decision.verdict.is_deny(), "{}", decision.reason);
    }
    assert!(
        registry
            .execute(
                &ctx,
                &plan,
                ExecutionPermit::for_plan(&plan, PermitIssuer::Development)
            )
            .unwrap()
            .content["content"]
            .as_str()
            .unwrap()
            .contains("Manual instructions")
    );
    let changed = "---\nname: manual-only\ndescription: Selected by the user only\ndisable-model-invocation: true\nuser-invocable: false\n---\nManual instructions.\n";
    service.save_skill(&skill.id, changed, None).unwrap();
    let next = ctx.for_call(EntityId::new("call_next").unwrap());
    assert!(
        registry
            .plan(
                &next,
                &ToolId::new("skill.read").unwrap(),
                json!({"id":skill.id})
            )
            .is_err()
    );
}
