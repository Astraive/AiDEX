//! Injected external execution with the same encrypted history and approvals.
use crate::NativeEngine;
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
    pub fn ensure_runtime_idle(&self, id: &str) -> Result<(), String> {
        for run in self.store.run_summaries()? {
            if run.status.active() {
                let config = self.store.get(&run.session_id)?.configuration;
                if config.runtime_id.as_deref() == Some(id) {
                    return Err("cancel active external runs and approvals before modifying runtime configuration".into());
                }
            }
        }
        Ok(())
    }
    pub fn mutate_runtimes<T>(
        &self,
        mutation: impl FnOnce() -> Result<(T, Vec<Arc<dyn RuntimeExecutor>>), String>,
    ) -> Result<T, String> {
        let _guard = self
            .operations
            .lock()
            .map_err(|_| "native operations poisoned")?;
        for run in self.store.run_summaries()? {
            if run.status.active() {
                let config = self.store.get(&run.session_id)?.configuration;
                if config
                    .runtime_id
                    .as_deref()
                    .is_some_and(|id| id != "native")
                {
                    return Err("cancel active external runs and approvals before modifying runtime configuration".into());
                }
            }
        }
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
        *self
            .runtimes
            .lock()
            .map_err(|_| "runtime registry poisoned")? = replacement;
        Ok(result)
    }
    pub fn synchronize_runtimes(
        &self,
        executors: Vec<Arc<dyn RuntimeExecutor>>,
    ) -> Result<(), String> {
        self.mutate_runtimes(|| Ok(((), executors)))
    }
    pub fn register_runtime(
        &self,
        id: &str,
        runtime: Arc<dyn RuntimeExecutor>,
    ) -> Result<(), String> {
        EntityId::new(id).map_err(|e| e.to_string())?;
        let descriptor = runtime.descriptor();
        if id == "native"
            || descriptor.id != id
            || descriptor.modes.is_empty()
            || descriptor.permission_modes.is_empty()
        {
            return Err("invalid external runtime descriptor".into());
        }
        let mut registry = self
            .runtimes
            .lock()
            .map_err(|_| "runtime registry poisoned")?;
        if registry.contains_key(id) {
            return Err("runtime already registered".into());
        }
        registry.insert(id.into(), runtime);
        Ok(())
    }
    pub fn runtime_descriptors(&self) -> Vec<RuntimeDescriptor> {
        let mut items = self
            .runtimes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
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
        let runtime = self
            .runtimes
            .lock()
            .map_err(|_| "runtime registry poisoned")?
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
        // SDK model routing can include cloud providers. Treat this boundary as
        // remote unless a future verified adapter locality contract says otherwise.
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
        let mut approvals: Vec<RuntimeApprovalDecision> = s
            .approvals
            .iter()
            .filter(|a| {
                a.run_id == run.id
                    && a.runtime_id == s.configuration.runtime_id
                    && matches!(a.status.as_str(), "approved" | "denied")
            })
            .filter_map(|a| {
                Some(RuntimeApprovalDecision {
                    request_id: a.runtime_request_id.clone()?,
                    approved: a.status == "approved",
                    fingerprint: a.fingerprint.clone(),
                    runtime_ref: a.runtime_ref.clone()?,
                })
            })
            .collect();
        if operation != RuntimeOperation::Approval {
            approvals.clear();
        }
        let contextual = self.workspace_context(&s, run)?;
        let instructions = std::iter::once(s.configuration.instructions.clone())
            .chain(contextual.into_iter().map(|message| message.content))
            .collect::<Vec<_>>()
            .join("\n\n");
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
            instructions,
            model: Some(s.configuration.model_id.clone()),
            messages: run.transcript.clone(),
            max_steps: s.configuration.max_steps.saturating_sub(run.steps),
            max_tokens: s.configuration.max_tokens,
            runtime_ref: run.runtime_ref.clone(),
            approvals,
            bridge: None,
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
        self.store.update(&run.session_id, |s| {
            let active = s
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            if active.status != RunStatus::WaitingApproval {
                return Err("runtime approval no longer has a waiting run".into());
            }
            let pending = s
                .approvals
                .iter_mut()
                .find(|a| a.id == approval.id)
                .ok_or("approval not found")?;
            if pending.status != "pending"
                || pending.fingerprint != approval.fingerprint
                || pending.bridge_call_id.is_some()
            {
                return Err("runtime approval changed or expired".into());
            }
            pending.status = if approved { "approved" } else { "denied" }.into();
            active.status = RunStatus::Running;
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
        if current.status != RunStatus::Running {
            return Err("runtime approval was interrupted before continuation".into());
        }
        self.launch_operation(&current, RuntimeOperation::Approval)?;
        Ok(current)
    }
    pub(crate) fn execute_runtime(
        self: &Arc<Self>,
        id: &str,
        operation: RuntimeOperation,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        if cancel.is_stopped() {
            return Err("runtime launch was cancelled".into());
        }
        if let Some(live) = self
            .live
            .lock()
            .map_err(|_| "native run registry unavailable")?
            .get_mut(id)
        {
            live.executing = true;
        }
        let run = self.store.run(id)?;
        let s = self.store.get(&run.session_id)?;
        let runtime = self.runtime_for(&s.configuration)?;
        let descriptor = runtime.descriptor();
        let mut request = self.runtime_request(&run, operation)?;
        if request.mode == Mode::Agent && runtime.supports_mcp_bridge() {
            request.bridge = Some(self.runtime_bridge(&run, cancel)?);
        }
        if request.max_steps == 0 {
            return Err("runtime step budget exhausted".into());
        }
        if operation == RuntimeOperation::Resume
            && (!descriptor.supports_resume || request.runtime_ref.is_none())
        {
            return Err(
                "runtime cannot resume an interrupted execution without a saved protocol reference"
                    .into(),
            );
        }
        runtime.validate(&request).map_err(|e| e.to_string())?;
        if cancel.is_stopped() {
            return Err("runtime launch was cancelled".into());
        }
        self.transition(id, RunStatus::Running, None)?;
        let index = self.store.update(&run.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            r.steps += 1;
            if r.parent_run_id.is_none() {
                let index = s.messages.len();
                s.messages.push(ChatMessage::assistant(""));
                Ok(Some(index))
            } else {
                Ok(None)
            }
        })?;
        let mut partial = String::new();
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut saved = 0;
        let mut error = None;
        let mut paused = false;
        let mut approval_event = None;
        let mut started_reference: Option<Value> = None;
        let mut tools = HashSet::new();
        let response=runtime.execute(&request,&mut|event|{
   if cancel.is_stopped()||error.is_some(){return RuntimeControl::Pause;}
   let result=(||->Result<RuntimeControl,String>{
    match event{
     RuntimeEvent::SessionStarted {runtime_ref} => {
      if paused{return Err("runtime identity arrived after an approval pause".into());}safe_reference(runtime_ref)?;
      if let Some(previous)=&started_reference{if previous!=runtime_ref{return Err("runtime session-start identity changed within one launch".into());}return Ok(RuntimeControl::Continue);}
      self.store.update(&run.session_id,|session|{let active=session.runs.iter_mut().find(|active|active.id==id).ok_or("run not found")?;if !active.status.active(){return Err("runtime identity requires an active run".into());}active.runtime_ref=Some(runtime_ref.clone());if active.parent_run_id.is_none(){session.runtime_ref=Some(runtime_ref.clone());}Ok(())})?;
      started_reference=Some(runtime_ref.clone());Ok(RuntimeControl::Continue)
     },
     RuntimeEvent::TextDelta{text}=>{partial.push_str(text);if partial.len()>1024*1024{return Err("runtime text exceeds output bound".into());}
      self.emit(&run.session_id,EventKind::AgentRunDelta{agent_id:EntityId::new(id).map_err(|e|e.to_string())?,text:text.clone()});if run.parent_run_id.is_none(){self.emit(&run.session_id,EventKind::AgentDelta{text:text.clone()});}
     if last.elapsed()>=Duration::from_millis(100)||partial.len()-saved>=4096{if let Some(index)=index{self.store.update(&run.session_id,|s|{s.messages[index].content=partial.clone();Ok(())})?;}last=Instant::now();saved=partial.len();}Ok(RuntimeControl::Continue)},
     RuntimeEvent::ToolProgress{call_id,tool_name,status,summary}=>{
      if paused||request.mode!=Mode::Agent{return Err("runtime attempted tool execution outside an active agent scope".into());}
      if matches!(status,RuntimeToolStatus::Requested|RuntimeToolStatus::Started)&&tools.insert(call_id.clone()){self.store.update(&run.session_id,|s|{let r=s.runs.iter_mut().find(|r|r.id==id).ok_or("run not found")?;if r.steps>=s.configuration.max_steps{return Err("runtime step budget exhausted".into());}r.steps+=1;Ok(())})?;}
      let call_id=event_id("call",call_id)?;let kind=match status{RuntimeToolStatus::Requested=>EventKind::ToolRequested{call_id,tool_id:event_id("tool",tool_name)?,summary:"external runtime tool requested".into(),risk:RiskLevel::Medium},RuntimeToolStatus::Started=>EventKind::ToolStarted{call_id},RuntimeToolStatus::Completed=>EventKind::ToolCompleted{call_id,duration_ms:0,summary:"external runtime tool completed".into()},RuntimeToolStatus::Failed=>EventKind::ToolFailed{call_id,code:"external_tool_failed".into(),message:"external runtime tool failed; details in encrypted run history".into()}};self.emit(&run.session_id,kind);
      if matches!(status,RuntimeToolStatus::Completed|RuntimeToolStatus::Failed){let summary=aidex_events::redact_text(summary).chars().take(8192).collect::<String>();self.store.update(&run.session_id,|s|{s.runs.iter_mut().find(|r|r.id==id).ok_or("run not found")?.transcript.push(ChatMessage::assistant(format!("Runtime tool {tool_name}: {summary}")));Ok(())})?;}Ok(RuntimeControl::Continue)
     },
     RuntimeEvent::ApprovalRequest{request_id,tool_name,arguments,summary:_,runtime_ref}=>{
      if request.mode!=Mode::Agent||!descriptor.supports_approvals{return Err("runtime approval is unavailable for this mode".into());}safe_reference(runtime_ref)?;
      let fingerprint=digest(json!({"runtime":descriptor.id,"session":run.session_id,"run":run.id,"workspace":request.workspace,"permission":request.permission_mode,"request":request_id,"tool":tool_name,"arguments":arguments,"reference":runtime_ref}).to_string().as_bytes());
      let existing=self.store.get(&run.session_id)?.approvals.into_iter().rev().find(|a|a.run_id==id&&a.runtime_request_id.as_deref()==Some(request_id)&&a.runtime_id.as_deref()==Some(&descriptor.id));
      if let Some(a)=existing{if a.fingerprint!=fingerprint{return Err("runtime approval request ID was reused with changed arguments".into());}
      if matches!(a.status.as_str(),"approved"|"denied"){return Ok(RuntimeControl::Continue);}
      if a.status=="pending"{paused=true;return Ok(RuntimeControl::Pause);}}
      let approval=Approval{id:aidex_native_sessions::id("approval")?,run_id:id.into(),tool_name:tool_name.clone(),arguments:arguments.clone(),fingerprint,status:"pending".into(),preview:None,runtime_id:Some(descriptor.id.clone()),runtime_request_id:Some(request_id.clone()),runtime_ref:Some(runtime_ref.clone()),bridge_call_id:None};self.store.update(&run.session_id,|s|{if s.approvals.iter().any(|approval|approval.run_id==id&&approval.status=="pending"){return Err("another approval is already pending for this run".into());}let active=s.runs.iter_mut().find(|active|active.id==id).ok_or("run not found")?;if active.status!=RunStatus::Running{return Err("runtime approval requires a running run".into());}active.runtime_ref=Some(runtime_ref.clone());if active.parent_run_id.is_none(){s.runtime_ref=Some(runtime_ref.clone());}s.approvals.push(approval.clone());Ok(())})?;approval_event=Some(EventKind::ApprovalCreated{approval_id:EntityId::new(&approval.id).map_err(|e|e.to_string())?,intent_id:event_id("intent",request_id)?,summary:"review external runtime tool arguments".into(),scope:aidex_events::ApprovalScope::Once,expires_at:None});paused=true;Ok(RuntimeControl::Pause)
     }
    }
   })();match result{Ok(control)=>control,Err(problem)=>{error=Some(problem);RuntimeControl::Pause}}
  },cancel).map_err(|e|e.to_string())?;
        if let Some(error) = error {
            return Err(error);
        }
        if cancel.is_stopped() {
            self.transition(id, RunStatus::Cancelled, None)?;
            return Ok(());
        }
        if let Some(reference) = &response.runtime_ref {
            safe_reference(reference)?;
        }
        if response.status == RuntimeStatus::Completed {
            self.wait_bridge_calls(id, cancel)?;
        }
        if paused && response.status != RuntimeStatus::WaitingApproval {
            return Err("runtime completed while approval was pending".into());
        }
        if response.status == RuntimeStatus::WaitingApproval
            && (!paused || response.runtime_ref.is_none())
        {
            return Err("runtime paused without an approval and safe resume reference".into());
        }
        self.store.update(&run.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
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
                    .unwrap_or_else(|| ChatMessage::assistant(partial.clone()));
                r.transcript.push(message.clone());
                if let Some(index) = index {
                    // Bridge calls append evidence while the streamed assistant
                    // placeholder is open. Keep final text after those pairs.
                    s.messages.remove(index);
                    s.messages.push(message);
                }
            }
            Ok(())
        })?;
        if response.status == RuntimeStatus::Completed && self.settle_children(id, cancel)? {
            if !descriptor.supports_resume || response.runtime_ref.is_none() {
                return Err("runtime cannot safely continue with delegated child results; explicit context handoff required".into());
            }
            return self.execute_runtime(id, RuntimeOperation::Resume, cancel);
        }
        if response.status == RuntimeStatus::WaitingApproval
            && let Some(live) = self
                .live
                .lock()
                .map_err(|_| "run registry poisoned")?
                .get_mut(id)
        {
            live.executing = false;
        }
        let finished = self.transition(
            id,
            if response.status == RuntimeStatus::WaitingApproval {
                RunStatus::WaitingApproval
            } else {
                RunStatus::Completed
            },
            None,
        )?;
        if finished.status == RunStatus::WaitingApproval
            && let Some(event) = approval_event
        {
            self.emit(&run.session_id, event);
        }
        Ok(())
    }
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
