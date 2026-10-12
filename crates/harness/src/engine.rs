use crate::{
    attachments::{CONTEXT_TOO_LARGE_FOR_RUNTIME, Prefetched, unsupported},
    broker::{Broker, Prepared},
    extensions::{Extensions, NoExtensions},
    native_tools::{ExtensionPlan, McpTarget},
};
use aidex_core::{CancelToken, EntityId, WorkspacePath};
use aidex_events::{EventKind, EventScope, EventSink};
use aidex_native_sessions::*;
use aidex_providers::{ModelDescriptor, ProviderRegistry};
use aidex_workspace::WorkspaceRegistry;
use std::{
    collections::{HashMap, HashSet},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};
/// The error of a run whose thread panicked.
const RUN_CRASHED: &str = "native run crashed unexpectedly";
/// Lock one of the engine's mutexes even when a panicking thread poisoned it.
/// Each critical section is a single lookup, insert, or remove (`operations`
/// guards no data at all), so the contents stay consistent across a panic,
/// and one crashed run must not refuse every later one.
pub(crate) fn relock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
/// A call waiting for the user's approval: a workspace tool plan or a
/// harness-level (web, MCP) plan.
pub(crate) enum PendingPlan {
    Tool(Box<Prepared>),
    Extension(Box<ExtensionPlan>),
}
impl PendingPlan {
    fn call_id(&self) -> &EntityId {
        match self {
            Self::Tool(prepared) => prepared.plan.call_id(),
            Self::Extension(plan) => &plan.call_id,
        }
    }
}
/// Per-request options of [`NativeEngine::start_with`].
pub use aidex_native_sessions::RunOptions;
/// Most skills one message may invoke.
pub const MAX_INVOKED_SKILLS: usize = 8;
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
    pub(crate) pending: Mutex<HashMap<String, PendingPlan>>,
    pub(crate) runtimes: Mutex<HashMap<String, Arc<dyn RuntimeExecutor>>>,
    pub(crate) operations: Mutex<()>,
    pub(crate) extensions: Arc<dyn Extensions>,
    /// Random per-engine key for untrusted-data delimiter nonces.
    pub(crate) nonce_key: String,
    /// The MCP tools each run's latest turn offered, which its calls resolve
    /// against (names shift when other servers come or go).
    pub(crate) offered_mcp: Mutex<HashMap<String, Vec<McpTarget>>>,
    /// Attachment bytes read for each running run, by attachment id. Kept
    /// while the run's thread works and dropped when it stops.
    pub(crate) attachment_cache: Mutex<HashMap<String, Prefetched>>,
    /// For each running run, the models (`provider/model`) that refused a
    /// request carrying attachments of earlier messages: the run's later
    /// steps go to them without those. In memory only, kept and dropped
    /// with the run's attachment bytes.
    pub(crate) refused_earlier: Mutex<HashMap<String, HashSet<String>>>,
}
impl NativeEngine {
    pub fn new(
        store: Arc<SessionStore>,
        registry: Arc<ProviderRegistry>,
        workspaces: Arc<WorkspaceRegistry>,
        events: Arc<dyn EventSink>,
        scratch: PathBuf,
    ) -> Arc<Self> {
        Self::with_extensions(
            store,
            registry,
            workspaces,
            events,
            scratch,
            Arc::new(NoExtensions),
        )
    }
    /// [`new`](Self::new) with skills, web, and MCP services from the embedder.
    pub fn with_extensions(
        store: Arc<SessionStore>,
        registry: Arc<ProviderRegistry>,
        workspaces: Arc<WorkspaceRegistry>,
        events: Arc<dyn EventSink>,
        scratch: PathBuf,
        extensions: Arc<dyn Extensions>,
    ) -> Arc<Self> {
        // The nonce key only needs to be unpredictable to content authors; a
        // generated id carries 80 random bits.
        let nonce_key = EntityId::generate("nonce")
            .map(|id| id.to_string())
            .unwrap_or_else(|_| format!("{:?}", std::time::SystemTime::now()));
        Arc::new(Self {
            store,
            registry,
            broker: Broker::new(workspaces, scratch, Arc::clone(&extensions)),
            events,
            live: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            runtimes: Mutex::new(HashMap::new()),
            operations: Mutex::new(()),
            extensions,
            nonce_key,
            offered_mcp: Mutex::new(HashMap::new()),
            attachment_cache: Mutex::new(HashMap::new()),
            refused_earlier: Mutex::new(HashMap::new()),
        })
    }
    /// The skills, web, and MCP services this engine was built with.
    pub fn extensions(&self) -> &Arc<dyn Extensions> {
        &self.extensions
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
        self.start_with(session, content, command, parent, RunOptions::default())
    }
    /// [`start`](Self::start) with invoked skills, the per-request web
    /// switch, and staged attachments. Refusals start with a reason
    /// ([`crate::start_refusal`] splits them): `skill_unavailable:`,
    /// `web_unavailable:`, `attachments_unsupported:` (an external runtime,
    /// or a kind, part, or size the session's model cannot take),
    /// `context_too_large_for_runtime:`, and the store's attachment reasons
    /// (`attachment_unavailable:` for unknown, foreign, sent, or duplicated
    /// ids and for attachments on a child run, `too_large:`). Nothing is
    /// stored when a check fails. A command id the session already records
    /// skips the checks and returns its original run (or a fingerprint
    /// conflict). External runtimes receive the text unchanged and ignore
    /// skills and web search.
    pub fn start_with(
        self: &Arc<Self>,
        session: &str,
        content: &str,
        command: &str,
        parent: Option<String>,
        options: RunOptions,
    ) -> Result<Run, String> {
        // A new native message with attachments is checked against the
        // session's model, whose catalog lookup may list the provider over
        // the network. It runs without the engine-wide lock, which is then
        // taken again; the session must still name the model that was
        // checked.
        let mut resolved: Option<(ModelKey, Result<ModelDescriptor, String>)> = None;
        let mut lookups = 0;
        let (_guard, s, model) = loop {
            let guard = relock(&self.operations);
            let s = self.store.get(session)?;
            let Some(key) = attachment_model_key(&s, command, parent.is_some(), &options) else {
                break (guard, s, None);
            };
            if let Some((_, model)) = resolved.take_if(|(checked, _)| *checked == key) {
                break (guard, s, Some(model));
            }
            if lookups == MAX_MODEL_LOOKUPS {
                return Err(
                    "the conversation's model changed while the message was being sent; send it \
                     again"
                        .into(),
                );
            }
            drop(guard);
            lookups += 1;
            let model = self.primary_model(&s.configuration).map(|(_, model)| model);
            resolved = Some((key, model));
        };
        if s.configuration.mode == Mode::Agent && s.configuration.workspace_id.is_none() {
            return Err("agent mode requires a selected workspace".into());
        }
        let external = s
            .configuration
            .runtime_id
            .as_deref()
            .is_some_and(|id| id != "native");
        // A retried command returns its run from `begin_with`: its ids are
        // already sent and the conversation already holds its message.
        let retry = s.commands.contains_key(command);
        if !retry && !options.attachments.is_empty() {
            if parent.is_some() {
                return Err(AttachmentReason::AttachmentUnavailable
                    .error("attachments can only be sent with a conversation message"));
            }
            if external {
                return Err(unsupported(
                    "Attachments work with AiDEX native models; this runtime cannot receive files yet.",
                ));
            }
        }
        if external {
            let runtime = self.runtime_for(&s.configuration)?;
            if !retry {
                self.check_runtime_context(&s, &runtime.descriptor(), content, parent.is_some())?;
            }
        } else {
            let provider = aidex_providers::ProviderId::new(&s.configuration.provider_id)
                .map_err(|e| e.to_string())?;
            if self.registry.provider(&provider).is_none() {
                return Err("configured provider is unavailable".into());
            }
        }
        aidex_providers::ModelId::new(&s.configuration.model_id).map_err(|e| e.to_string())?;
        let options = if external || parent.is_some() {
            RunOptions::default()
        } else {
            self.check_options(&s.configuration, options)?
        };
        // `model` is set exactly for a new native root message with
        // attachments (child and external ones were refused above).
        let prefetched = match model {
            Some(model) => {
                Some(self.check_attachments(&s, content, &options.attachments, &model?)?)
            }
            None => None,
        };
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
        let r = self
            .store
            .begin_with(session, content, command, parent, &options)?;
        if let Some(bytes) = prefetched
            && r.status == RunStatus::Pending
        {
            self.remember_attachments(&r.id, bytes);
        }
        if external && r.status == RunStatus::Pending {
            let request = self.runtime_request(&r, RuntimeOperation::Start)?;
            if let Err(error) = self.runtime_for(&s.configuration)?.validate(&request) {
                self.transition(&r.id, RunStatus::Failed, Some(error.to_string()))?;
                return Err(error.to_string());
            }
        }
        if r.status == RunStatus::Pending
            && let Err(error) = self.launch(&r)
        {
            self.forget_attachments(&r.id);
            return Err(error);
        }
        Ok(r)
    }
    /// Refuse an external-runtime run before anything is stored when the
    /// conversation it would receive exceeds the runtime's bounds: more than
    /// 1 MiB of message text, a prompt frame over 1 MiB, or, for runtimes
    /// that take the prompt as one command-line argument (Claude Code,
    /// Cursor), more than this platform allows in one argument
    /// ([`argv_limit_exceeded`]).
    fn check_runtime_context(
        &self,
        s: &SessionDetail,
        runtime: &RuntimeDescriptor,
        content: &str,
        child: bool,
    ) -> Result<(), String> {
        let mut messages = if child {
            Vec::new()
        } else {
            s.messages.clone()
        };
        messages.push(aidex_providers::ChatMessage::user(content));
        if !child {
            let mut turn = crate::attachments::TurnAttachments {
                own: Some(messages.len() - 1),
                ..Default::default()
            };
            turn.by_position = self.store.message_attachments(&s.id)?;
            turn.by_position.retain(|index, _| {
                messages
                    .get(*index)
                    .is_some_and(|m| m.role == aidex_providers::Role::User)
            });
            crate::attachments::placeholders_only(&mut messages, &turn, &runtime.name);
        }
        let refuse = |detail: String| Err(format!("{CONTEXT_TOO_LARGE_FOR_RUNTIME}: {detail}"));
        let text: usize = messages.iter().map(|m| m.content.len()).sum();
        if text > RUNTIME_TEXT_BYTES {
            return refuse(format!(
                "This conversation is too long for {} (at most 1 MiB of messages). Continue in a \
                 new conversation or with an AiDEX native model.",
                runtime.name
            ));
        }
        let prompt = runtime_prompt(&s.configuration.instructions, &messages);
        let frame = serde_json::to_string(&prompt).map_or(usize::MAX, |json| json.len());
        if frame.saturating_add(RUNTIME_FRAME_RESERVE) > RUNTIME_FRAME_BYTES {
            return refuse(format!(
                "This conversation is too long for {} (its request would exceed 1 MiB). Continue \
                 in a new conversation or with an AiDEX native model.",
                runtime.name
            ));
        }
        if ARGV_RUNTIMES.contains(&runtime.name.as_str())
            && let Some(limit) = argv_limit_exceeded(ArgvPlatform::HOST, &prompt)
        {
            return refuse(format!(
                "This conversation is too long for {}, which receives it as one command-line \
                 argument (at most {limit} on this system). Continue in a new conversation or \
                 with an AiDEX native model.",
                runtime.name
            ));
        }
        Ok(())
    }
    /// Deduplicate and validate invoked skills and the web switch.
    fn check_options(
        &self,
        config: &SessionConfig,
        options: RunOptions,
    ) -> Result<RunOptions, String> {
        let mut skills: Vec<String> = Vec::new();
        for id in options.skills {
            let id = id.trim().to_owned();
            if !id.is_empty() && !skills.contains(&id) {
                skills.push(id);
            }
        }
        if skills.len() > MAX_INVOKED_SKILLS {
            return Err(format!(
                "skill_unavailable: at most {MAX_INVOKED_SKILLS} skills can be invoked at once"
            ));
        }
        if !skills.is_empty() {
            self.extensions
                .invoked_skills(config.workspace_id.as_deref(), &skills)
                .map_err(|error| match error {
                    crate::extensions::ExtensionError::SkillUnavailable(message) => {
                        format!("skill_unavailable: {message}")
                    }
                    other => format!("skill_unavailable: {other}"),
                })?;
        }
        if options.web_search && config.mode == Mode::Chat {
            let web = self.extensions.web();
            if !web.search_ready() {
                return Err(match web.problem {
                    Some(problem) if web.enabled => format!("web_unavailable: {problem}"),
                    _ => "web_unavailable: web search is turned off in settings".into(),
                });
            }
        }
        Ok(RunOptions {
            skills,
            web_search: options.web_search,
            attachments: options.attachments,
        })
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
            PendingPlan,
            aidex_providers::ToolCall,
            aidex_tools::PermitIssuer,
        )>,
        operation: RuntimeOperation,
    ) -> Result<(), String> {
        let mut live = relock(&self.live);
        if live.get(&run.id).is_some_and(|r| r.executing) {
            return Ok(());
        }
        let token = live
            .get(&run.id)
            .map(|r| r.provider.clone())
            .unwrap_or_default();
        let tool = match &prelude {
            Some((PendingPlan::Tool(prepared), _, _)) => prepared.context.cancel.clone(),
            _ => Default::default(),
        };
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
                // Last resort: a panic anywhere in the run (a provider, an
                // extension, a runtime) fails the run instead of unwinding
                // past the bookkeeping below and leaving it running forever.
                let result = catch_unwind(AssertUnwindSafe(|| {
                    match prelude {
                        Some((PendingPlan::Tool(prepared), call, issuer)) => {
                            engine.execute(&run.id, &call, &prepared, issuer)?;
                        }
                        Some((PendingPlan::Extension(plan), call, _)) => {
                            engine.execute_extension(&run.id, &call, &plan, &tool)?;
                        }
                        None => {}
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
                }))
                .unwrap_or_else(|_| Err(RUN_CRASHED.into()));
                // Attachment bytes live only while a thread works on the run.
                engine.forget_attachments(&run.id);
                if let Err(error) = result {
                    let status = if token.is_cancelled() {
                        RunStatus::Cancelled
                    } else {
                        RunStatus::Failed
                    };
                    // Settling (the store, the event sink) must not skip the
                    // registry cleanup either.
                    let _ = catch_unwind(AssertUnwindSafe(|| {
                        engine.transition(
                            &run.id,
                            status,
                            Some(aidex_events::redact_text(&error).into_owned()),
                        )
                    }));
                }
                let mut live = relock(&engine.live);
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
        let result = self.store.update(&r.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            if r.status == RunStatus::Cancelled && status != RunStatus::Cancelled {
                return Ok(r.clone());
            }
            r.status = status;
            r.error = error;
            r.updated_at = now();
            if matches!(status,RunStatus::Failed|RunStatus::Cancelled){
                for approval in &mut s.approvals{if approval.run_id==id&&approval.status=="pending"{approval.status="expired".into();}}
                for call in r.pending_calls.drain(..){
                    let message=aidex_providers::ChatMessage::tool_result(&call.id,"Tool execution did not complete successfully. Inspect the workspace before retrying; effects may be unknown after interruption.");
                    r.transcript.push(message.clone());
                    if r.parent_run_id.is_none(){s.messages.push(message);}
                }
            }
            Ok(r.clone())
        })?;
        if !result.status.active() {
            self.forget_offer(id);
        }
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
        let guard = relock(&self.operations);
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
        let prepared = relock(&self.pending)
            .remove(id)
            .ok_or("approval interrupted by daemon restart; explicitly resume run to replan")?;
        let call = run
            .pending_calls
            .first()
            .cloned()
            .ok_or("approval has no pending call")?;
        self.store.update(&run.session_id, |s| {
            let a = s
                .approvals
                .iter_mut()
                .find(|a| a.id == id)
                .ok_or("approval not found")?;
            a.status = if approved { "approved" } else { "denied" }.into();
            s.runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?
                .status = RunStatus::Running;
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
                    call_id: prepared.call_id().clone(),
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
        if let Some(live) = relock(&self.live).get(id) {
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
        let guard = relock(&self.operations);
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
        if relock(&self.live).get(id).is_some_and(|r| r.executing) {
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
        self.forget_all_attachments();
        for r in relock(&self.live).values() {
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
        self.forget_all_attachments();
        if let Ok(runs) = self.store.run_summaries() {
            for run in runs {
                if run.status.active() {
                    let _ = self.cancel(&run.id);
                }
            }
        }
        for r in relock(&self.live).values() {
            r.provider.cancel();
            r.tool.cancel();
        }
        let _ = self
            .broker
            .sandbox
            .registry()
            .terminate_all(aidex_sandbox::StopReason::Cancelled);
        self.forget_all_attachments();
    }
}

/// The session configuration a model check of attachments was made for:
/// provider id, model id, and mode.
type ModelKey = (String, String, Mode);

/// How often [`NativeEngine::start_with`] looks the model up again when the
/// session's model changes while it checks attachments.
const MAX_MODEL_LOOKUPS: usize = 3;

/// The configuration a new message's attachments must be checked against:
/// `None` without attachments, for a retried command, a child run, or an
/// external runtime (those are refused or skip the check).
fn attachment_model_key(
    s: &SessionDetail,
    command: &str,
    child: bool,
    options: &RunOptions,
) -> Option<ModelKey> {
    let external = s
        .configuration
        .runtime_id
        .as_deref()
        .is_some_and(|id| id != "native");
    let checked =
        !options.attachments.is_empty() && !child && !external && !s.commands.contains_key(command);
    checked.then(|| {
        (
            s.configuration.provider_id.clone(),
            s.configuration.model_id.clone(),
            s.configuration.mode,
        )
    })
}

/// External runtimes refuse more message text than this (1 MiB).
const RUNTIME_TEXT_BYTES: usize = 1024 * 1024;
/// Largest JSON frame a runtime protocol carries (1 MiB).
const RUNTIME_FRAME_BYTES: usize = 1024 * 1024;
/// Room in a frame for everything besides the prompt.
const RUNTIME_FRAME_RESERVE: usize = 16 * 1024;
/// Runtimes (by descriptor name) that receive the prompt as one argument.
const ARGV_RUNTIMES: [&str; 2] = ["Claude Code", "Cursor"];
/// Most UTF-16 units of a quoted prompt argument on Windows (32,767 for the
/// whole command line; the rest is left for the executable and flags).
pub(crate) const ARGV_PROMPT_UNITS: usize = 30_000;
/// Most bytes of one argument on Linux, its terminating NUL included
/// (`MAX_ARG_STRLEN`).
const LINUX_ARGUMENT_BYTES: usize = 128 * 1024;

/// Where a runtime's command line is built, for its one-argument limit.
#[derive(Clone, Copy, Debug)]
enum ArgvPlatform {
    Windows,
    Linux,
    /// macOS and the rest: their limit is on the whole command line (about
    /// 1 MiB on macOS), which the 1 MiB text and frame bounds keep below.
    Other,
}

impl ArgvPlatform {
    const HOST: Self = if cfg!(windows) {
        Self::Windows
    } else if cfg!(target_os = "linux") {
        Self::Linux
    } else {
        Self::Other
    };
}

/// The one-argument limit `prompt` exceeds on `platform`, as the refusal
/// names it, or `None` when it fits.
fn argv_limit_exceeded(platform: ArgvPlatform, prompt: &str) -> Option<String> {
    match platform {
        ArgvPlatform::Windows => (windows_argument_units(prompt) > ARGV_PROMPT_UNITS)
            .then(|| format!("{ARGV_PROMPT_UNITS} characters")),
        ArgvPlatform::Linux => (prompt.len().saturating_add(1) > LINUX_ARGUMENT_BYTES)
            .then(|| format!("{} KiB", LINUX_ARGUMENT_BYTES / 1024)),
        ArgvPlatform::Other => None,
    }
}

/// The prompt external runtimes build for a new run: the instructions, then
/// one `role: content` line per message.
pub(crate) fn runtime_prompt(
    instructions: &str,
    messages: &[aidex_providers::ChatMessage],
) -> String {
    let history = messages
        .iter()
        .map(|m| format!("{}: {}", m.role.as_str(), m.content))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{instructions}\n{history}")
}

/// UTF-16 units of `argument` once quoted for a Windows command line: wrapped
/// in quotes when it has spaces or tabs (or is empty), every `"` escaped, and
/// backslashes before a quote (or the closing quote) doubled.
pub(crate) fn windows_argument_units(argument: &str) -> usize {
    let quoted = argument.is_empty() || argument.contains([' ', '\t']);
    let mut units = if quoted { 2 } else { 0 };
    let mut backslashes = 0;
    for c in argument.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                units += backslashes + 1;
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
        units += c.len_utf16();
    }
    if quoted {
        units += backslashes;
    }
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_quoting_counts_escapes_and_utf16_units() {
        assert_eq!(windows_argument_units("abc"), 3);
        assert_eq!(windows_argument_units(""), 2);
        assert_eq!(windows_argument_units("a b"), 5);
        // `a"b` becomes `a\"b`.
        assert_eq!(windows_argument_units("a\"b"), 4);
        // `a\\"` becomes `a\\\\\"`: both backslashes doubled plus the escape.
        assert_eq!(windows_argument_units("a\\\\\""), 7);
        // A trailing backslash inside quotes is doubled.
        assert_eq!(windows_argument_units("a b\\"), 7);
        // Characters outside the BMP take two units.
        assert_eq!(windows_argument_units("\u{1F600}"), 2);
        assert_eq!(
            runtime_prompt("rules", &[aidex_providers::ChatMessage::user("hi")]),
            "rules\nuser: hi"
        );
    }

    #[test]
    fn the_one_argument_limit_depends_on_the_platform() {
        use ArgvPlatform::{Linux, Other, Windows};
        let units = |n: usize| "x".repeat(n);
        // Windows counts quoted UTF-16 units.
        assert_eq!(
            argv_limit_exceeded(Windows, &units(ARGV_PROMPT_UNITS)),
            None
        );
        assert_eq!(
            argv_limit_exceeded(Windows, &units(ARGV_PROMPT_UNITS + 1)).as_deref(),
            Some("30000 characters")
        );
        assert!(argv_limit_exceeded(Windows, &"\"".repeat(ARGV_PROMPT_UNITS / 2 + 1)).is_some());
        // Linux counts bytes, the terminating NUL included.
        assert_eq!(argv_limit_exceeded(Linux, &units(40_000)), None);
        assert_eq!(
            argv_limit_exceeded(Linux, &units(LINUX_ARGUMENT_BYTES - 1)),
            None
        );
        assert_eq!(
            argv_limit_exceeded(Linux, &units(LINUX_ARGUMENT_BYTES)).as_deref(),
            Some("128 KiB")
        );
        assert!(argv_limit_exceeded(Linux, &"é".repeat(LINUX_ARGUMENT_BYTES / 2)).is_some());
        // Elsewhere only the 1 MiB text and frame bounds apply.
        assert_eq!(argv_limit_exceeded(Other, &units(1024 * 1024)), None);
    }
}
