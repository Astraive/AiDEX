//! Injected external execution with the same encrypted history and approvals.
use crate::{NativeEngine, engine::relock};
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
        let _guard = relock(&self.operations);
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
        *relock(&self.runtimes) = replacement;
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
        // Runtimes cannot receive files: attachments of the conversation
        // reach them as placeholder lines.
        let mut messages = run.transcript.clone();
        let runtime_name = self
            .runtime_for(&s.configuration)
            .map(|runtime| runtime.descriptor().name)
            .unwrap_or_else(|_| "this runtime".into());
        self.runtime_placeholders(&s, run, &runtime_name, &mut messages)?;
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
        self.store.update(&run.session_id, |s| {
            s.approvals
                .iter_mut()
                .find(|a| a.id == approval.id)
                .ok_or("approval not found")?
                .status = if approved { "approved" } else { "denied" }.into();
            s.runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?
                .status = RunStatus::Running;
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
        self.launch_operation(&current, RuntimeOperation::Approval)?;
        Ok(current)
    }
    pub(crate) fn execute_runtime(
        &self,
        id: &str,
        operation: RuntimeOperation,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let run = self.store.run(id)?;
        let s = self.store.get(&run.session_id)?;
        let runtime = self.runtime_for(&s.configuration)?;
        let descriptor = runtime.descriptor();
        let request = self.runtime_request(&run, operation)?;
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
        let mut tools = HashSet::new();
        let response=runtime.execute(&request,&mut|event|{
   if cancel.is_stopped()||error.is_some(){return RuntimeControl::Pause;}
   let result=(||->Result<RuntimeControl,String>{
    match event{
     RuntimeEvent::SessionStarted{..}=>Ok(RuntimeControl::Continue),
     RuntimeEvent::TextDelta{text}=>{partial.push_str(text);if partial.len()>1024*1024{return Err("runtime text exceeds output bound".into());}
     if run.parent_run_id.is_none(){self.emit(&run.session_id,EventKind::AgentDelta{text:text.clone()});}
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
      let approval=Approval{id:aidex_native_sessions::id("approval")?,run_id:id.into(),tool_name:tool_name.clone(),arguments:arguments.clone(),fingerprint,status:"pending".into(),preview:None,runtime_id:Some(descriptor.id.clone()),runtime_request_id:Some(request_id.clone()),runtime_ref:Some(runtime_ref.clone())};self.store.update(&run.session_id,|s|{s.approvals.push(approval.clone());Ok(())})?;self.emit(&run.session_id,EventKind::ApprovalCreated{approval_id:EntityId::new(&approval.id).map_err(|e|e.to_string())?,intent_id:event_id("intent",request_id)?,summary:"review external runtime tool arguments".into(),scope:aidex_events::ApprovalScope::Once,expires_at:None});paused=true;Ok(RuntimeControl::Pause)
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
            r.runtime_ref = response.runtime_ref.clone();
            if r.parent_run_id.is_none() {
                s.runtime_ref = response.runtime_ref.clone();
            }
            if response.status == RuntimeStatus::Completed {
                let message = response
                    .message
                    .clone()
                    .unwrap_or_else(|| ChatMessage::assistant(partial.clone()));
                r.transcript.push(message.clone());
                if let Some(index) = index {
                    s.messages[index] = message;
                }
            }
            Ok(())
        })?;
        if response.status == RuntimeStatus::WaitingApproval
            && let Some(live) = relock(&self.live).get_mut(id)
        {
            live.executing = false;
        }
        self.transition(
            id,
            if response.status == RuntimeStatus::WaitingApproval {
                RunStatus::WaitingApproval
            } else {
                RunStatus::Completed
            },
            None,
        )?;
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
