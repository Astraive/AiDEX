use crate::broker::{Broker, Prepared};
use aidex_core::{CancelToken, EntityId, WorkspacePath};
use aidex_events::{EventKind, EventScope, EventSink};
use aidex_native_sessions::*;
use aidex_providers::ProviderRegistry;
use aidex_workspace::WorkspaceRegistry;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub(crate) struct LiveRun {
    pub provider: CancelToken,
    pub tool: aidex_tools::CancelToken,
    pub generation: String,
    pub executing: bool,
}
pub struct NativeEngine {
    pub(crate) store: Arc<SessionStore>,
    pub(crate) registry: Arc<ProviderRegistry>,
    pub(crate) broker: Broker,
    pub(crate) events: Arc<dyn EventSink>,
    pub(crate) live: Mutex<HashMap<String, LiveRun>>,
    pub(crate) pending: Mutex<HashMap<String, Prepared>>,
    pub(crate) runtimes: Mutex<HashMap<String, Arc<dyn RuntimeExecutor>>>,
    pub(crate) operations: Mutex<()>,
    pub(crate) prompt_context: Arc<dyn crate::PromptContext>,
    pub(crate) bridge_server: Mutex<Option<Arc<aidex_runtime_bridge::BridgeServer>>>,
    pub(crate) bridge_leases: Mutex<HashMap<String, aidex_runtime_bridge::BridgeLease>>,
    pub(crate) bridge_calls: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub(crate) bridge_approvals: crate::bridge_approval::BridgeApprovals,
}
impl NativeEngine {
    pub fn new(
        store: Arc<SessionStore>,
        registry: Arc<ProviderRegistry>,
        workspaces: Arc<WorkspaceRegistry>,
        events: Arc<dyn EventSink>,
        scratch: PathBuf,
    ) -> Arc<Self> {
        Self::with_tools(
            store,
            registry,
            workspaces,
            events,
            scratch,
            aidex_tools::ToolRegistry::with_builtin_tools(),
            Arc::new(crate::EmptyPromptContext),
        )
    }
    pub fn with_tools(
        store: Arc<SessionStore>,
        registry: Arc<ProviderRegistry>,
        workspaces: Arc<WorkspaceRegistry>,
        events: Arc<dyn EventSink>,
        scratch: PathBuf,
        tools: aidex_tools::ToolRegistry,
        prompt_context: Arc<dyn crate::PromptContext>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            registry,
            broker: Broker::new(workspaces, scratch, tools, events.clone()),
            events,
            live: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            runtimes: Mutex::new(HashMap::new()),
            operations: Mutex::new(()),
            prompt_context,
            bridge_server: Mutex::new(None),
            bridge_leases: Mutex::new(HashMap::new()),
            bridge_calls: Mutex::new(HashMap::new()),
            bridge_approvals: Default::default(),
        })
    }
    pub fn store(&self) -> &Arc<SessionStore> {
        &self.store
    }
    pub fn capabilities(&self) -> serde_json::Value {
        let caps = self.broker.sandbox.capabilities();
        let filesystem = caps.supports_baseline(aidex_sandbox::SandboxLevel::Restricted);
        let network = caps.max_level.is_some()
            && caps
                .enforcement(aidex_sandbox::Requirement::NetworkDeny)
                .satisfies(aidex_sandbox::Requirement::NetworkDeny);
        serde_json::json!({"sandbox":caps,"automatic_reviewer":false,"auto_falls_back_to":"supervised","shell_requires_full_access_on_windows":cfg!(windows),"filesystem_confinement":filesystem,"network_confinement":network,"max_children":4,"max_child_depth":2,"max_steps":128,"configured_fallback":true})
    }
    pub fn start(
        self: &Arc<Self>,
        session: &str,
        content: &str,
        command: &str,
        parent: Option<String>,
    ) -> Result<Run, String> {
        let _guard = self
            .operations
            .lock()
            .map_err(|_| "native operations poisoned")?;
        let s = self.store.get(session)?;
        if s.configuration.mode == Mode::Agent && s.configuration.workspace_id.is_none() {
            return Err("agent mode requires a selected workspace".into());
        }
        let external = s
            .configuration
            .runtime_id
            .as_deref()
            .is_some_and(|id| id != "native");
        let operation = if external && parent.is_none() && s.runtime_ref.is_some() {
            if !self
                .runtime_for(&s.configuration)?
                .descriptor()
                .supports_resume
            {
                return Err("selected runtime cannot resume saved conversation; choose an explicit new session or context handoff".into());
            }
            RuntimeOperation::Resume
        } else {
            RuntimeOperation::Start
        };
        if external {
            self.runtime_for(&s.configuration)?;
        } else {
            let provider = aidex_providers::ProviderId::new(&s.configuration.provider_id)
                .map_err(|e| e.to_string())?;
            if self.registry.provider(&provider).is_none() {
                return Err("configured provider is unavailable".into());
            }
        }
        aidex_providers::ModelId::new(&s.configuration.model_id).map_err(|e| e.to_string())?;
        if let Some(parent) = &parent {
            self.broker.authorize_native(
                session,
                &s.configuration,
                aidex_policy::Action::AgentSpawn,
                None,
            )?;
            let r = self.store.run(parent)?;
            if r.session_id != session {
                return Err("child must share its parent's session and workspace".into());
            }
            if !r.status.active() {
                return Err("child requires an active parent".into());
            }
            if s.runs
                .iter()
                .filter(|r| r.parent_run_id.as_ref() == Some(parent))
                .count()
                >= 4
            {
                return Err("child count limit reached".into());
            }
            let mut depth = 1;
            let mut ancestor = r.parent_run_id;
            while let Some(a) = ancestor {
                depth += 1;
                ancestor = self.store.run(&a)?.parent_run_id;
            }
            if depth > 2 {
                return Err("child depth limit reached".into());
            }
        }
        let r = self.store.begin(session, content, command, parent)?;
        if external && r.status == RunStatus::Pending {
            let request = match self.runtime_request(&r, operation) {
                Ok(request) => request,
                Err(error) => {
                    self.transition(
                        &r.id,
                        RunStatus::Failed,
                        Some(aidex_events::redact_text(&error).into_owned()),
                    )?;
                    return Err(error);
                }
            };
            if let Err(error) = self.runtime_for(&s.configuration)?.validate(&request) {
                self.transition(&r.id, RunStatus::Failed, Some(error.to_string()))?;
                return Err(error.to_string());
            }
        }
        if r.status == RunStatus::Pending {
            if let Some(parent) = &r.parent_run_id {
                self.emit(
                    session,
                    EventKind::AgentChildSpawned {
                        agent_id: EntityId::new(&r.id).map_err(|e| e.to_string())?,
                        parent_agent_id: EntityId::new(parent).map_err(|e| e.to_string())?,
                        role: "delegated task".into(),
                    },
                );
            }
            self.launch_operation(&r, operation)?;
        }
        Ok(r)
    }
    pub(crate) fn launch(self: &Arc<Self>, run: &Run) -> Result<(), String> {
        self.launch_with(run, None, RuntimeOperation::Start)
    }
    pub(crate) fn launch_operation(
        self: &Arc<Self>,
        run: &Run,
        operation: RuntimeOperation,
    ) -> Result<(), String> {
        self.launch_with(run, None, operation)
    }
    fn launch_with(
        self: &Arc<Self>,
        run: &Run,
        prelude: Option<(
            Prepared,
            aidex_providers::ToolCall,
            aidex_tools::PermitIssuer,
        )>,
        operation: RuntimeOperation,
    ) -> Result<(), String> {
        if !self.store.run(&run.id)?.status.active() {
            return Err("cannot launch a terminal run; explicit continuation is required".into());
        }
        let mut live = self
            .live
            .lock()
            .map_err(|_| "native run registry poisoned")?;
        if live.get(&run.id).is_some_and(|r| r.executing) {
            return Ok(());
        }
        let token = live
            .get(&run.id)
            .map(|r| r.provider.clone())
            .unwrap_or_default();
        let tool = prelude
            .as_ref()
            .map(|(prepared, _, _)| prepared.context.cancel.clone())
            .unwrap_or_default();
        let generation = aidex_native_sessions::id("worker")?;
        live.insert(
            run.id.clone(),
            LiveRun {
                provider: token.clone(),
                tool: tool.clone(),
                generation: generation.clone(),
                executing: true,
            },
        );
        drop(live);
        let engine = self.clone();
        let run = run.clone();
        std::thread::Builder::new()
            .name("aidex-native-run".into())
            .spawn(move || {
                let result = (|| {
                    if let Some((prepared, call, issuer)) = prelude {
                        engine.execute(&run.id, &call, &prepared, issuer)?;
                    }
                    let session = engine.store.get(&run.session_id)?;
                    if session
                        .configuration
                        .runtime_id
                        .as_deref()
                        .is_some_and(|id| id != "native")
                    {
                        engine.execute_runtime(&run.id, operation, &token)
                    } else {
                        engine.pump(&run.id, &token, &tool)
                    }
                })();
                if let Err(error) = result {
                    let status = if token.is_cancelled() {
                        RunStatus::Cancelled
                    } else {
                        RunStatus::Failed
                    };
                    let _ = engine.transition(
                        &run.id,
                        status,
                        Some(aidex_events::redact_text(&error).into_owned()),
                    );
                }
                let mut live = engine.live.lock().unwrap_or_else(|p| p.into_inner());
                if live
                    .get(&run.id)
                    .is_some_and(|r| r.generation == generation)
                {
                    let paused_external = engine
                        .store
                        .run(&run.id)
                        .ok()
                        .is_some_and(|r| r.status == RunStatus::WaitingApproval)
                        && engine.store.get(&run.session_id).ok().is_some_and(|s| {
                            s.configuration
                                .runtime_id
                                .as_deref()
                                .is_some_and(|id| id != "native")
                        });
                    if paused_external {
                        if let Some(r) = live.get_mut(&run.id) {
                            r.executing = false;
                        }
                    } else {
                        live.remove(&run.id);
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub(crate) fn emit(&self, session: &str, event: EventKind) {
        if let Ok(s) = self.store.get(session) {
            let workspace = s
                .configuration
                .workspace_id
                .unwrap_or_else(|| "workspace_chat".into());
            if let (Ok(workspace), Ok(session)) = (EntityId::new(workspace), EntityId::new(session))
            {
                let _ = self
                    .events
                    .emit(EventScope::session(workspace, session), event);
            }
        }
    }
    pub(crate) fn transition(
        &self,
        id: &str,
        status: RunStatus,
        error: Option<String>,
    ) -> Result<Run, String> {
        let r = self.store.run(id)?;
        if status == RunStatus::Failed {
            let children = self
                .store
                .get(&r.session_id)?
                .runs
                .into_iter()
                .filter(|child| child.parent_run_id.as_deref() == Some(id) && child.status.active())
                .collect::<Vec<_>>();
            for child in children {
                self.cancel(&child.id)?;
            }
        }
        let result = self.store.update(&r.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            if !r.status.active() && status != r.status {
                return Ok(r.clone());
            }
            r.status = status;
            r.error = error;
            r.updated_at = now();
            if matches!(status,RunStatus::Failed|RunStatus::Cancelled|RunStatus::Interrupted){
                for approval in &mut s.approvals{if approval.run_id==id&&approval.status=="pending"{approval.status="expired".into();}}
                for call in r.pending_calls.drain(..){
                    let message=aidex_providers::ChatMessage::tool_result(&call.id,"Tool execution did not complete successfully. Inspect the workspace before retrying; effects may be unknown after interruption.");
                    r.transcript.push(message.clone());
                    if r.parent_run_id.is_none(){s.messages.push(message);}
                }
            }
            Ok(r.clone())
        })?;
        let task_id = EntityId::new(id).map_err(|e| e.to_string())?;
        let event = match result.status {
            RunStatus::Running => Some(EventKind::TaskStarted { task_id }),
            RunStatus::Completed => Some(EventKind::TaskCompleted { task_id }),
            RunStatus::Failed => Some(EventKind::TaskFailed {
                task_id,
                reason: "native run failed; details in encrypted run history".into(),
            }),
            RunStatus::Cancelled => Some(EventKind::TaskCancelled { task_id }),
            _ => None,
        };
        if let Some(e) = event {
            self.emit(&r.session_id, e);
        }
        if !result.status.active() {
            self.revoke_bridge(&r.session_id, id);
        }
        if result.status == RunStatus::Running {
            self.emit(
                &r.session_id,
                EventKind::AgentStarted {
                    agent_id: EntityId::new(id).map_err(|e| e.to_string())?,
                    model: None,
                },
            );
        }
        if result.status != r.status {
            let outcome = match result.status {
                RunStatus::Completed => Some(aidex_events::Outcome::Succeeded),
                RunStatus::Failed => Some(aidex_events::Outcome::Failed),
                RunStatus::Cancelled => Some(aidex_events::Outcome::Cancelled),
                _ => None,
            };
            if let Some(outcome) = outcome {
                self.emit(
                    &r.session_id,
                    EventKind::AgentFinished {
                        agent_id: EntityId::new(id).map_err(|e| e.to_string())?,
                        outcome,
                    },
                );
            }
        }
        Ok(result)
    }
    pub fn approvals(&self, run: Option<&str>) -> Result<Vec<Approval>, String> {
        let mut out = vec![];
        for s in self.store.list()? {
            if s.locked {
                return Err("vault is locked".into());
            }
            out.extend(
                self.store
                    .get(&s.id)?
                    .approvals
                    .into_iter()
                    .filter(|a| run.is_none_or(|r| r == a.run_id)),
            );
        }
        Ok(out)
    }
    pub fn respond(
        self: &Arc<Self>,
        id: &str,
        fingerprint: &str,
        approved: bool,
        command: &str,
    ) -> Result<Run, String> {
        if command.is_empty() || command.len() > 128 {
            return Err("approval command id is required".into());
        }
        let guard = self
            .operations
            .lock()
            .map_err(|_| "native operations poisoned")?;
        let approval = self
            .approvals(None)?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or("approval not found")?;
        if approval.fingerprint != fingerprint {
            return Err("approval fingerprint mismatch".into());
        }
        let run = self.store.run(&approval.run_id)?;
        if approval.status != "pending" {
            if (approved && approval.status == "approved")
                || (!approved && approval.status == "denied")
            {
                return Ok(run);
            }
            return Err("approval already decided differently".into());
        }
        if run.status != RunStatus::WaitingApproval {
            return Err("approval requires a waiting run".into());
        }
        if approval.runtime_id.is_some() {
            return self.respond_runtime(&approval, approved);
        }
        if approval.bridge_call_id.is_some() {
            return self.bridge_approvals.respond(self, &approval, approved);
        }
        let prepared = self
            .pending
            .lock()
            .map_err(|_| "pending plans poisoned")?
            .remove(id)
            .ok_or("approval interrupted by daemon restart; explicitly resume run to replan")?;
        let call = run
            .pending_calls
            .first()
            .cloned()
            .ok_or("approval has no pending call")?;
        self.store.update(&run.session_id, |s| {
            let active = s
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            if active.status != RunStatus::WaitingApproval {
                return Err("approval no longer has a waiting run".into());
            }
            let a = s
                .approvals
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or("approval not found")?;
            if a.status != "pending" || a.fingerprint != fingerprint {
                return Err("approval changed or expired".into());
            }
            a.status = if approved { "approved" } else { "denied" }.into();
            active.status = RunStatus::Running;
            Ok(())
        })?;
        if approved {
            let approval_id = EntityId::new(id).map_err(|e| e.to_string())?;
            self.emit(
                &run.session_id,
                EventKind::ApprovalApproved {
                    approval_id: approval_id.clone(),
                    scope: aidex_events::ApprovalScope::Once,
                },
            );
            let current = self.store.run(&run.id)?;
            self.launch_with(
                &current,
                Some((
                    prepared,
                    call,
                    aidex_tools::PermitIssuer::Approval { approval_id },
                )),
                RuntimeOperation::Approval,
            )?;
            return Ok(current);
        } else {
            self.emit(
                &run.session_id,
                EventKind::ApprovalDenied {
                    approval_id: EntityId::new(id).map_err(|e| e.to_string())?,
                    reason: None,
                },
            );
            self.emit(
                &run.session_id,
                EventKind::ToolDenied {
                    call_id: prepared.plan.call_id().clone(),
                    reason: "user denied native tool".into(),
                },
            );
            self.append_result(&run.id, &call.id, "Tool execution denied by user.".into())?;
        }
        drop(guard);
        let r = self.store.run(&run.id)?;
        self.launch(&r)?;
        Ok(r)
    }
    pub fn cancel(&self, id: &str) -> Result<Run, String> {
        let r = self.store.run(id)?;
        if !r.status.active() {
            return Ok(r);
        }
        if let Some(live) = self
            .live
            .lock()
            .map_err(|_| "native run registry poisoned")?
            .get(id)
        {
            live.provider.cancel();
            live.tool.cancel();
        }
        let children = self
            .store
            .get(&r.session_id)?
            .runs
            .into_iter()
            .filter(|r| r.parent_run_id.as_deref() == Some(id))
            .collect::<Vec<_>>();
        for child in children {
            self.cancel(&child.id)?;
        }
        self.transition(id, RunStatus::Cancelled, None)
    }
    pub fn resume(self: &Arc<Self>, id: &str) -> Result<Run, String> {
        let guard = self
            .operations
            .lock()
            .map_err(|_| "native operations poisoned")?;
        let r = self.store.run(id)?;
        let configuration = self.store.get(&r.session_id)?.configuration;
        let external = configuration
            .runtime_id
            .as_deref()
            .is_some_and(|id| id != "native");
        if external {
            let runtime = self.runtime_for(&configuration)?;
            if !runtime.descriptor().supports_resume || r.runtime_ref.is_none() {
                return Err(
                    "selected runtime cannot safely resume without a saved protocol reference"
                        .into(),
                );
            }
        }
        if !matches!(
            r.status,
            RunStatus::Interrupted | RunStatus::WaitingApproval
        ) {
            return Err("only an interrupted run can resume".into());
        }
        if self
            .live
            .lock()
            .map_err(|_| "native run registry poisoned")?
            .get(id)
            .is_some_and(|r| r.executing)
        {
            return Err("run is still executing".into());
        }
        self.store.update(&r.session_id, |s| {
            for approval in &mut s.approvals {
                if approval.run_id == id && approval.status == "pending" {
                    approval.status = "expired".into();
                }
            }
            let run = s.runs.iter_mut().find(|run| run.id == id).ok_or("run not found")?;
            for call in run.pending_calls.drain(..) {
                let message = aidex_providers::ChatMessage::tool_result(
                    &call.id,
                    "Tool interrupted or approval expired; no automatic retry. Ask for a fresh call if still needed.",
                );
                run.transcript.push(message.clone());
                if run.parent_run_id.is_none() {
                    s.messages.push(message);
                }
            }
            run.status = RunStatus::Pending;
            run.error = None;
            Ok(())
        })?;
        drop(guard);
        let r = self.store.run(id)?;
        if external {
            self.launch_operation(&r, RuntimeOperation::Resume)?;
        } else {
            self.launch(&r)?;
        }
        Ok(r)
    }
    pub fn compare(&self, session: &str, id: &str) -> Result<serde_json::Value, String> {
        let s = self.store.get(session)?;
        let c = s
            .checkpoints
            .iter()
            .find(|c| c.id == id)
            .ok_or("checkpoint not found")?;
        let handle = self
            .broker
            .workspaces
            .handle(
                &EntityId::new(
                    s.configuration
                        .workspace_id
                        .ok_or("checkpoint workspace missing")?,
                )
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let path = WorkspacePath::new(&c.path).map_err(|e| e.to_string())?;
        let absolute = handle
            .resolve(&path, aidex_workspace::Access::Create)
            .map_err(|e| e.to_string())?;
        let current = if absolute.exists() {
            digest(&std::fs::read(absolute).map_err(|e| e.to_string())?)
        } else {
            "absent".into()
        };
        Ok(
            serde_json::json!({"checkpoint":c,"current_digest":current,"conflict":current!=c.after_digest}),
        )
    }
    pub fn restore(&self, session: &str, id: &str, expected: &str) -> Result<(), String> {
        let s = self.store.get(session)?;
        let restore_session = s.clone();
        if s.runs.iter().any(|r| r.status.active()) {
            return Err("cancel active runs before restoring checkpoint".into());
        }
        let checkpoint = s
            .checkpoints
            .iter()
            .find(|c| c.id == id)
            .ok_or("checkpoint not found")?;
        if expected != checkpoint.after_digest {
            return Err("checkpoint digest mismatch".into());
        }
        let handle = self
            .broker
            .workspaces
            .handle(
                &EntityId::new(
                    s.configuration
                        .workspace_id
                        .ok_or("checkpoint workspace missing")?,
                )
                .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        let path = WorkspacePath::new(&checkpoint.path).map_err(|e| e.to_string())?;
        let absolute = handle
            .resolve(&path, aidex_workspace::Access::Create)
            .map_err(|e| e.to_string())?;
        let current = if absolute.exists() {
            digest(&std::fs::read(&absolute).map_err(|e| e.to_string())?)
        } else {
            "absent".into()
        };
        if current != checkpoint.after_digest {
            return Err("checkpoint conflicts with current user changes".into());
        }
        match &checkpoint.before {
            Some(before) => {
                let input = if checkpoint.after_digest == "absent" {
                    serde_json::json!({"path":checkpoint.path,"content":before,"create_only":true})
                } else {
                    serde_json::json!({"path":checkpoint.path,"content":before,"expected_sha256":checkpoint.after_digest})
                };
                let call = aidex_providers::ToolCall {
                    id: aidex_native_sessions::id("call")?,
                    name: "fs_write".into(),
                    arguments: input.to_string(),
                    provider_metadata: None,
                };
                let prepared = self.broker.prepare(&restore_session, &call)?;
                self.broker.execute(
                    &prepared,
                    aidex_tools::PermitIssuer::Approval {
                        approval_id: EntityId::new(&checkpoint.id).map_err(|e| e.to_string())?,
                    },
                )?;
            }
            None => {
                if current == "absent" {
                    return Ok(());
                }
                let call=aidex_providers::ToolCall{id:aidex_native_sessions::id("call")?,name:"fs_delete".into(),arguments:serde_json::json!({"path":checkpoint.path,"recursive":false,"expected_sha256":checkpoint.after_digest}).to_string(),provider_metadata:None};
                let prepared = self.broker.prepare(&restore_session, &call)?;
                self.broker.execute(
                    &prepared,
                    aidex_tools::PermitIssuer::Approval {
                        approval_id: EntityId::new(&checkpoint.id).map_err(|e| e.to_string())?,
                    },
                )?;
            }
        }
        Ok(())
    }
    pub fn shutdown(&self) {
        if let Ok(server) = self.bridge_server.lock()
            && let Some(server) = server.as_ref()
        {
            server.shutdown();
        }
        for r in self.live.lock().unwrap_or_else(|p| p.into_inner()).values() {
            r.provider.cancel();
            r.tool.cancel();
        }
        let _ = self
            .broker
            .sandbox
            .registry()
            .shutdown(aidex_sandbox::StopReason::DaemonShutdown);
    }
    pub fn cancel_all(&self) {
        if let Ok(pending) = self.pending.lock() {
            for prepared in pending.values() {
                prepared.context.cancel.cancel();
            }
        }
        if let Ok(mut slot) = self.bridge_server.lock()
            && let Some(server) = slot.take()
        {
            server.shutdown();
        }
        if let Ok(mut leases) = self.bridge_leases.lock() {
            leases.clear();
        }
        if let Ok(mut gates) = self.bridge_calls.lock() {
            gates.clear();
        }
        if let Ok(runs) = self.store.run_summaries() {
            for run in runs {
                if run.status.active() {
                    let _ = self.cancel(&run.id);
                }
            }
        }
        for r in self.live.lock().unwrap_or_else(|p| p.into_inner()).values() {
            r.provider.cancel();
            r.tool.cancel();
        }
        let _ = self
            .broker
            .sandbox
            .registry()
            .terminate_all(aidex_sandbox::StopReason::Cancelled);
    }
    /// A vault lock can prevent private terminal-state commits. Reconcile after
    /// unlock without reviving stopped actors or executable approval plans.
    pub fn reconcile_after_unlock(&self) -> Result<(), String> {
        for summary in self.store.run_summaries()? {
            if !summary.status.active() {
                continue;
            }
            let run = self.store.run(&summary.id)?;
            let stopped = match self
                .live
                .lock()
                .map_err(|_| "native run registry poisoned")?
                .get(&run.id)
            {
                Some(live) => live.provider.is_stopped() || live.tool.is_cancelled(),
                None => {
                    let session = self.store.get(&run.session_id)?;
                    let pending = self.pending.lock().map_err(|_| "pending plans poisoned")?;
                    !session
                        .approvals
                        .iter()
                        .filter(|approval| {
                            approval.run_id == run.id && approval.status == "pending"
                        })
                        .any(|approval| {
                            pending
                                .get(&approval.id)
                                .is_some_and(|prepared| !prepared.context.cancel.is_cancelled())
                        })
                }
            };
            if stopped {
                let approvals = self
                    .store
                    .get(&run.session_id)?
                    .approvals
                    .into_iter()
                    .filter(|approval| approval.run_id == run.id)
                    .map(|approval| approval.id)
                    .collect::<std::collections::HashSet<_>>();
                self.transition(&run.id,RunStatus::Interrupted,Some("Execution stopped while private records were locked; explicit continuation is required".into()))?;
                if let Ok(mut pending) = self.pending.lock() {
                    pending.retain(|id, _| !approvals.contains(id));
                }
                self.revoke_bridge(&run.session_id, &run.id);
            }
        }
        Ok(())
    }
}
