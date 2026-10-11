use aidex_events::MemoryEventSink;
use aidex_harness::NativeEngine;
use aidex_native_sessions::{Mode, RunStatus, SessionStore};
use aidex_providers::{
    FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef, NoVault,
    OfflineMode, ProviderId, ProviderRegistry,
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
    fixture_with_services(
        turns,
        mode,
        context,
        output,
        aidex_tools::ToolRegistry::with_builtin_tools(),
        Arc::new(aidex_harness::EmptyPromptContext),
    )
}
fn fixture_with_services(
    turns: Vec<FakeTurn>,
    mode: Mode,
    context: u32,
    output: Option<u32>,
    tools: aidex_tools::ToolRegistry,
    prompt_context: Arc<dyn aidex_harness::PromptContext>,
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
    .with_tool_calls(aidex_providers::ToolCallSupport::Native);
    descriptor.max_output_tokens = output;
    let provider = Arc::new(
        FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
            .with_model(descriptor)
            .with_script(turns.into_iter().collect::<FakeScript>()),
    );
    registry.register_shared(provider.clone()).unwrap();
    let events = Arc::new(MemoryEventSink::new());
    let engine = NativeEngine::with_tools(
        store.clone(),
        registry,
        workspaces,
        events.clone(),
        drive.path().join("scratch"),
        tools,
        prompt_context,
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

struct ExtensionTool {
    descriptor: aidex_tools::ToolDescriptor,
    effects: Arc<std::sync::atomic::AtomicUsize>,
}
impl aidex_tools::Tool for ExtensionTool {
    fn descriptor(&self) -> &aidex_tools::ToolDescriptor {
        &self.descriptor
    }
    fn plan(
        &self,
        context: &aidex_tools::ToolContext,
        input: serde_json::Value,
    ) -> Result<aidex_tools::ToolPlan, aidex_tools::ToolError> {
        aidex_tools::ToolPlan::new(
            context,
            &self.descriptor,
            aidex_tools::PlanParts::new(input, "extension invocation"),
        )
    }
    fn execute(
        &self,
        _: &aidex_tools::ToolContext,
        _: &aidex_tools::ToolPlan,
        _: &aidex_tools::ExecutionPermit,
    ) -> Result<aidex_tools::ToolOutput, aidex_tools::ToolError> {
        self.effects
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        aidex_tools::ToolOutput::typed(
            "PRIVATE_TOOL_EVIDENCE",
            &serde_json::json!({"value":"PRIVATE_TOOL_EVIDENCE"}),
        )
    }
}
fn extension_registry(
    full_access: bool,
) -> (
    aidex_tools::ToolRegistry,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let effects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut registry = aidex_tools::ToolRegistry::with_builtin_tools();
    registry
        .register(Box::new(ExtensionTool {
            descriptor: aidex_tools::ToolDescriptor {
                id: aidex_tools::ToolId::new("extension.echo").unwrap(),
                version: 1,
                description: "Fixture extension evidence".into(),
                input_schema: serde_json::json!({"type":"object","additionalProperties":false}),
                output_schema: serde_json::json!({"type":"object"}),
                risk: aidex_tools::RiskProfile::read_only(aidex_policy::Action::McpCall),
                capabilities: if full_access {
                    vec!["permissions:full_access".into()]
                } else {
                    vec![]
                },
            },
            effects: effects.clone(),
        }))
        .unwrap();
    (registry, effects)
}
struct SelectedSkillContext;
impl aidex_harness::PromptContext for SelectedSkillContext {
    fn for_run(
        &self,
        _: &aidex_native_sessions::SessionDetail,
        _: &aidex_native_sessions::Run,
        _: &aidex_workspace::ConfinedRoot,
    ) -> Result<Vec<aidex_providers::ChatMessage>, String> {
        Ok(vec![aidex_providers::ChatMessage::system(
            "Selected skill context: inspect extension evidence before completing.",
        )])
    }
}
#[test]
fn registered_extension_and_selected_skill_context_reach_real_native_loop() {
    let (tools, effects) = extension_registry(false);
    let (_d, _ws, e, s, p, events) = fixture_with_services(
        vec![
            FakeTurn::tool_call("extension_echo", "{}"),
            FakeTurn::text("complete"),
        ],
        Mode::Agent,
        65536,
        None,
        tools,
        Arc::new(SelectedSkillContext),
    );
    let run = e
        .start(&s, "use the selected extension", "extension", None)
        .unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    assert_eq!(effects.load(std::sync::atomic::Ordering::SeqCst), 1);
    let requests = p.requests();
    assert!(
        requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "extension_echo")
    );
    assert!(
        requests[0]
            .messages
            .iter()
            .any(|message| message.content.contains("Selected skill context"))
    );
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message.role == aidex_providers::Role::Tool
                && message.content.contains("PRIVATE_TOOL_EVIDENCE"))
    );
    assert!(
        !serde_json::to_string(&events.events())
            .unwrap()
            .contains("PRIVATE_TOOL_EVIDENCE")
    );
}
#[test]
fn full_access_extension_is_hidden_and_model_name_bypass_is_denied() {
    let (tools, effects) = extension_registry(true);
    let (_d, _ws, e, s, p, _events) = fixture_with_services(
        vec![FakeTurn::tool_call("extension_echo", "{}")],
        Mode::Agent,
        65536,
        None,
        tools,
        Arc::new(aidex_harness::EmptyPromptContext),
    );
    let run = e.start(&s, "invoke extension", "scope", None).unwrap();
    let final_run = wait(&e, &run.id);
    assert_eq!(final_run.status, RunStatus::Failed);
    assert!(final_run.error.unwrap().contains("full_access"));
    assert!(
        !p.requests()[0]
            .tools
            .iter()
            .any(|tool| tool.name == "extension_echo")
    );
    assert_eq!(effects.load(std::sync::atomic::Ordering::SeqCst), 0);
}
#[test]
fn explicit_full_access_offers_and_executes_registered_extension() {
    let (tools, effects) = extension_registry(true);
    let (_d, _ws, e, s, p, _events) = fixture_with_services(
        vec![
            FakeTurn::tool_call("extension_echo", "{}"),
            FakeTurn::text("complete"),
        ],
        Mode::Agent,
        65536,
        None,
        tools,
        Arc::new(aidex_harness::EmptyPromptContext),
    );
    e.store()
        .update(&s, |session| {
            session.configuration.permission_mode =
                aidex_native_sessions::PermissionMode::FullAccess;
            Ok(())
        })
        .unwrap();
    let run = e.start(&s, "invoke extension", "scope", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    assert!(
        p.requests()[0]
            .tools
            .iter()
            .any(|tool| tool.name == "extension_echo")
    );
    assert_eq!(effects.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn effect_free_planning_error_reaches_model_for_repair() {
    let (_drive, ws, e, s, p, _events) = fixture(
        vec![
            FakeTurn::tool_call(
                "fs_patch",
                r#"{"path":"missing.txt","edits":[{"old":"x","new":"y"}]}"#,
            ),
            FakeTurn::text("The file is absent; no edit was applied."),
        ],
        Mode::Agent,
    );
    let r = e.start(&s, "repair the file", "repair", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    let requests = p.requests();
    assert_eq!(requests.len(), 2);
    let result = requests[1]
        .messages
        .iter()
        .find(|m| m.role == aidex_providers::Role::Tool)
        .unwrap();
    assert!(result.content.contains("not_found"));
    assert!(!ws.path().join("missing.txt").exists());
}

#[test]
fn project_instructions_are_bounded_context_in_native_request() {
    let (_drive, ws, e, s, p, _events) = fixture(vec![FakeTurn::text("ready")], Mode::Agent);
    std::fs::write(ws.path().join("AGENTS.md"), "Use the fixture test command.").unwrap();
    let r = e
        .start(&s, "inspect this project", "instructions", None)
        .unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert!(
        p.requests()[0]
            .messages
            .iter()
            .any(|m| m.content.contains("Use the fixture test command.")
                && m.content.contains("AGENTS.md"))
    );
}

#[test]
fn denied_project_context_read_never_reaches_model() {
    let (drive, ws, e, s, p, _events) =
        fixture(vec![FakeTurn::text("must not be invoked")], Mode::Agent);
    std::fs::write(ws.path().join("AGENTS.md"), "private instruction marker").unwrap();
    let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
    std::fs::write(paths.policies().join("deny-context-read.toml"),"schema_version=1\nscope=\"global\"\n[[rules]]\nid=\"deny-context-read\"\naction=\"fs.read\"\nverdict=\"deny\"\n").unwrap();
    let run = e
        .start(&s, "inspect project", "denied-context", None)
        .unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Failed);
    assert!(p.requests().is_empty());
}

#[test]
fn file_effects_publish_diff_events_and_one_tool_lifecycle() {
    let (_drive, ws, e, s, _p, events) = fixture(
        vec![
            FakeTurn::tool_call("fs_write", r#"{"path":"result.txt","content":"written"}"#),
            FakeTurn::text("done"),
        ],
        Mode::Agent,
    );
    e.store()
        .update(&s, |session| {
            session.configuration.permission_mode =
                aidex_native_sessions::PermissionMode::AutoAcceptEdits;
            Ok(())
        })
        .unwrap();
    let r = e.start(&s, "write", "events", None).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Completed);
    assert_eq!(
        std::fs::read_to_string(ws.path().join("result.txt")).unwrap(),
        "written"
    );
    let names = events.names();
    assert!(names.contains(&"file.changed"));
    assert!(names.contains(&"diff.created"));
    assert_eq!(names.iter().filter(|n| **n == "tool.started").count(), 1);
    assert_eq!(names.iter().filter(|n| **n == "tool.completed").count(), 1);
}

#[test]
fn root_waits_for_child_result_before_final_completion() {
    let (_drive, _ws, e, s, p, events) = fixture(
        vec![
            FakeTurn::text("root draft").with_delay(Duration::from_millis(200)),
            FakeTurn::text("child evidence").with_delay(Duration::from_millis(600)),
            FakeTurn::text("integrated child evidence"),
        ],
        Mode::Agent,
    );
    let root = e.start(&s, "parent", "join-root", None).unwrap();
    while p.requests().is_empty() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let child = e
        .start(&s, "child", "join-child", Some(root.id.clone()))
        .unwrap();
    assert_eq!(wait(&e, &root.id).status, RunStatus::Completed);
    assert_eq!(
        e.store().run(&child.id).unwrap().status,
        RunStatus::Completed
    );
    assert_eq!(
        e.store()
            .run(&root.id)
            .unwrap()
            .transcript
            .last()
            .unwrap()
            .content,
        "integrated child evidence"
    );
    let recorded = events.events();
    let child_text = recorded
        .iter()
        .filter_map(|event| match &event.event {
            aidex_events::EventKind::AgentRunDelta { agent_id, text }
                if agent_id.as_str() == child.id =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<String>();
    assert_eq!(child_text, "child evidence");
    assert!(recorded.iter().any(|event|matches!(&event.event,aidex_events::EventKind::AgentChildSpawned {agent_id,parent_agent_id,..} if agent_id.as_str()==child.id&&parent_agent_id.as_str()==root.id)));
    let legacy = recorded
        .iter()
        .filter_map(|event| match &event.event {
            aidex_events::EventKind::AgentDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    let root_text = recorded
        .iter()
        .filter_map(|event| match &event.event {
            aidex_events::EventKind::AgentRunDelta { agent_id, text }
                if agent_id.as_str() == root.id =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<String>();
    assert_eq!(
        legacy, root_text,
        "child output leaked into root-only deltas"
    );
}

#[test]
fn failed_parent_cancels_active_child_work() {
    let (_d, _ws, e, s, p, _events) = fixture(
        vec![
            FakeTurn::error(aidex_providers::ProviderError::Protocol(
                "parent failure".into(),
            ))
            .with_delay(Duration::from_millis(200)),
            FakeTurn::text("child must stop").with_delay(Duration::from_secs(3)),
        ],
        Mode::Agent,
    );
    let root = e.start(&s, "parent", "failed-root", None).unwrap();
    while p.requests().is_empty() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let child = e
        .start(&s, "child", "failed-root-child", Some(root.id.clone()))
        .unwrap();
    assert_eq!(wait(&e, &root.id).status, RunStatus::Failed);
    assert_eq!(
        e.store().run(&child.id).unwrap().status,
        RunStatus::Cancelled
    );
}

#[test]
fn transient_inference_failure_retries_without_replaying_tools() {
    let (_d, _ws, e, s, p, _events) = fixture(
        vec![
            FakeTurn::error(aidex_providers::ProviderError::unavailable("transient")),
            FakeTurn::text("recovered"),
        ],
        Mode::Chat,
    );
    let run = e.start(&s, "continue", "retry", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    assert_eq!(p.requests().len(), 2);
    assert_eq!(e.store().run(&run.id).unwrap().inference_retries, 1);
}

#[test]
fn inference_retries_consume_the_remaining_run_step_budget() {
    let (_d, _ws, e, s, p, _events) = fixture(
        vec![
            FakeTurn::error(aidex_providers::ProviderError::unavailable("first")),
            FakeTurn::error(aidex_providers::ProviderError::unavailable("second")),
            FakeTurn::text("must not be invoked"),
        ],
        Mode::Chat,
    );
    e.store()
        .update(&s, |session| {
            session.configuration.max_steps = 2;
            Ok(())
        })
        .unwrap();
    let run = e.start(&s, "continue", "retry-budget", None).unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Failed);
    assert_eq!(p.requests().len(), 2);
}

#[test]
fn oversized_history_replay_keeps_recent_tool_pairs_and_durable_history() {
    let (_d, _ws, e, s, p, _events) = fixture_with_limits(
        vec![FakeTurn::text("continued")],
        Mode::Chat,
        2048,
        Some(256),
    );
    e.store()
        .update(&s, |session| {
            let mut old = aidex_providers::ChatMessage::assistant("");
            old.tool_calls.push(aidex_providers::ToolCall {
                id: "old-call".into(),
                name: "fs_read".into(),
                arguments: r#"{"path":"old.txt"}"#.into(),
                provider_metadata: None,
            });
            let mut recent = aidex_providers::ChatMessage::assistant("");
            recent.tool_calls.push(aidex_providers::ToolCall {
                id: "recent-call".into(),
                name: "fs_read".into(),
                arguments: r#"{"path":"keep.txt"}"#.into(),
                provider_metadata: None,
            });
            session.messages = vec![
                aidex_providers::ChatMessage::user("old task"),
                old,
                aidex_providers::ChatMessage::tool_result("old-call", "x".repeat(20000)),
                recent,
                aidex_providers::ChatMessage::tool_result("recent-call", "keep recent evidence"),
            ];
            Ok(())
        })
        .unwrap();
    let run = e
        .start(&s, "use the recent evidence", "context", None)
        .unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    let request = &p.requests()[0];
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("recent-call"))
    );
    assert!(
        !request
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("old-call"))
    );
    for message in &request.messages {
        if let Some(id) = &message.tool_call_id {
            assert!(
                request
                    .messages
                    .iter()
                    .any(|m| m.tool_calls.iter().any(|c| &c.id == id))
            );
        }
    }
    assert!(request.estimated_input_tokens() + request.max_tokens.unwrap() <= 2048);
    assert!(
        e.store()
            .get(&s)
            .unwrap()
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("old-call") && m.content.len() == 20000)
    );
}

#[test]
fn latest_large_tool_result_is_clipped_only_in_replay_view() {
    let (_d, _ws, e, s, p, _events) = fixture_with_limits(
        vec![FakeTurn::text("continued")],
        Mode::Chat,
        2048,
        Some(256),
    );
    e.store().update(&s,|session|{
        let mut message=aidex_providers::ChatMessage::assistant("");
        message.tool_calls.push(aidex_providers::ToolCall{id:"latest-call".into(),name:"fs_read".into(),arguments:r#"{"path":"large.txt"}"#.into(),provider_metadata:None});
        session.messages=vec![aidex_providers::ChatMessage::user("inspect the large file"),message,aidex_providers::ChatMessage::tool_result("latest-call",serde_json::json!({"path":"large.txt","content":"x".repeat(20000),"sha256":"fixture-digest"}).to_string())];
        Ok(())
    }).unwrap();
    let run = e
        .start(&s, "continue with the file", "large-context", None)
        .unwrap();
    assert_eq!(wait(&e, &run.id).status, RunStatus::Completed);
    let request = &p.requests()[0];
    let result = request
        .messages
        .iter()
        .find(|m| m.tool_call_id.as_deref() == Some("latest-call"))
        .unwrap();
    assert!(result.content.contains("fixture-digest"));
    assert!(result.content.len() < 8000);
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.tool_calls.iter().any(|call| call.id == "latest-call"))
    );
    assert!(
        e.store()
            .get(&s)
            .unwrap()
            .messages
            .iter()
            .any(|m| m.tool_call_id.as_deref() == Some("latest-call") && m.content.len() > 20000)
    );
}

#[test]
fn repository_task_records_failing_and_passing_test_output_edits_and_git_diff() {
    let test_command = if cfg!(windows) {
        serde_json::json!({"program":"findstr.exe","args":["/x","new","result.txt"]})
    } else {
        serde_json::json!({"program":"grep","args":["-x","new","result.txt"]})
    };
    let (_d, ws, e, s, p, events) = fixture(
        vec![
            FakeTurn::tool_call("fs_read", r#"{"path":"result.txt"}"#),
            FakeTurn::tool_call(
                "fs_patch",
                r#"{"path":"result.txt","edits":[{"old":"missing","new":"new"}]}"#,
            ),
            FakeTurn::tool_call("shell_exec", test_command.to_string()),
            FakeTurn::tool_call(
                "fs_patch",
                r#"{"path":"result.txt","edits":[{"old":"old","new":"new"}]}"#,
            ),
            FakeTurn::tool_call("shell_exec", test_command.to_string()),
            FakeTurn::tool_call("git_diff", r#"{"mode":"worktree"}"#),
            FakeTurn::text("The file is fixed, its check passes, and the diff shows the edit."),
        ],
        Mode::Agent,
    );
    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(ws.path())
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let line_ending = if cfg!(windows) { "\r\n" } else { "\n" };
    std::fs::write(ws.path().join("result.txt"), format!("old{line_ending}")).unwrap();
    let add = std::process::Command::new("git")
        .args(["add", "--", "result.txt"])
        .current_dir(ws.path())
        .output()
        .unwrap();
    assert!(add.status.success());
    e.store()
        .update(&s, |session| {
            session.configuration.permission_mode =
                aidex_native_sessions::PermissionMode::FullAccess;
            session.configuration.max_steps = 32;
            Ok(())
        })
        .unwrap();
    let run = e
        .start(
            &s,
            "fix the repository and verify it",
            "repository-proof",
            None,
        )
        .unwrap();
    let finished = wait(&e, &run.id);
    assert_eq!(
        finished.status,
        RunStatus::Completed,
        "{:?}",
        finished.error
    );
    assert_eq!(
        std::fs::read_to_string(ws.path().join("result.txt")).unwrap(),
        format!("new{line_ending}")
    );
    let outputs = finished
        .transcript
        .iter()
        .filter(|message| message.role == aidex_providers::Role::Tool)
        .map(|message| serde_json::from_str::<serde_json::Value>(&message.content).unwrap())
        .collect::<Vec<_>>();
    assert!(
        outputs
            .iter()
            .any(|output| output.get("code").and_then(|value| value.as_str())
                == Some("patch_failed"))
    );
    assert!(
        outputs
            .iter()
            .any(|output| output["content"]["exit_code"] == 1)
    );
    assert!(
        outputs.iter().any(|output| {
            output["content"]["exit_code"] == 0
                && output["content"]["stdout"]
                    .as_str()
                    .is_some_and(|text| text.contains("new"))
        }),
        "tool outputs: {outputs:?}"
    );
    assert!(
        outputs
            .iter()
            .any(|output| output["content"].to_string().contains("-old")
                && output["content"].to_string().contains("+new"))
    );
    assert!(outputs.iter().any(|output| !output["diff"].is_null()));
    assert!(events.names().contains(&"diff.created"));
    assert_eq!(p.requests().len(), 7);
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
    let (drive, ws, e, s, _p, events) = fixture(
        vec![
            FakeTurn::tool_call(
                "shell_exec",
                r#"{"program":"cmd.exe","args":["/D","/C","echo started>started.txt & echo ready & ping -n 11 127.0.0.1>nul & echo finished>finished.txt"]}"#,
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
    let until = Instant::now() + Duration::from_secs(5);
    while !ws.path().join("started.txt").exists() || !events.events().iter().any(|event| matches!(&event.event, aidex_events::EventKind::ProcessOutput {chunk,..} if chunk.contains("ready"))) {
        let state = e.store().run(&r.id).unwrap();
        assert!(state.status.active(), "{:?}", state.error);
        assert!(Instant::now() < until, "process readiness missing; lifecycle={:?}", events.names());
        std::thread::sleep(Duration::from_millis(20));
    }
    e.cancel(&r.id).unwrap();
    assert_eq!(wait(&e, &r.id).status, RunStatus::Cancelled);
    let until = Instant::now() + Duration::from_secs(5);
    while !events.names().contains(&"process.exited") {
        assert!(
            Instant::now() < until,
            "managed process did not exit after cancellation"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(events.names().contains(&"process.started"));
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
