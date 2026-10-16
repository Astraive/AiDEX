//! Injected external execution with the same encrypted history and approvals.
use crate::{
    NativeEngine,
    engine::{check_runtime_bounds, relock},
};
use aidex_core::{CancelToken, EntityId};
use aidex_events::{EventKind, RiskLevel};
use aidex_native_sessions::*;
use aidex_providers::ChatMessage;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
impl NativeEngine {
    /// Refuse while a run of the external runtime `id` (`None`: of any
    /// external runtime) is active.
    fn refuse_active_external(&self, id: Option<&str>) -> Result<(), String> {
        for run in self.store.run_summaries()? {
            if run.status.active() {
                let config = self.store.get(&run.session_id)?.configuration;
                let runtime = config.runtime_id.as_deref().filter(|id| *id != "native");
                if runtime.is_some() && (id.is_none() || runtime == id) {
                    return Err("cancel active external runs and approvals before modifying runtime configuration".into());
                }
            }
        }
        Ok(())
    }
    pub fn ensure_runtime_idle(&self, id: &str) -> Result<(), String> {
        self.refuse_active_external(Some(id))
    }
    pub fn mutate_runtimes<T>(
        &self,
        mutation: impl FnOnce() -> Result<(T, Vec<Arc<dyn RuntimeExecutor>>), String>,
    ) -> Result<T, String> {
        let _guard = relock(&self.operations);
        self.refuse_active_external(None)?;
        let (result, executors) = mutation()?;
        let mut replacement = std::collections::HashMap::new();
        for executor in executors {
            let descriptor = executor.descriptor();
            EntityId::new(&descriptor.id).map_err(|e| e.to_string())?;
            if descriptor.id == "native"
                || descriptor.modes.is_empty()
                || descriptor.permission_modes.is_empty()
                || replacement.contains_key(&descriptor.id)
            {
                return Err("invalid or duplicate runtime descriptor".into());
            }
            replacement.insert(descriptor.id, executor);
        }
        *relock(&self.runtimes) = replacement;
        Ok(result)
    }
    pub fn synchronize_runtimes(
        &self,
        executors: Vec<Arc<dyn RuntimeExecutor>>,
    ) -> Result<(), String> {
        self.mutate_runtimes(|| Ok(((), executors)))
    }
    /// Replace the executor of runtime `id` alone (or add it), once none of
    /// that runtime's runs is active, while other runtimes' runs go on: for
    /// an embedder whose full [`Self::synchronize_runtimes`] was refused
    /// because another runtime is busy, and whose executor of `id` is stale
    /// (its account or credential changed).
    pub fn replace_runtime(
        &self,
        id: &str,
        runtime: Arc<dyn RuntimeExecutor>,
    ) -> Result<(), String> {
        let _guard = relock(&self.operations);
        check_descriptor(id, &runtime.descriptor())?;
        self.refuse_active_external(Some(id))?;
        relock(&self.runtimes).insert(id.into(), runtime);
        Ok(())
    }
    pub fn register_runtime(
        &self,
        id: &str,
        runtime: Arc<dyn RuntimeExecutor>,
    ) -> Result<(), String> {
        check_descriptor(id, &runtime.descriptor())?;
        let mut registry = relock(&self.runtimes);
        if registry.contains_key(id) {
            return Err("runtime already registered".into());
        }
        registry.insert(id.into(), runtime);
        Ok(())
    }
    pub fn runtime_descriptors(&self) -> Vec<RuntimeDescriptor> {
        let mut items = relock(&self.runtimes)
            .values()
            .map(|r| r.descriptor())
            .collect::<Vec<_>>();
        items.sort_by(|a, b| a.id.cmp(&b.id));
        items
    }
    pub(crate) fn runtime_for(
        &self,
        config: &SessionConfig,
    ) -> Result<Arc<dyn RuntimeExecutor>, String> {
        let id = config
            .runtime_id
            .as_deref()
            .ok_or("external runtime was not selected")?;
        let runtime = relock(&self.runtimes)
            .get(id)
            .cloned()
            .ok_or("selected external runtime is not registered")?;
        let d = runtime.descriptor();
        let permission = if config.permission_mode == PermissionMode::Auto {
            PermissionMode::Supervised
        } else {
            config.permission_mode
        };
        if !d.modes.contains(&config.mode) || !d.permission_modes.contains(&permission) {
            return Err("selected runtime does not support this mode or permission scope".into());
        }
        Ok(runtime)
    }
    pub(crate) fn runtime_request(
        &self,
        run: &Run,
        operation: RuntimeOperation,
    ) -> Result<RuntimeRequest, String> {
        let s = self.store.get(&run.session_id)?;
        // An SDK may route to a cloud model whatever the session names, so
        // every launch is evaluated as a remote model invocation.
        self.broker.authorize_native(
            &run.session_id,
            &s.configuration,
            aidex_policy::Action::ModelInvoke,
            Some(aidex_policy::ModelLocality::Remote),
        )?;
        let workspace = s
            .configuration
            .workspace_id
            .as_ref()
            .map(|id| {
                let h = self
                    .broker
                    .workspaces
                    .handle(&EntityId::new(id).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                Ok::<_, String>(RuntimeWorkspace {
                    workspace_id: id.clone(),
                    root: h.root().display_root().display().to_string(),
                })
            })
            .transpose()?;
        if let Some(reference) = &run.runtime_ref {
            safe_reference(reference)?;
        }
        // An approval continuation carries the decision just made: the run's
        // newest decided approval (one approval of a run is pending at a
        // time). Earlier launches' decisions are left out, since a vendor
        // numbers its requests per process and a resumed run's new process
        // may reuse an earlier request's id.
        let approvals: Vec<RuntimeApprovalDecision> = if operation == RuntimeOperation::Approval {
            s.approvals
                .iter()
                .rev()
                .filter(|a| {
                    a.run_id == run.id
                        && a.runtime_id == s.configuration.runtime_id
                        && matches!(a.status.as_str(), "approved" | "denied")
                })
                .take(1)
                .filter_map(|a| {
                    Some(RuntimeApprovalDecision {
                        request_id: a.runtime_request_id.clone()?,
                        approved: a.status == "approved",
                        fingerprint: a.fingerprint.clone(),
                        runtime_ref: a.runtime_ref.clone()?,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        // Runtimes cannot receive files: attachments of the conversation
        // reach them as placeholder lines.
        let mut messages = run.transcript.clone();
        let runtime_name = self
            .runtime_for(&s.configuration)
            .map(|runtime| runtime.descriptor().name)
            .unwrap_or_else(|_| "this runtime".into());
        self.runtime_placeholders(&s, run, &runtime_name, &mut messages)?;
        // Tool results earlier in the conversation carry diff statistics, not
        // applied diffs, as for native models.
        crate::tool_view::tool_results_for_model(&mut messages);
        if operation == RuntimeOperation::Resume {
            // The thread's saved reference may predate turns that did not
            // complete: their requests and partial output come too.
            let unseen = crate::engine::first_unseen(&s, run.runtime_ref.as_ref(), Some(&run.id));
            crate::engine::resume_messages(&mut messages, unseen);
        }
        Ok(RuntimeRequest {
            session_id: run.session_id.clone(),
            run_id: run.id.clone(),
            parent_run_id: run.parent_run_id.clone(),
            operation,
            mode: s.configuration.mode,
            workspace,
            permission_mode: if s.configuration.permission_mode == PermissionMode::Auto {
                PermissionMode::Supervised
            } else {
                s.configuration.permission_mode
            },
            instructions: s.configuration.instructions.clone(),
            // The default-model sentinel lets the runtime pick its own default.
            model: (s.configuration.model_id != RUNTIME_DEFAULT_MODEL)
                .then(|| s.configuration.model_id.clone()),
            messages,
            max_steps: s.configuration.max_steps.saturating_sub(run.steps),
            max_tokens: s.configuration.max_tokens,
            runtime_ref: run.runtime_ref.clone(),
            approvals,
        })
    }
    pub(crate) fn respond_runtime(
        self: &Arc<Self>,
        approval: &Approval,
        approved: bool,
    ) -> Result<Run, String> {
        let run = self.store.run(&approval.run_id)?;
        let s = self.store.get(&run.session_id)?;
        let runtime = self.runtime_for(&s.configuration)?;
        if !runtime.descriptor().supports_approvals || run.runtime_ref.is_none() {
            return Err("runtime approval continuation is unavailable".into());
        }
        // A stop under way (a cancel registered, or the run's tokens stopped)
        // ends the wait: the decision is not recorded and the run is left to
        // whoever stops it, which records it `cancelled` (or reconciliation
        // `interrupted` after a lock). Checked first, so a cancel that has
        // not stopped the tokens yet is caught too.
        if self.stopping(&run) {
            return Err(RUN_STOPPING.into());
        }
        // The paused process ended since it asked (its wall timeout, a
        // crash): the decision cannot reach it, so it is not recorded. The
        // approval expires with the run's interruption, and an explicit
        // resume continues the run from its saved reference.
        if !runtime.awaits_decision(&run.id) {
            self.transition(
                &run.id,
                RunStatus::Interrupted,
                Some(PAUSED_PROCESS_ENDED.into()),
            )?;
            return Err(PAUSED_PROCESS_ENDED.into());
        }
        // Checked again where the decision is recorded: the run may have been
        // cancelled or interrupted, and the approval expired, since `respond`
        // looked.
        self.store.update(&run.session_id, |s| {
            let waiting = s
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            if waiting.status != RunStatus::WaitingApproval {
                return Err("runtime approval no longer has a waiting run".into());
            }
            let pending = s
                .approvals
                .iter_mut()
                .find(|a| a.id == approval.id)
                .ok_or("approval not found")?;
            if pending.status != "pending" || pending.fingerprint != approval.fingerprint {
                return Err("runtime approval changed or expired".into());
            }
            pending.status = if approved { "approved" } else { "denied" }.into();
            waiting.status = RunStatus::Running;
            Ok(())
        })?;
        let approval_id = EntityId::new(&approval.id).map_err(|e| e.to_string())?;
        self.emit(
            &run.session_id,
            if approved {
                EventKind::ApprovalApproved {
                    approval_id,
                    scope: aidex_events::ApprovalScope::Once,
                }
            } else {
                EventKind::ApprovalDenied {
                    approval_id,
                    reason: None,
                }
            },
        );
        let current = self.store.run(&run.id)?;
        self.launch_operation(&current, RuntimeLaunch::Approval)?;
        Ok(current)
    }
    /// Check that the runtime accepts a new run's request before it is
    /// launched, and return how to launch it: a follow-up whose saved
    /// reference the runtime refuses at once ([`RESUME_UNAVAILABLE`]) starts
    /// over with the transcript instead.
    pub(crate) fn check_launch(
        &self,
        run: &Run,
        config: &SessionConfig,
        launch: RuntimeLaunch,
    ) -> Result<RuntimeLaunch, String> {
        let runtime = self.runtime_for(config)?;
        let request = self.runtime_request(run, launch.operation())?;
        match runtime.validate(&request) {
            Ok(()) => Ok(launch),
            Err(error) if launch == RuntimeLaunch::FollowUp && error.code == RESUME_UNAVAILABLE => {
                let request = self.restart_request(run, &request, runtime.as_ref())?;
                runtime.validate(&request).map_err(|e| e.to_string())?;
                Ok(RuntimeLaunch::Start)
            }
            Err(error) => Err(error.to_string()),
        }
    }
    /// Run one launch of an external-runtime run. Returns whether the run is
    /// now waiting for an approval with its runtime paused, which the
    /// launching thread must keep counted although it stops. Otherwise
    /// whatever the runtime kept for an approval of the run is released
    /// ([`RuntimeExecutor::release`]): nothing continues it (a continuation
    /// that failed before reaching the runtime, a cancel that the runtime
    /// answered by pausing, a refused or failed launch).
    pub(crate) fn execute_runtime(
        &self,
        id: &str,
        launch: RuntimeLaunch,
        cancel: &CancelToken,
    ) -> Result<bool, String> {
        let run = self.store.run(id)?;
        let s = self.store.get(&run.session_id)?;
        let runtime = self.runtime_for(&s.configuration)?;
        let result = self.drive_runtime(&run, &runtime, launch, cancel);
        if !matches!(result, Ok(true)) {
            runtime.release(id);
        }
        result
    }
    /// [`execute_runtime`](Self::execute_runtime) with its runtime.
    fn drive_runtime(
        &self,
        run: &Run,
        runtime: &Arc<dyn RuntimeExecutor>,
        launch: RuntimeLaunch,
        cancel: &CancelToken,
    ) -> Result<bool, String> {
        let id = run.id.as_str();
        let descriptor = runtime.descriptor();
        let resumable = descriptor.supports_resume
            && run.runtime_ref.as_ref().is_some_and(is_resumable_reference);
        // A follow-up whose reference stopped being resumable since it was
        // sent replays the conversation, as it would have from the start.
        let replayed = launch == RuntimeLaunch::FollowUp && !resumable;
        let mut launch = if replayed {
            RuntimeLaunch::Start
        } else {
            launch
        };
        if launch == RuntimeLaunch::Resume && !resumable {
            // Nothing ran: the run returns to interrupted.
            self.transition(
                id,
                RunStatus::Interrupted,
                Some(
                    "runtime cannot resume an interrupted execution without a saved protocol \
                     reference"
                        .into(),
                ),
            )?;
            return Ok(false);
        }
        let mut request = self.runtime_request(run, launch.operation())?;
        if replayed {
            check_runtime_bounds(&descriptor.name, &request.instructions, &request.messages)?;
        }
        if request.max_steps == 0 {
            return Err("runtime step budget exhausted".into());
        }
        // A refused reference, as below when the runtime starts: a follow-up
        // starts over once with the transcript, and a resumed run is never
        // replayed. A resumed run the runtime refuses only for now (offline,
        // a pending official login) also stays interrupted.
        if let Err(error) = runtime.validate(&request) {
            match launch {
                RuntimeLaunch::FollowUp if error.code == RESUME_UNAVAILABLE => {
                    request = self.restart_request(run, &request, runtime.as_ref())?;
                    runtime.validate(&request).map_err(|e| e.to_string())?;
                    launch = RuntimeLaunch::Start;
                }
                RuntimeLaunch::Resume if resume_refusal(&error.code) => {
                    self.transition(
                        id,
                        RunStatus::Interrupted,
                        Some(aidex_events::redact_text(&error.to_string()).into_owned()),
                    )?;
                    return Ok(false);
                }
                _ => return Err(error.to_string()),
            }
        }
        if cancel.is_stopped() {
            self.transition(id, RunStatus::Cancelled, None)?;
            return Ok(false);
        }
        if self.transition(id, RunStatus::Running, None)?.status != RunStatus::Running {
            // Cancelled or interrupted meanwhile.
            return Ok(false);
        }
        // `resumed`: the placeholder is the paused turn's own, which an
        // approval continuation goes on with.
        let (index, partial, resumed) = self.store.update(&run.session_id, |s| {
            let message_index = s
                .runs
                .iter()
                .find(|r| r.id == id)
                .ok_or("run not found")?
                .message_index;
            let last = s.messages.len().checked_sub(1);
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            r.steps += 1;
            if r.parent_run_id.is_some() {
                return Ok((None, String::new(), false));
            }
            // An approval continuation goes on with the turn its paused launch
            // streamed into its own placeholder (the last message: a root run
            // is alone in its conversation while it is active), so the turn
            // stays one message.
            if launch == RuntimeLaunch::Approval
                && let Some(last) = last.filter(|last| message_index.is_some_and(|m| *last > m))
                && s.messages[last].role == aidex_providers::Role::Assistant
                && s.messages[last].tool_calls.is_empty()
            {
                return Ok((Some(last), s.messages[last].content.clone(), true));
            }
            let index = s.messages.len();
            s.messages.push(ChatMessage::assistant(""));
            Ok((Some(index), String::new(), false))
        })?;
        let mut attempt = self.attempt_runtime(
            runtime.as_ref(),
            &descriptor,
            run,
            &request,
            (index, &partial),
            cancel,
        );
        match (launch, attempt.unstarted()) {
            (RuntimeLaunch::FollowUp, Some(RESUME_UNAVAILABLE)) if !cancel.is_stopped() => {
                // The saved native thread cannot be continued (another
                // account or credential, a changed or missing snapshot) and
                // nothing ran: start over with the transcript, once.
                request = self.restart_request(run, &request, runtime.as_ref())?;
                runtime.validate(&request).map_err(|e| e.to_string())?;
                attempt = self.attempt_runtime(
                    runtime.as_ref(),
                    &descriptor,
                    run,
                    &request,
                    (index, &partial),
                    cancel,
                );
            }
            (RuntimeLaunch::FollowUp, Some(PROTOCOL_ERROR | PROTOCOL_PARAMETERS)) => {
                // The runtime could not even reopen the saved native thread:
                // this run fails, and the next message starts a new thread
                // with the transcript instead of failing the same way.
                self.drop_reference(run, runtime.as_ref())?;
            }
            (RuntimeLaunch::Resume, Some(code)) if resume_refusal(code) && !cancel.is_stopped() => {
                // Replaying could repeat this run's effects: it returns to
                // interrupted (nothing ran), and an explicit resume or a new
                // message continues instead.
                let error = attempt
                    .result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                self.withdraw_turn(run, index)?;
                self.transition(
                    id,
                    RunStatus::Interrupted,
                    Some(aidex_events::redact_text(&error).into_owned()),
                )?;
                return Ok(false);
            }
            (RuntimeLaunch::Approval, Some(APPROVAL_EXPIRED)) => {
                // The paused process ended after `respond` looked (its wall
                // timeout, a crash), and the runtime refused the continuation
                // before sending the decision: nothing ran. The decision
                // lapses with the process (its approval expires instead of
                // staying approved or denied), the turn's step is given back
                // (the paused turn keeps its text), and the run returns to
                // interrupted, so an explicit resume continues it from its
                // saved reference. A run cancelled meanwhile stays cancelled.
                let error = attempt
                    .result
                    .as_ref()
                    .err()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                self.withdraw_turn(run, index.filter(|_| !resumed))?;
                self.expire_decision(run, &request)?;
                if cancel.is_stopped() {
                    self.transition(id, RunStatus::Cancelled, None)?;
                } else {
                    self.transition(
                        id,
                        RunStatus::Interrupted,
                        Some(aidex_events::redact_text(&error).into_owned()),
                    )?;
                }
                return Ok(false);
            }
            _ => {}
        }
        if let Some(problem) = attempt.problem {
            return Err(problem);
        }
        let response = match attempt.result {
            Ok(response) => response,
            // The runtime gate closed while the turn ran (going offline, a
            // vault lock, an emergency stop): those cancel external runs.
            Err(error) if error.code == OFFLINE_REFUSED && attempt.reported => {
                self.transition(
                    id,
                    RunStatus::Cancelled,
                    Some(aidex_events::redact_text(&error.to_string()).into_owned()),
                )?;
                return Ok(false);
            }
            Err(error) => return Err(error.to_string()),
        };
        if cancel.is_stopped() {
            self.transition(id, RunStatus::Cancelled, None)?;
            return Ok(false);
        }
        if let Some(reference) = &response.runtime_ref {
            safe_reference(reference)?;
        }
        if attempt.paused && response.status != RuntimeStatus::WaitingApproval {
            return Err("runtime completed while approval was pending".into());
        }
        if response.status == RuntimeStatus::WaitingApproval
            && (!attempt.paused || response.runtime_ref.is_none())
        {
            return Err("runtime paused without an approval and safe resume reference".into());
        }
        self.store.update(&run.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            // A response without a reference keeps the last saved one.
            if let Some(reference) = &response.runtime_ref {
                r.runtime_ref = Some(reference.clone());
                if r.parent_run_id.is_none() {
                    s.runtime_ref = Some(reference.clone());
                }
            }
            if response.status == RuntimeStatus::Completed {
                let message = response
                    .message
                    .clone()
                    .unwrap_or_else(|| ChatMessage::assistant(attempt.partial.clone()));
                r.transcript.push(message.clone());
                if let Some(slot) = index.and_then(|index| s.messages.get_mut(index)) {
                    *slot = message;
                }
            }
            Ok(())
        })?;
        // The paused runtime is kept with the run's registry entry, so a
        // cancel, an interruption, or a lock releases it even with no thread
        // and no readable store.
        if response.status == RuntimeStatus::WaitingApproval
            && let Some(live) = relock(&self.live).get_mut(id)
        {
            live.executing = false;
            live.runtime = Some(runtime.clone());
        }
        let settled = self.transition(
            id,
            if response.status == RuntimeStatus::WaitingApproval {
                RunStatus::WaitingApproval
            } else {
                RunStatus::Completed
            },
            None,
        )?;
        // Announced only now, so a client answering at once finds the run
        // waiting.
        let paused = settled.status == RunStatus::WaitingApproval;
        if paused && let Some(event) = attempt.approval {
            self.emit(&run.session_id, event);
        }
        Ok(paused)
    }
    /// One call of [`RuntimeExecutor::execute`] for `request`, handling the
    /// events it reports. Its text goes to the root run's placeholder
    /// `slot.0`, after the text `slot.1` it already holds (an approval
    /// continuation's paused turn).
    fn attempt_runtime(
        &self,
        runtime: &dyn RuntimeExecutor,
        descriptor: &RuntimeDescriptor,
        run: &Run,
        request: &RuntimeRequest,
        slot: (Option<usize>, &str),
        cancel: &CancelToken,
    ) -> Attempt {
        let (index, partial) = slot;
        let mut turn = Turn {
            run,
            request,
            descriptor,
            index,
            scope: self.event_scope(&run.session_id),
            agent_id: EntityId::new(&run.id).ok(),
            partial: partial.to_owned(),
            last: Instant::now() - Duration::from_secs(1),
            saved: partial.len(),
            paused: false,
            tools: HashSet::new(),
            started: None,
            approval: None,
        };
        let mut problem = None;
        let mut reported = false;
        let result = runtime.execute(
            request,
            &mut |event| {
                reported = true;
                if cancel.is_stopped() || problem.is_some() {
                    return RuntimeControl::Pause;
                }
                match self.runtime_event(&mut turn, event) {
                    Ok(control) => control,
                    Err(error) => {
                        problem = Some(error);
                        RuntimeControl::Pause
                    }
                }
            },
            cancel,
        );
        Attempt {
            result,
            problem,
            reported,
            paused: turn.paused,
            partial: turn.partial,
            approval: turn.approval,
        }
    }
    fn runtime_event(
        &self,
        turn: &mut Turn,
        event: &RuntimeEvent,
    ) -> Result<RuntimeControl, String> {
        let id = turn.run.id.as_str();
        let session = turn.run.session_id.as_str();
        match event {
            RuntimeEvent::SessionStarted { runtime_ref } => {
                if turn.paused {
                    return Err("runtime identity arrived after an approval pause".into());
                }
                safe_reference(runtime_ref)?;
                if let Some(started) = &turn.started {
                    if started != runtime_ref {
                        return Err(
                            "runtime session-start identity changed within one launch".into()
                        );
                    }
                    return Ok(RuntimeControl::Continue);
                }
                // Saved before the runtime goes on, so a crash mid-turn still
                // leaves the identity to resume; a child keeps its own.
                self.store.update(session, |s| {
                    let r = s
                        .runs
                        .iter_mut()
                        .find(|r| r.id == id)
                        .ok_or("run not found")?;
                    if !r.status.active() {
                        return Err("runtime identity requires an active run".into());
                    }
                    r.runtime_ref = Some(runtime_ref.clone());
                    if r.parent_run_id.is_none() {
                        s.runtime_ref = Some(runtime_ref.clone());
                    }
                    Ok(())
                })?;
                turn.started = Some(runtime_ref.clone());
                Ok(RuntimeControl::Continue)
            }
            RuntimeEvent::TextDelta { text } => {
                turn.partial.push_str(text);
                if turn.partial.len() > RUNTIME_OUTPUT_BYTES {
                    return Err("runtime text exceeds output bound".into());
                }
                if let Some(scope) = &turn.scope {
                    if let Some(agent_id) = &turn.agent_id {
                        let _ = self.events.emit(
                            scope.clone(),
                            EventKind::AgentRunDelta {
                                agent_id: agent_id.clone(),
                                text: text.clone(),
                            },
                        );
                    }
                    if turn.run.parent_run_id.is_none() {
                        let _ = self
                            .events
                            .emit(scope.clone(), EventKind::AgentDelta { text: text.clone() });
                    }
                }
                if turn.last.elapsed() >= Duration::from_millis(100)
                    || turn.partial.len() - turn.saved >= 4096
                {
                    if let Some(index) = turn.index {
                        let partial = &turn.partial;
                        self.store.update(session, |s| {
                            if let Some(message) = s.messages.get_mut(index) {
                                message.content = partial.clone();
                            }
                            Ok(())
                        })?;
                    }
                    turn.last = Instant::now();
                    turn.saved = turn.partial.len();
                }
                Ok(RuntimeControl::Continue)
            }
            RuntimeEvent::ToolProgress {
                call_id,
                tool_name,
                status,
                summary,
            } => {
                if turn.paused || turn.request.mode != Mode::Agent {
                    return Err(
                        "runtime attempted tool execution outside an active agent scope".into(),
                    );
                }
                if matches!(
                    status,
                    RuntimeToolStatus::Requested | RuntimeToolStatus::Started
                ) && turn.tools.insert(call_id.clone())
                {
                    self.store.update(session, |s| {
                        let r = s
                            .runs
                            .iter_mut()
                            .find(|r| r.id == id)
                            .ok_or("run not found")?;
                        if r.steps >= s.configuration.max_steps {
                            return Err("runtime step budget exhausted".into());
                        }
                        r.steps += 1;
                        Ok(())
                    })?;
                }
                let call_id = event_id("call", call_id)?;
                let kind = match status {
                    RuntimeToolStatus::Requested => EventKind::ToolRequested {
                        call_id,
                        tool_id: event_id("tool", tool_name)?,
                        summary: "external runtime tool requested".into(),
                        risk: RiskLevel::Medium,
                    },
                    RuntimeToolStatus::Started => EventKind::ToolStarted { call_id },
                    RuntimeToolStatus::Completed => EventKind::ToolCompleted {
                        call_id,
                        duration_ms: 0,
                        summary: "external runtime tool completed".into(),
                    },
                    RuntimeToolStatus::Failed => EventKind::ToolFailed {
                        call_id,
                        code: "external_tool_failed".into(),
                        message: "external runtime tool failed; details in encrypted run history"
                            .into(),
                    },
                };
                self.emit(session, kind);
                if matches!(
                    status,
                    RuntimeToolStatus::Completed | RuntimeToolStatus::Failed
                ) {
                    let summary = aidex_events::redact_text(summary)
                        .chars()
                        .take(8192)
                        .collect::<String>();
                    self.store.update(session, |s| {
                        s.runs
                            .iter_mut()
                            .find(|r| r.id == id)
                            .ok_or("run not found")?
                            .transcript
                            .push(ChatMessage::assistant(format!(
                                "Runtime tool {tool_name}: {summary}"
                            )));
                        Ok(())
                    })?;
                }
                Ok(RuntimeControl::Continue)
            }
            RuntimeEvent::ApprovalRequest {
                request_id,
                tool_name,
                arguments,
                summary: _,
                runtime_ref,
            } => {
                if turn.request.mode != Mode::Agent || !turn.descriptor.supports_approvals {
                    return Err("runtime approval is unavailable for this mode".into());
                }
                safe_reference(runtime_ref)?;
                let request = turn.request;
                let fingerprint = digest(
                    json!({
                        "runtime": turn.descriptor.id,
                        "session": session,
                        "run": id,
                        "workspace": request.workspace,
                        "permission": request.permission_mode,
                        "request": request_id,
                        "tool": tool_name,
                        "arguments": arguments,
                        "reference": runtime_ref,
                    })
                    .to_string()
                    .as_bytes(),
                );
                // A request id is matched only among the approvals this launch
                // of the runtime asked for (its approval continuations
                // included): a vendor numbers requests per process, so a
                // resumed run's new process reuses an earlier one's ids.
                let launched = relock(&self.live)
                    .get(id)
                    .map(|live| live.approvals.clone())
                    .unwrap_or_default();
                let existing = self
                    .store
                    .get(session)?
                    .approvals
                    .into_iter()
                    .rev()
                    .find(|a| {
                        a.run_id == id
                            && launched.contains(&a.id)
                            && a.runtime_request_id.as_deref() == Some(request_id)
                            && a.runtime_id.as_deref() == Some(&turn.descriptor.id)
                    });
                if let Some(a) = existing {
                    if a.fingerprint != fingerprint {
                        return Err(
                            "runtime approval request ID was reused with changed arguments".into(),
                        );
                    }
                    if matches!(a.status.as_str(), "approved" | "denied") {
                        return Ok(RuntimeControl::Continue);
                    }
                    if a.status == "pending" {
                        turn.paused = true;
                        return Ok(RuntimeControl::Pause);
                    }
                }
                let approval = Approval {
                    id: aidex_native_sessions::id("approval")?,
                    run_id: id.into(),
                    tool_name: tool_name.clone(),
                    arguments: arguments.clone(),
                    fingerprint,
                    status: "pending".into(),
                    preview: None,
                    runtime_id: Some(turn.descriptor.id.clone()),
                    runtime_request_id: Some(request_id.clone()),
                    runtime_ref: Some(runtime_ref.clone()),
                };
                // The approval and the reference to continue from are saved
                // together, before the runtime pauses.
                self.store.update(session, |s| {
                    if s.approvals
                        .iter()
                        .any(|a| a.run_id == id && a.status == "pending")
                    {
                        return Err("another approval is already pending for this run".into());
                    }
                    let r = s
                        .runs
                        .iter_mut()
                        .find(|r| r.id == id)
                        .ok_or("run not found")?;
                    if r.status != RunStatus::Running {
                        return Err("runtime approval requires a running run".into());
                    }
                    r.runtime_ref = Some(runtime_ref.clone());
                    if r.parent_run_id.is_none() {
                        s.runtime_ref = Some(runtime_ref.clone());
                    }
                    s.approvals.push(approval.clone());
                    Ok(())
                })?;
                if let Some(live) = relock(&self.live).get_mut(id) {
                    live.approvals.push(approval.id.clone());
                }
                turn.approval = Some(EventKind::ApprovalCreated {
                    approval_id: EntityId::new(&approval.id).map_err(|e| e.to_string())?,
                    intent_id: event_id("intent", request_id)?,
                    summary: "review external runtime tool arguments".into(),
                    scope: aidex_events::ApprovalScope::Once,
                    expires_at: None,
                });
                turn.paused = true;
                Ok(RuntimeControl::Pause)
            }
        }
    }
    /// Forget the run's and the conversation's saved reference, which the
    /// runtime refused ([`Self::drop_reference`]), and turn `refused` into a
    /// `Start` request with the whole transcript, bounded as a new start.
    fn restart_request(
        &self,
        run: &Run,
        refused: &RuntimeRequest,
        runtime: &dyn RuntimeExecutor,
    ) -> Result<RuntimeRequest, String> {
        let run = self.drop_reference(run, runtime)?;
        let mut request = self.runtime_request(&run, RuntimeOperation::Start)?;
        // The refused attempt's step is this one's: the budget is unchanged.
        request.max_steps = refused.max_steps;
        check_runtime_bounds(
            &runtime.descriptor().name,
            &request.instructions,
            &request.messages,
        )?;
        Ok(request)
    }
    /// Clear the run's saved reference, and the conversation's for a root
    /// run, so nothing continues that native thread; the runtime then
    /// forgets the state the run's reference names
    /// ([`RuntimeExecutor::forget`]) unless something of the conversation
    /// that may still continue a thread names it ([`still_named`]). Returns
    /// the run as saved.
    fn drop_reference(&self, run: &Run, runtime: &dyn RuntimeExecutor) -> Result<Run, String> {
        let (run, unnamed) = self.store.update(&run.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            let dropped = r.runtime_ref.take();
            let r = r.clone();
            if r.parent_run_id.is_none() {
                s.runtime_ref = None;
            }
            let unnamed = dropped.filter(|reference| !still_named(s, &r.id, reference));
            Ok((r, unnamed))
        })?;
        if let Some(reference) = unnamed {
            runtime.forget(&reference);
        }
        Ok(run)
    }
    /// Expire the decision an approval continuation carried, which the
    /// runtime refused before sending it (`approval_expired`): the run's
    /// approval it names, recorded `approved` or `denied` by `respond`, did
    /// not take effect. `approval.expired` follows the decision's own event.
    fn expire_decision(&self, run: &Run, request: &RuntimeRequest) -> Result<(), String> {
        let Some(decision) = request.approvals.first() else {
            return Ok(());
        };
        let expired = self.store.update(&run.session_id, |s| {
            let runtime_id = s.configuration.runtime_id.clone();
            Ok(s.approvals
                .iter_mut()
                .rev()
                .find(|a| {
                    a.run_id == run.id
                        && a.runtime_id == runtime_id
                        && matches!(a.status.as_str(), "approved" | "denied")
                })
                .filter(|a| {
                    a.fingerprint == decision.fingerprint
                        && a.runtime_request_id.as_deref() == Some(decision.request_id.as_str())
                })
                .map(|a| {
                    a.status = "expired".into();
                    a.id.clone()
                }))
        })?;
        if let Some(id) = expired {
            self.emit(
                &run.session_id,
                EventKind::ApprovalExpired {
                    approval_id: EntityId::new(&id).map_err(|e| e.to_string())?,
                },
            );
        }
        Ok(())
    }
    /// Undo a turn the runtime refused before starting: give back its step
    /// and drop its still-empty assistant placeholder.
    fn withdraw_turn(&self, run: &Run, index: Option<usize>) -> Result<(), String> {
        self.store.update(&run.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            r.steps = r.steps.saturating_sub(1);
            if let Some(index) = index
                && index + 1 == s.messages.len()
                && s.messages[index].content.is_empty()
                && s.messages[index].tool_calls.is_empty()
            {
                s.messages.pop();
            }
            Ok(())
        })
    }
}

/// Refuse a runtime that cannot be registered as `id`: not an entity id,
/// `native`, or a descriptor with another id or without modes.
fn check_descriptor(id: &str, descriptor: &RuntimeDescriptor) -> Result<(), String> {
    EntityId::new(id).map_err(|e| e.to_string())?;
    if id == "native"
        || descriptor.id != id
        || descriptor.modes.is_empty()
        || descriptor.permission_modes.is_empty()
    {
        return Err("invalid external runtime descriptor".into());
    }
    Ok(())
}

/// `RuntimeError.code` of a runtime that refused a saved reference before it
/// started anything: adapters return it before spawning a process.
pub(crate) const RESUME_UNAVAILABLE: &str = "resume_unavailable";
/// `RuntimeError.code` of a runtime the runtime gate keeps from running
/// (offline, a locked vault, an emergency stop).
const OFFLINE_REFUSED: &str = "offline_refused";
/// `RuntimeError.code` of an approval continuation whose paused process is
/// gone: adapters return it before the decision is sent, so nothing ran.
const APPROVAL_EXPIRED: &str = "approval_expired";
/// The error of a run whose paused process ended before its approval was
/// answered ([`RuntimeExecutor::awaits_decision`]), and of `respond` then.
/// Its words map to `conflict` on the wire.
const PAUSED_PROCESS_ENDED: &str = "approval_expired: the paused runtime process ended before the \
                                    decision reached it; the run was interrupted, resume it \
                                    explicitly";
/// The error of `respond` for a run whose stop is under way
/// ([`NativeEngine::stopping`]): the decision is not recorded, and the run
/// ends as its stop settles it. Its words map to `conflict` on the wire.
const RUN_STOPPING: &str =
    "approval_expired: the run is being stopped; the decision was not recorded";
/// `RuntimeError.code`s of a native protocol request the runtime rejected.
const PROTOCOL_ERROR: &str = "protocol_error";
const PROTOCOL_PARAMETERS: &str = "protocol_parameters";

/// Whether a `Resume` the runtime refused with `code` before it started
/// anything leaves the run `interrupted` rather than `failed`: the saved
/// reference cannot be continued ([`RESUME_UNAVAILABLE`]), or the runtime is
/// unavailable only for now (the runtime gate is closed, an official login is
/// pending), so an explicit resume can try again later.
fn resume_refusal(code: &str) -> bool {
    matches!(code, RESUME_UNAVAILABLE | OFFLINE_REFUSED | "auth_pending")
}

/// Whether something of conversation `s` that may still continue a native
/// thread names the snapshot `reference` names (`snapshot_id`): the
/// conversation's reference, a run other than `except` that is active or
/// interrupted, or a pending approval. Finished runs' references are history
/// that nothing resumes. A reference that names no snapshot counts as named:
/// there is nothing to forget.
pub(crate) fn still_named(s: &SessionDetail, except: &str, reference: &Value) -> bool {
    fn snapshot(reference: &Value) -> Option<&str> {
        reference.get("snapshot_id")?.as_str()
    }
    let Some(name) = snapshot(reference) else {
        return true;
    };
    let names = |other: &Option<Value>| other.as_ref().and_then(snapshot) == Some(name);
    names(&s.runtime_ref)
        || s.runs.iter().any(|r| {
            r.id != except
                && (r.status.active() || r.status == RunStatus::Interrupted)
                && names(&r.runtime_ref)
        })
        || s.approvals
            .iter()
            .any(|a| a.status == "pending" && names(&a.runtime_ref))
}
/// Most text one external turn may stream (1 MiB).
const RUNTIME_OUTPUT_BYTES: usize = 1024 * 1024;

/// What an external run's thread asks of its runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLaunch {
    /// A new run: `Start` with the conversation transcript (a child gets
    /// only its prompt).
    Start,
    /// A new root message continuing the conversation's saved native thread
    /// with `Resume`. When the runtime refuses the reference before starting
    /// anything ([`RESUME_UNAVAILABLE`]), the run starts over once with
    /// `Start`.
    FollowUp,
    /// [`NativeEngine::resume`] of an interrupted run: `Resume`, and never a
    /// replay of the conversation.
    Resume,
    /// Continuing after an approval decision.
    Approval,
}
impl RuntimeLaunch {
    pub(crate) fn operation(self) -> RuntimeOperation {
        match self {
            Self::Start => RuntimeOperation::Start,
            Self::FollowUp | Self::Resume => RuntimeOperation::Resume,
            Self::Approval => RuntimeOperation::Approval,
        }
    }
}

/// One call of [`RuntimeExecutor::execute`] and what its events left.
struct Attempt {
    result: Result<RuntimeResponse, RuntimeError>,
    /// The first problem found in the runtime's events; the runtime was told
    /// to pause.
    problem: Option<String>,
    /// Whether the runtime reported any event.
    reported: bool,
    paused: bool,
    partial: String,
    /// `approval.created` of the approval the runtime paused for, emitted
    /// once the run waits.
    approval: Option<EventKind>,
}
impl Attempt {
    /// The code of the runtime's error when it failed before reporting
    /// anything (a refusal before it started, such as [`RESUME_UNAVAILABLE`]).
    fn unstarted(&self) -> Option<&str> {
        match &self.result {
            Err(error) if !self.reported && self.problem.is_none() => Some(error.code.as_str()),
            _ => None,
        }
    }
}

/// The state of one attempt while its events arrive.
struct Turn<'a> {
    run: &'a Run,
    request: &'a RuntimeRequest,
    descriptor: &'a RuntimeDescriptor,
    /// The root run's assistant placeholder in the conversation.
    index: Option<usize>,
    scope: Option<aidex_events::EventScope>,
    agent_id: Option<EntityId>,
    partial: String,
    last: Instant,
    saved: usize,
    paused: bool,
    tools: HashSet<String>,
    /// The identity `SessionStarted` reported in this attempt.
    started: Option<Value>,
    approval: Option<EventKind>,
}

fn event_id(prefix: &str, value: &str) -> Result<EntityId, String> {
    EntityId::new(format!("{prefix}_{}", &digest(value.as_bytes())[..32]))
        .map_err(|e| e.to_string())
}
fn safe_reference(value: &Value) -> Result<(), String> {
    let text = value.to_string();
    if text.len() > 64 * 1024 || aidex_events::redact_text(&text) != text {
        return Err("unsafe runtime resume metadata".into());
    }
    fn visit(v: &Value) -> bool {
        match v {
            Value::Object(object) => object.iter().any(|(k, v)| {
                matches!(
                    k.to_ascii_lowercase().as_str(),
                    "access_token"
                        | "refresh_token"
                        | "id_token"
                        | "authorization"
                        | "api_key"
                        | "password"
                        | "secret"
                        | "secret_key"
                        | "client_secret"
                        | "private_key"
                ) || visit(v)
            }),
            Value::Array(items) => items.iter().any(visit),
            _ => false,
        }
    }
    if visit(value) {
        return Err("authentication material cannot be runtime resume metadata".into());
    }
    Ok(())
}
