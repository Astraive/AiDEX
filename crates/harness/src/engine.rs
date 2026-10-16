use crate::{
    attachments::{CONTEXT_TOO_LARGE_FOR_RUNTIME, Prefetched, unsupported},
    broker::{Broker, Prepared},
    extensions::{Extensions, NoExtensions},
    native_tools::{ExtensionPlan, McpTarget},
    runtimes::RuntimeLaunch,
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
    /// Whether the run was stopped since this plan was held: a workspace
    /// tool plan keeps the tool token of the thread that planned it.
    fn stopped(&self) -> bool {
        match self {
            Self::Tool(prepared) => prepared.context.cancel.is_cancelled(),
            Self::Extension(_) => false,
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
    /// The executor of an external run paused for approval, which keeps its
    /// process for the decision until the run continues or is released
    /// ([`RuntimeExecutor::release`]); `None` for anything else.
    pub runtime: Option<Arc<dyn RuntimeExecutor>>,
    /// The approvals (ids) the run's current launch of an external runtime
    /// asked for, kept through its approval continuations, which go on with
    /// the same process: a request id is matched only among these.
    pub approvals: Vec<String>,
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
    /// Runs a [`NativeEngine::cancel`] is working on, with how many cancels
    /// of each are under way: a launch meanwhile gets stopped tokens.
    pub(crate) cancelling: Mutex<HashMap<String, usize>>,
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
            cancelling: Mutex::new(HashMap::new()),
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
    /// stored when one of these checks fails; an external runtime's policy,
    /// request, or validation refusal, checked once the run is stored,
    /// leaves it `failed`. A command id the session already records
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
        // A new root message continues the conversation's native thread only
        // when the saved reference is bound to a native branch and snapshot;
        // everything else (and a refused resume) replays the transcript.
        let mut runtime_launch = RuntimeLaunch::Start;
        if external {
            let descriptor = self.runtime_for(&s.configuration)?.descriptor();
            if parent.is_none()
                && descriptor.supports_resume
                && s.runtime_ref.as_ref().is_some_and(is_resumable_reference)
            {
                runtime_launch = RuntimeLaunch::FollowUp;
            }
            if !retry {
                self.check_runtime_context(
                    &s,
                    &descriptor,
                    content,
                    parent.is_some(),
                    runtime_launch.operation(),
                )?;
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
            // A parent being cancelled is no longer active either; the store
            // checks again where it records the child.
            if !r.status.active() || relock(&self.cancelling).contains_key(parent) {
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
            // The run is stored now: a refusal (policy, request, validation)
            // fails it instead of leaving it pending.
            match self.check_launch(&r, &s.configuration, runtime_launch) {
                Ok(launch) => runtime_launch = launch,
                Err(error) => {
                    self.transition(
                        &r.id,
                        RunStatus::Failed,
                        Some(aidex_events::redact_text(&error).into_owned()),
                    )?;
                    return Err(error);
                }
            }
        }
        if r.status == RunStatus::Pending
            && let Err(error) = self.launch_with(&r, None, runtime_launch, true)
        {
            self.forget_attachments(&r.id);
            return Err(error);
        }
        Ok(r)
    }
    /// Refuse an external-runtime run before anything is stored when what it
    /// would send exceeds the runtime's bounds ([`check_runtime_bounds`]): the
    /// whole conversation for `Start`, and for `Resume` only what the resumed
    /// native thread has not seen ([`resume_messages`]): the new message, and
    /// before it any turn that did not complete since the thread's saved
    /// reference.
    fn check_runtime_context(
        &self,
        s: &SessionDetail,
        runtime: &RuntimeDescriptor,
        content: &str,
        child: bool,
        operation: RuntimeOperation,
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
        crate::tool_view::tool_results_for_model(&mut messages);
        if operation == RuntimeOperation::Resume {
            resume_messages(&mut messages, first_unseen(s, s.runtime_ref.as_ref(), None));
        }
        check_runtime_bounds(&runtime.name, &s.configuration.instructions, &messages)
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
    /// Launch a run again (a native run after an approval decision or a
    /// resume).
    pub(crate) fn launch(self: &Arc<Self>, run: &Run) -> Result<(), String> {
        self.launch_with(run, None, RuntimeLaunch::Start, false)
    }
    /// [`launch`](Self::launch) telling an external runtime what to do.
    pub(crate) fn launch_operation(
        self: &Arc<Self>,
        run: &Run,
        launch: RuntimeLaunch,
    ) -> Result<(), String> {
        self.launch_with(run, None, launch, false)
    }
    /// Start a thread working on `run`. `fresh` for a run just stored by
    /// [`start_with`](Self::start_with): when no thread can be started, it
    /// fails, while a relaunched run (an approval decision, a resume)
    /// returns to `interrupted`, so an explicit resume can try again.
    fn launch_with(
        self: &Arc<Self>,
        run: &Run,
        prelude: Option<(
            PendingPlan,
            aidex_providers::ToolCall,
            aidex_tools::PermitIssuer,
        )>,
        launch: RuntimeLaunch,
        fresh: bool,
    ) -> Result<(), String> {
        // A finished, cancelled, or interrupted run continues only through an
        // explicit `resume`, which makes it pending first. (A run that cannot
        // be read here fails in its thread.)
        if self
            .store
            .run(&run.id)
            .is_ok_and(|stored| !stored.status.active())
        {
            return Err("cannot launch a run that is no longer active".into());
        }
        let mut live = relock(&self.live);
        if live.get(&run.id).is_some_and(|r| r.executing) {
            return Ok(());
        }
        let token = live
            .get(&run.id)
            .map(|r| r.provider.clone())
            .unwrap_or_default();
        // An approval continuation goes on with the paused launch's process.
        let approvals = match launch {
            RuntimeLaunch::Approval => live
                .get(&run.id)
                .map(|r| r.approvals.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let tool = match &prelude {
            Some((PendingPlan::Tool(prepared), _, _)) => prepared.context.cancel.clone(),
            _ => Default::default(),
        };
        // A cancel of this run, or of its parent (which cancels it next),
        // under way may already have looked at the registry: the new thread
        // starts stopped, so it settles the run without running anything.
        let cancelling = relock(&self.cancelling);
        if cancelling.contains_key(&run.id)
            || run
                .parent_run_id
                .as_ref()
                .is_some_and(|parent| cancelling.contains_key(parent))
        {
            token.cancel();
            tool.cancel();
        }
        drop(cancelling);
        let generation = aidex_native_sessions::id("worker")?;
        let mut displaced = live.insert(
            run.id.clone(),
            LiveRun {
                provider: token.clone(),
                tool: tool.clone(),
                generation: generation.clone(),
                executing: true,
                runtime: None,
                approvals,
            },
        );
        drop(live);
        // The runtime paused for the approval this launch continues: released
        // when the launch ends any other way than paused again, also when it
        // fails before its runtime is known (a locked store).
        let paused_runtime = displaced.as_mut().and_then(|entry| entry.runtime.take());
        let thread_paused_runtime = paused_runtime.clone();
        let engine = self.clone();
        let thread_run = run.clone();
        let thread_generation = generation.clone();
        let spawned = spawn_run_thread(move || {
            let run = thread_run;
            let generation = thread_generation;
            let paused_runtime = thread_paused_runtime;
            // Last resort: a panic anywhere in the run (a provider, an
            // extension, a runtime) fails the run instead of unwinding
            // past the bookkeeping below and leaving it running forever.
            // `Ok(true)`: an external runtime paused for approval.
            let result = catch_unwind(AssertUnwindSafe(|| {
                match prelude {
                    // Stopped before the thread began (a cancel under way):
                    // the approved effect never runs, and the turn below
                    // settles the run.
                    Some(_) if token.is_cancelled() || tool.is_cancelled() => {}
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
                    engine.execute_runtime(&run.id, launch, &token)
                } else {
                    engine.pump(&run.id, &token, &tool).map(|()| false)
                }
            }))
            .unwrap_or_else(|_| Err(RUN_CRASHED.into()));
            // Attachment bytes live only while a thread works on the run.
            engine.forget_attachments(&run.id);
            let paused = matches!(result, Ok(true));
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
            // Released while this thread still counts as executing, so no
            // resume launches the run anew meanwhile.
            if !paused && let Some(runtime) = paused_runtime {
                runtime.release(&run.id);
            }
            // A paused runtime keeps its entry (`execute_runtime` marked
            // it no longer executing before the pause was saved), without
            // reading the store again: a vault locked meanwhile must not
            // drop a process that may still hold credentials. A cancel
            // that wins the race removes it once the run is cancelled. A
            // native run that paused for approval (marked the same way
            // before its pause was saved) has no process left, so its
            // entry goes now, unless an approval already relaunched the
            // run under a new generation.
            let mut live = relock(&engine.live);
            if !paused
                && live
                    .get(&run.id)
                    .is_some_and(|r| r.generation == generation)
            {
                live.remove(&run.id);
            }
        });
        if let Err(error) = spawned {
            self.abandon_launch(run, &generation, paused_runtime, fresh);
            return Err(error);
        }
        Ok(())
    }
    /// Undo a launch whose thread could not be started: drop its registry
    /// entry, so nothing counts it as executing, settle the run (a new run
    /// fails, and a relaunched one returns to `interrupted`, so an explicit
    /// resume can try again), and release the paused runtime it was to
    /// continue.
    fn abandon_launch(
        &self,
        run: &Run,
        generation: &str,
        paused_runtime: Option<Arc<dyn RuntimeExecutor>>,
        fresh: bool,
    ) {
        let mut live = relock(&self.live);
        if live
            .get(&run.id)
            .is_some_and(|entry| entry.generation == generation)
        {
            live.remove(&run.id);
        }
        drop(live);
        self.forget_attachments(&run.id);
        let _ = self.transition(
            &run.id,
            if fresh {
                RunStatus::Failed
            } else {
                RunStatus::Interrupted
            },
            Some("the run's worker thread could not be started".into()),
        );
        if let Some(runtime) = paused_runtime {
            runtime.release(&run.id);
        }
    }
    pub(crate) fn emit(&self, session: &str, event: EventKind) {
        if let Some(scope) = self.event_scope(session) {
            let _ = self.events.emit(scope, event);
        }
    }
    /// The scope of a session's events: its workspace, or the chat
    /// workspace. `None` when the session cannot be read.
    pub(crate) fn event_scope(&self, session: &str) -> Option<EventScope> {
        let s = self.store.get(session).ok()?;
        let workspace = s
            .configuration
            .workspace_id
            .unwrap_or_else(|| "workspace_chat".into());
        Some(EventScope::session(
            EntityId::new(workspace).ok()?,
            EntityId::new(session).ok()?,
        ))
    }
    /// Move an active run to `status`. A run that is no longer active keeps
    /// its state and emits nothing: only [`resume`](Self::resume) moves an
    /// interrupted run on. Failing, cancelling, or interrupting a run expires
    /// its pending approvals (dropping their held plans) and answers its
    /// pending tool calls, and a failing parent cancels its active children
    /// (before and again after its own state is saved, so a child begun
    /// meanwhile is cancelled too). A run that stops being active releases
    /// the runtime it kept paused for an approval, and a child of an external
    /// runtime that completes, fails, or is cancelled has its native state
    /// forgotten ([`RuntimeExecutor::forget`]) unless something of the
    /// conversation still names it.
    pub(crate) fn transition(
        &self,
        id: &str,
        status: RunStatus,
        error: Option<String>,
    ) -> Result<Run, String> {
        self.settle(id, status, error).map(|(run, _)| run)
    }
    /// [`transition`](Self::transition), also telling whether this call saved
    /// the new state (`false`: the run was no longer active and kept its
    /// state).
    pub(crate) fn settle(
        &self,
        id: &str,
        status: RunStatus,
        error: Option<String>,
    ) -> Result<(Run, bool), String> {
        let r = self.store.run(id)?;
        if status == RunStatus::Failed && r.status.active() {
            self.cancel_children(&r.session_id, id)?;
        }
        let (result, changed, expired, finished) = self.store.update(&r.session_id, |s| {
            let mut expired = Vec::new();
            let runtime_id = s.configuration.runtime_id.clone();
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            if !r.status.active() {
                return Ok((r.clone(), false, expired, None));
            }
            r.status = status;
            r.error = error;
            r.updated_at = now();
            if matches!(
                status,
                RunStatus::Failed | RunStatus::Cancelled | RunStatus::Interrupted
            ) {
                for approval in &mut s.approvals {
                    if approval.run_id == id && approval.status == "pending" {
                        approval.status = "expired".into();
                        expired.push(approval.id.clone());
                    }
                }
                for call in r.pending_calls.drain(..) {
                    let message = aidex_providers::ChatMessage::tool_result(
                        &call.id,
                        "Tool execution did not complete successfully. Inspect the workspace \
                         before retrying; effects may be unknown after interruption.",
                    );
                    r.transcript.push(message.clone());
                    if r.parent_run_id.is_none() {
                        s.messages.push(message);
                    }
                }
            }
            let result = r.clone();
            // A finished child's native thread is never continued.
            let finished = match (&result.runtime_ref, runtime_id) {
                (Some(reference), Some(runtime))
                    if result.parent_run_id.is_some()
                        && matches!(
                            status,
                            RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled
                        )
                        && !crate::runtimes::still_named(s, id, reference) =>
                {
                    Some((runtime, reference.clone()))
                }
                _ => None,
            };
            Ok((result, true, expired, finished))
        })?;
        if !changed {
            return Ok((result, false));
        }
        // An expired approval's plan can never run.
        if !expired.is_empty() {
            relock(&self.pending).retain(|approval, _| !expired.contains(approval));
        }
        if !result.status.active() {
            self.forget_offer(id);
            if status == RunStatus::Failed {
                // A child begun while the parent was failing; an error here
                // leaves it to reconciliation.
                let _ = self.cancel_children(&r.session_id, id);
            }
            self.drop_paused(id);
            if let Some((runtime, reference)) = finished
                && let Some(runtime) = relock(&self.runtimes).get(&runtime).cloned()
            {
                runtime.forget(&reference);
            }
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
        Ok((result, true))
    }
    /// Every approval of every conversation (or of `run`), oldest first: by
    /// the creation time its id encodes ([`approval_time`]), then by
    /// conversation id, then in the order its conversation recorded them. The
    /// order is the same however the store lists conversations, so a caller
    /// paging by offset gets strictly time-ordered pages and the last page
    /// holds the newest approvals.
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
                    .enumerate()
                    .filter(|(_, a)| run.is_none_or(|r| r == a.run_id))
                    .map(|(position, a)| (s.id.clone(), position, a)),
            );
        }
        out.sort_by(|(session_a, position_a, a), (session_b, position_b, b)| {
            approval_time(&a.id)
                .cmp(approval_time(&b.id))
                .then_with(|| session_a.cmp(session_b))
                .then_with(|| position_a.cmp(position_b))
        });
        Ok(out.into_iter().map(|(_, _, approval)| approval).collect())
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
        let _guard = relock(&self.operations);
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
        // Checked again where the decision is recorded: the run may have been
        // cancelled or interrupted, and the approval expired, since the
        // checks above.
        self.store.update(&run.session_id, |s| {
            let waiting = s
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            if waiting.status != RunStatus::WaitingApproval {
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
            waiting.status = RunStatus::Running;
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
                RuntimeLaunch::Approval,
                false,
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
        // The lock is held through the launch, so no reconciliation sees the
        // running run without its thread.
        let r = self.store.run(&run.id)?;
        self.launch(&r)?;
        Ok(r)
    }
    /// Cancel `id` and its active children. While the cancel works on the
    /// run, a launch of it (an approval decision, a resume) starts with
    /// stopped tokens and settles it without running anything; after the
    /// cancellation is saved, children begun meanwhile are cancelled too and
    /// any thread still working on the run is stopped. A run already
    /// finished only has such a thread stopped.
    pub fn cancel(&self, id: &str) -> Result<Run, String> {
        let _cancelling = Cancelling::new(&self.cancelling, id);
        let r = self.store.run(id)?;
        if !r.status.active() {
            self.stop_tokens(id);
            self.drop_paused(id);
            return Ok(r);
        }
        // A runtime paused for approval has no thread left to drop its entry,
        // so it is dropped (and its runtime released) once the run is
        // cancelled, also when it paused while this cancel ran. Until then it
        // keeps the cancelled tokens: an approval or resume launched meanwhile
        // reuses them (or is refused once the run is cancelled), and a cancel
        // that cannot be saved leaves the entry to `reconcile_after_unlock`.
        self.stop_tokens(id);
        self.cancel_children(&r.session_id, id)?;
        let cancelled = self.transition(id, RunStatus::Cancelled, None)?;
        // The cancellation is saved; what follows is best effort (a vault
        // that locked meanwhile leaves the rest to reconciliation).
        let _ = self.cancel_children(&r.session_id, id);
        self.stop_tokens(id);
        self.drop_paused(id);
        Ok(cancelled)
    }
    /// Cancel every child of `parent` that is still active.
    fn cancel_children(&self, session: &str, parent: &str) -> Result<(), String> {
        let children = self
            .store
            .get(session)?
            .runs
            .into_iter()
            .filter(|r| r.parent_run_id.as_deref() == Some(parent) && r.status.active())
            .collect::<Vec<_>>();
        for child in children {
            self.cancel(&child.id)?;
        }
        Ok(())
    }
    /// Cancel the tokens of whatever thread works on `id`.
    fn stop_tokens(&self, id: &str) {
        if let Some(entry) = relock(&self.live).get(id) {
            entry.provider.cancel();
            entry.tool.cancel();
        }
    }
    /// Whether a stop of `run` is under way: its tokens were cancelled (a
    /// cancel, an emergency stop, going offline, a vault lock), or a cancel
    /// of it or of its parent works on it. Whoever stopped it records how it
    /// ended (`cancelled`, or `interrupted` by reconciliation after a lock).
    pub(crate) fn stopping(&self, run: &Run) -> bool {
        if relock(&self.live)
            .get(&run.id)
            .is_some_and(|entry| entry.provider.is_cancelled())
        {
            return true;
        }
        let cancelling = relock(&self.cancelling);
        cancelling.contains_key(&run.id)
            || run
                .parent_run_id
                .as_ref()
                .is_some_and(|parent| cancelling.contains_key(parent))
    }
    /// Drop the registry entry of `id` when no thread works on it (a runtime
    /// paused for approval), releasing the paused runtime.
    pub(crate) fn drop_paused(&self, id: &str) {
        let mut live = relock(&self.live);
        let entry = if live.get(id).is_some_and(|entry| !entry.executing) {
            live.remove(id)
        } else {
            None
        };
        drop(live);
        if let Some(runtime) = entry.and_then(|entry| entry.runtime) {
            runtime.release(id);
        }
    }
    /// Release the runtime `id` keeps paused for an approval, keeping its
    /// registry entry (and tokens) for whatever relaunches or settles it.
    fn release_paused(&self, id: &str) {
        let runtime = relock(&self.live)
            .get_mut(id)
            .filter(|entry| !entry.executing)
            .and_then(|entry| entry.runtime.take());
        if let Some(runtime) = runtime {
            runtime.release(id);
        }
    }
    /// Continue an interrupted run, or replan a run waiting for an approval
    /// (whose approval expires). Checked where the run is made pending: it
    /// must still be interrupted or waiting, and a root run cannot resume
    /// while another root run of the conversation is active, nor, for an
    /// external runtime, once a newer message started a later root run
    /// (that would continue a native thread missing the later turns). An
    /// external run resumes its own saved native thread (`Resume`, never a
    /// replay), after whatever its runtime kept paused is released.
    pub fn resume(self: &Arc<Self>, id: &str) -> Result<Run, String> {
        let _guard = relock(&self.operations);
        let r = self.store.run(id)?;
        let configuration = self.store.get(&r.session_id)?.configuration;
        let external = configuration
            .runtime_id
            .as_deref()
            .is_some_and(|id| id != "native");
        let runtime = if external {
            // Resuming continues this run's own native thread; replaying its
            // conversation instead could repeat effects.
            let runtime = self.runtime_for(&configuration)?;
            if !runtime.descriptor().supports_resume
                || !r.runtime_ref.as_ref().is_some_and(is_resumable_reference)
            {
                return Err(
                    "selected runtime cannot safely resume without a saved protocol reference"
                        .into(),
                );
            }
            Some(runtime)
        } else {
            None
        };
        if !resumable_status(r.status) {
            return Err("only an interrupted run can resume".into());
        }
        if relock(&self.live).get(id).is_some_and(|r| r.executing) {
            return Err("run is still executing".into());
        }
        let expired = self.store.update(&r.session_id, |s| {
            let position = s
                .runs
                .iter()
                .position(|run| run.id == id)
                .ok_or("run not found")?;
            let run = &s.runs[position];
            if !resumable_status(run.status) {
                return Err("only an interrupted run can resume".into());
            }
            if run.parent_run_id.is_none() {
                if s.runs
                    .iter()
                    .any(|other| other.id != id && other.parent_run_id.is_none() && other.status.active())
                {
                    return Err("session already has an active root run".into());
                }
                if external
                    && s.runs[position + 1..]
                        .iter()
                        .any(|later| later.parent_run_id.is_none())
                {
                    return Err(
                        "only the conversation's latest message can resume; send a new message \
                         to continue"
                            .into(),
                    );
                }
            }
            let mut expired = Vec::new();
            for approval in &mut s.approvals {
                if approval.run_id == id && approval.status == "pending" {
                    approval.status = "expired".into();
                    expired.push(approval.id.clone());
                }
            }
            let run = &mut s.runs[position];
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
            Ok(expired)
        })?;
        // An expired approval's plan can never run.
        relock(&self.pending).retain(|approval, _| !expired.contains(approval));
        // The lock is held through the launch, so no reconciliation sees the
        // pending run without its thread.
        let r = self.store.run(id)?;
        if let Some(runtime) = runtime {
            // What the runtime kept paused for the expired approval is not
            // continued: the resume launches from the saved reference.
            self.release_paused(id);
            runtime.release(id);
            self.launch_operation(&r, RuntimeLaunch::Resume)?;
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
        self.signal_cancel_all();
        self.release_all_paused();
        let _ = self
            .broker
            .sandbox
            .registry()
            .shutdown(aidex_sandbox::StopReason::DaemonShutdown);
    }
    /// Cancel the tokens of every run with a thread or a paused runtime,
    /// saving nothing and taking no engine-wide lock: the cheap first step of
    /// an emergency stop or of going offline, taken before the runtime gate
    /// closes, so a run whose vendor process the closing gate then kills
    /// settles as `cancelled` rather than `failed`. [`Self::cancel_all`]
    /// records the cancellations afterwards.
    pub fn signal_cancel_all(&self) {
        for r in relock(&self.live).values() {
            r.provider.cancel();
            r.tool.cancel();
        }
    }
    /// Release every runtime paused for an approval, keeping the registry
    /// entries of their runs for whatever settles them (the paused runs stay
    /// counted by [`Self::has_active`] until then).
    fn release_all_paused(&self) {
        let paused = relock(&self.live)
            .iter_mut()
            .filter(|(_, entry)| !entry.executing)
            .filter_map(|(id, entry)| Some((id.clone(), entry.runtime.take()?)))
            .collect::<Vec<_>>();
        for (id, runtime) in paused {
            runtime.release(&id);
        }
    }
    pub fn cancel_all(&self) {
        self.cancel_all_except(&HashSet::new());
    }
    /// [`Self::cancel_all`] for a vault lock the user asked for, called
    /// while the record key is still available: every active run is
    /// cancelled (and recorded so), except an external run paused for an
    /// approval. Its launch is only released (its process ended, its
    /// temporary home removed), and the run is settled as by every other
    /// end of the vault session, the idle auto-lock included: the lock's own
    /// cleanup cannot save a cancel, so [`Self::reconcile_after_unlock`]
    /// interrupts the run (its approval expires) and an explicit
    /// [`Self::resume`] continues it from its saved reference. A paused
    /// child whose parent is executing is still cancelled with its parent
    /// ([`Self::cancel`] cancels a run's active children).
    pub fn cancel_all_but_paused(&self) {
        let paused = relock(&self.live)
            .iter()
            .filter(|(_, entry)| !entry.executing && entry.runtime.is_some())
            .map(|(id, _)| id.clone())
            .collect::<HashSet<_>>();
        self.cancel_all_except(&paused);
    }
    fn cancel_all_except(&self, paused: &HashSet<String>) {
        self.forget_all_attachments();
        // No held plan may run after this, even when a cancelled run's final
        // state cannot be saved (a locked vault): `reconcile_after_unlock`
        // then interrupts its run.
        relock(&self.pending).clear();
        if let Ok(runs) = self.store.run_summaries() {
            for run in runs {
                if run.status.active() && !paused.contains(&run.id) {
                    let _ = self.cancel(&run.id);
                }
            }
        }
        self.signal_cancel_all();
        // A runtime paused for approval whose run could not be cancelled (a
        // locked vault) gives up its process and restored home now; its run
        // is interrupted by `reconcile_after_unlock`.
        self.release_all_paused();
        let _ = self
            .broker
            .sandbox
            .registry()
            .terminate_all(aidex_sandbox::StopReason::Cancelled);
        self.forget_all_attachments();
    }
    /// Whether any run has a working thread, or an external runtime paused
    /// for approval (its process may still hold credentials).
    pub fn has_active(&self) -> bool {
        !relock(&self.live).is_empty()
    }
    /// Whether any run is executing: its thread works on it (an external
    /// run's thread drives its vendor process). A run waiting for an approval
    /// does not count, native or external, even while its paused runtime
    /// process keeps it in [`Self::has_active`]. A native parent waiting for
    /// its active children still executes, so it counts until that wait ends
    /// (at most ten minutes), even while a child waits for an approval.
    pub fn has_executing(&self) -> bool {
        relock(&self.live).values().any(|run| run.executing)
    }
    /// Settle runs whose final state could not be saved while the vault was
    /// locked. Every active run with no working thread, with a cancelled one,
    /// or waiting on a native approval whose plan is gone (or was planned by
    /// a thread since cancelled) becomes `interrupted`, which expires its
    /// approvals. Nothing is continued: an
    /// interrupted run needs an explicit `resume`. A run that cannot be
    /// settled does not stop the others; the first such error is returned.
    pub fn reconcile_after_unlock(&self) -> Result<(), String> {
        let _guard = relock(&self.operations);
        let mut first_error = None;
        for summary in self.store.run_summaries()? {
            if summary.status.active()
                && let Err(error) = self.reconcile_run(&summary)
            {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
    fn reconcile_run(&self, summary: &RunSummary) -> Result<(), String> {
        let id = summary.id.as_str();
        let thread = relock(&self.live)
            .get(id)
            .map(|live| live.provider.is_stopped() || live.tool.is_cancelled());
        let approvals = self
            .store
            .get(&summary.session_id)?
            .approvals
            .into_iter()
            .filter(|a| a.run_id == id)
            .collect::<Vec<_>>();
        let stopped = match thread {
            Some(cancelled) => cancelled,
            // Waiting on a plan the engine still holds, for a run nothing
            // stopped since (a stop the locked vault kept from being saved
            // cancelled the plan's tool token).
            None => {
                let pending = relock(&self.pending);
                !approvals.iter().any(|a| {
                    a.status == "pending"
                        && a.runtime_id.is_none()
                        && pending.get(&a.id).is_some_and(|plan| !plan.stopped())
                })
            }
        };
        if !stopped {
            return Ok(());
        }
        self.transition(
            id,
            RunStatus::Interrupted,
            Some(
                "Execution stopped while private records were locked; explicit continuation is \
                 required"
                    .into(),
            ),
        )?;
        relock(&self.pending).retain(|approval, _| approvals.iter().all(|a| a.id != *approval));
        self.drop_paused(id);
        Ok(())
    }
}

/// Counts a [`NativeEngine::cancel`] of one run under way
/// ([`NativeEngine::cancelling`]) while it lives.
struct Cancelling<'a> {
    runs: &'a Mutex<HashMap<String, usize>>,
    id: String,
}
impl<'a> Cancelling<'a> {
    fn new(runs: &'a Mutex<HashMap<String, usize>>, id: &str) -> Self {
        *relock(runs).entry(id.to_owned()).or_default() += 1;
        Self {
            runs,
            id: id.to_owned(),
        }
    }
}
impl Drop for Cancelling<'_> {
    fn drop(&mut self) {
        let mut runs = relock(self.runs);
        if let Some(count) = runs.get_mut(&self.id) {
            *count -= 1;
            if *count == 0 {
                runs.remove(&self.id);
            }
        }
    }
}

/// Whether a run in `status` may [`resume`](NativeEngine::resume).
fn resumable_status(status: RunStatus) -> bool {
    matches!(status, RunStatus::Interrupted | RunStatus::WaitingApproval)
}

#[cfg(test)]
thread_local! {
    /// Make the next run thread this test thread starts fail to spawn.
    static FAIL_NEXT_SPAWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Start a run's worker thread.
fn spawn_run_thread(work: impl FnOnce() + Send + 'static) -> Result<(), String> {
    #[cfg(test)]
    if FAIL_NEXT_SPAWN.with(|fail| fail.replace(false)) {
        return Err("injected worker thread spawn failure".into());
    }
    std::thread::Builder::new()
        .name("aidex-native-run".into())
        .spawn(work)
        .map(drop)
        .map_err(|e| e.to_string())
}

/// The creation time an approval id (`approval_<ULID>`) encodes: the ten
/// timestamp characters of its ULID, whose Crockford base32 text sorts like
/// the milliseconds it encodes. An id without a ULID gives the empty string,
/// so it sorts before every timed one.
fn approval_time(id: &str) -> &str {
    const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    id.rsplit_once('_')
        .map(|(_, ulid)| ulid)
        .filter(|ulid| ulid.len() == 26 && ulid.bytes().all(|b| CROCKFORD.contains(&b)))
        .map_or("", |ulid| &ulid[..10])
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

/// Refuse (`context_too_large_for_runtime`) `messages` that exceed what the
/// runtime named `runtime` can receive with `instructions`: more than 1 MiB
/// of message text, a prompt frame over 1 MiB, or, for runtimes that take the
/// prompt as one command-line argument (Claude Code, Cursor), more than this
/// platform allows in one argument ([`argv_limit_exceeded`]).
pub(crate) fn check_runtime_bounds(
    runtime: &str,
    instructions: &str,
    messages: &[aidex_providers::ChatMessage],
) -> Result<(), String> {
    let refuse = |detail: String| Err(format!("{CONTEXT_TOO_LARGE_FOR_RUNTIME}: {detail}"));
    let text: usize = messages.iter().map(|m| m.content.len()).sum();
    if text > RUNTIME_TEXT_BYTES {
        return refuse(format!(
            "This conversation is too long for {runtime} (at most 1 MiB of messages). Continue \
             in a new conversation or with an AiDEX native model."
        ));
    }
    let prompt = runtime_prompt(instructions, messages);
    let frame = serde_json::to_string(&prompt).map_or(usize::MAX, |json| json.len());
    if frame.saturating_add(RUNTIME_FRAME_RESERVE) > RUNTIME_FRAME_BYTES {
        return refuse(format!(
            "This conversation is too long for {runtime} (its request would exceed 1 MiB). \
             Continue in a new conversation or with an AiDEX native model."
        ));
    }
    if ARGV_RUNTIMES.contains(&runtime)
        && let Some(limit) = argv_limit_exceeded(ArgvPlatform::HOST, &prompt)
    {
        return refuse(format!(
            "This conversation is too long for {runtime}, which receives it as one command-line \
             argument (at most {limit} on this system). Continue in a new conversation or with \
             an AiDEX native model."
        ));
    }
    Ok(())
}

/// Keep only what a `Resume` request carries: the messages the resumed
/// native thread has not seen, from `unseen` ([`first_unseen`]) when that is
/// an earlier user message, otherwise from the newest user message on. A
/// runtime renders more than one message as a handoff of the turns its
/// thread missed (a turn that failed, was cancelled, or was interrupted after
/// the thread's saved checkpoint), so the vendor never loses a request the
/// conversation shows; resuming the same interrupted run, it only uses the
/// newest user message as evidence of the request.
pub(crate) fn resume_messages(
    messages: &mut Vec<aidex_providers::ChatMessage>,
    unseen: Option<usize>,
) {
    let newest = messages
        .iter()
        .rposition(|m| m.role == aidex_providers::Role::User)
        .unwrap_or(0);
    let from = unseen
        .filter(|&index| index < newest && messages[index].role == aidex_providers::Role::User)
        .unwrap_or(newest);
    messages.drain(..from);
}

/// Where the conversation messages a native thread has not seen begin, for
/// a `Resume` continuing `reference` (saved by the root run its `run_id`
/// names): the user message of that run when it did not complete (its
/// checkpoint was taken before its turn was sent), otherwise of the first
/// root run after it, or, when there is none yet, the end of the
/// conversation (where a new message goes). `None` when `run` itself saved
/// `reference` (a recovery resume of that run), or when it names no known
/// root run with a message index: only the newest user message is sent.
pub(crate) fn first_unseen(
    s: &SessionDetail,
    reference: Option<&serde_json::Value>,
    run: Option<&str>,
) -> Option<usize> {
    let saved_by = reference?.get("run_id")?.as_str()?;
    if run == Some(saved_by) {
        return None;
    }
    let position = s
        .runs
        .iter()
        .position(|r| r.id == saved_by && r.parent_run_id.is_none())?;
    let saver = &s.runs[position];
    if saver.status != RunStatus::Completed {
        return saver.message_index;
    }
    match s.runs[position + 1..]
        .iter()
        .find(|r| r.parent_run_id.is_none())
    {
        Some(next) => next.message_index,
        None => Some(s.messages.len()),
    }
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
    use aidex_providers::{
        FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef, NoVault,
        OfflineMode, ProviderId,
    };
    use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};

    /// An engine with a native agent session on a fake provider answering
    /// `turns`.
    fn engine_fixture(
        turns: Vec<FakeTurn>,
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
                paths.ensure_dir(&paths.vault()).unwrap(),
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
        let workspaces = Arc::new(WorkspaceRegistry::open(&paths).unwrap());
        let workspace = workspaces.add_host(ws.path(), Some("fixture")).unwrap();
        let registry = Arc::new(
            ProviderRegistry::load(
                &paths,
                Arc::new(NoVault::default()),
                Arc::new(OfflineMode::new(false)),
            )
            .unwrap(),
        );
        let model = ModelDescriptor::new(
            ModelRef::new(
                ProviderId::new("fixture").unwrap(),
                ModelId::new("small").unwrap(),
            ),
            Locality::Local,
            65536,
        )
        .with_tool_calls(aidex_providers::ToolCallSupport::Native);
        registry
            .register_shared(Arc::new(
                FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
                    .with_model(model)
                    .with_script(turns.into_iter().collect::<FakeScript>()),
            ))
            .unwrap();
        let engine = NativeEngine::new(
            store.clone(),
            registry,
            workspaces,
            Arc::new(aidex_events::MemoryEventSink::new()),
            drive.path().join("scratch"),
        );
        let session = store
            .create(
                serde_json::from_value(serde_json::json!({"title":"fixture","workspace_id":workspace.id,"mode":"agent","provider_id":"fixture","model_id":"small","permission_mode":"supervised"}))
                    .unwrap(),
            )
            .unwrap();
        (drive, ws, engine, session.id)
    }

    fn settled(engine: &NativeEngine, id: &str) -> Run {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let run = engine.store().run(id).unwrap();
            if !matches!(run.status, RunStatus::Pending | RunStatus::Running) {
                return run;
            }
            assert!(std::time::Instant::now() < until, "run {id} never settled");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_worker_thread_that_cannot_start_leaves_nothing_executing() {
        let (_drive, _ws, engine, session) =
            engine_fixture(vec![FakeTurn::text("first"), FakeTurn::text("resumed")]);
        // A new message: its run fails.
        FAIL_NEXT_SPAWN.with(|fail| fail.set(true));
        let error = engine
            .start(&session, "hello", "spawn-fails", None)
            .unwrap_err();
        assert!(error.contains("injected"), "{error}");
        assert!(!engine.has_executing());
        assert!(!engine.has_active());
        let failed = engine.store().get(&session).unwrap().runs.remove(0);
        assert_eq!(failed.status, RunStatus::Failed);
        // Nothing holds the conversation: the next message runs.
        let next = engine
            .start(&session, "hello again", "spawn-works", None)
            .unwrap();
        assert_eq!(settled(&engine, &next.id).status, RunStatus::Completed);
        // A relaunch (a resume) returns to interrupted and can try again.
        let interrupted = engine
            .store()
            .begin(&session, "go on", "interrupted", None)
            .unwrap();
        engine
            .store()
            .update(&session, |s| {
                s.runs
                    .iter_mut()
                    .find(|r| r.id == interrupted.id)
                    .unwrap()
                    .status = RunStatus::Interrupted;
                Ok(())
            })
            .unwrap();
        FAIL_NEXT_SPAWN.with(|fail| fail.set(true));
        assert!(engine.resume(&interrupted.id).is_err());
        assert!(!engine.has_executing());
        assert_eq!(
            engine.store().run(&interrupted.id).unwrap().status,
            RunStatus::Interrupted
        );
        engine.resume(&interrupted.id).unwrap();
        assert_eq!(
            settled(&engine, &interrupted.id).status,
            RunStatus::Completed
        );
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while engine.has_active() {
            assert!(
                std::time::Instant::now() < until,
                "a run thread never stopped"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

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
    fn approval_ids_sort_by_the_time_they_encode() {
        let earlier = aidex_native_sessions::id("approval").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let later = aidex_native_sessions::id("approval").unwrap();
        assert_eq!(approval_time(&earlier).len(), 10);
        assert!(approval_time(&earlier) < approval_time(&later));
        // Only the timestamp counts, not the random part.
        assert_eq!(
            approval_time("approval_01ARZ3NDEKTSV4RRFFQ69G5FAV"),
            "01ARZ3NDEK"
        );
        // Ids without a ULID sort first.
        assert_eq!(approval_time("legacy"), "");
        assert_eq!(approval_time("approval_01arz3ndektsv4rrffq69g5fav"), "");
        assert_eq!(approval_time("approval_short"), "");
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
