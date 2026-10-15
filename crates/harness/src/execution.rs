//! Provider turns and scoped tool exchange; no blind retry of side effects.
use crate::{
    NativeEngine,
    broker::PrepareError,
    engine::{PendingPlan, relock},
    inference::Turn,
    native_tools::{AGENT_RESULT, AGENT_SPAWN},
};
use aidex_core::{CancelToken, EntityId, WorkspacePath};
use aidex_events::{EventKind, Outcome};
use aidex_native_sessions::*;
use aidex_providers::{ChatMessage, ChatRequest, ModelId, ToolCallSupport, ToolChoice, ToolSpec};
use aidex_tools::PermitIssuer;
use std::sync::Arc;

/// What an approval shows the user and binds to.
pub(crate) struct ApprovalRequest {
    pub pending: PendingPlan,
    pub arguments: serde_json::Value,
    pub fingerprint: String,
    pub preview: Option<serde_json::Value>,
    pub intent_id: EntityId,
}

impl NativeEngine {
    pub(crate) fn append_result(&self, id: &str, call: &str, result: String) -> Result<(), String> {
        self.append_output(id, call, result, None)
    }
    /// [`append_result`](Self::append_result) of a result stored as its model
    /// view, with the complete output it was made from (`full`), which goes
    /// to the run's [`Run::tool_outputs`].
    fn append_output(
        &self,
        id: &str,
        call: &str,
        result: String,
        full: Option<String>,
    ) -> Result<(), String> {
        let run = self.store.run(id)?;
        self.store.update(&run.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            if r.status == RunStatus::Cancelled {
                return Err("run cancelled".into());
            }
            if let Some(output) = full {
                r.tool_outputs.push(ToolOutputRecord {
                    call_id: call.into(),
                    output,
                });
            }
            let message = ChatMessage::tool_result(call, result);
            r.transcript.push(message.clone());
            if !r.pending_calls.is_empty() {
                r.pending_calls.remove(0);
            }
            if r.parent_run_id.is_none() {
                s.messages.push(message);
            }
            Ok(())
        })
    }
    /// Park the run until the user decides on `request`.
    ///
    /// The run keeps its live entry until its thread ends, marked no longer
    /// executing before the pause is saved: reconciliation never finds the
    /// running run without an entry, and an approval answered as soon as it
    /// is saved relaunches the run (taking the entry over) instead of finding
    /// it still executing. The plan is held before the approval can be read,
    /// and the pause is saved only for a run that is still running and not
    /// stopped, so a run cancelled or interrupted meanwhile keeps that state
    /// (or, when its stop could not be saved, settles as stopped) and its
    /// plan is dropped.
    fn request_approval(
        &self,
        id: &str,
        run: &Run,
        call: &aidex_providers::ToolCall,
        request: ApprovalRequest,
    ) -> Result<(), String> {
        let approval = Approval {
            id: aidex_native_sessions::id("approval")?,
            run_id: id.into(),
            tool_name: call.name.clone(),
            arguments: request.arguments,
            fingerprint: request.fingerprint,
            status: "pending".into(),
            runtime_id: None,
            runtime_request_id: None,
            runtime_ref: None,
            preview: request.preview,
        };
        relock(&self.pending).insert(approval.id.clone(), request.pending);
        let tokens = relock(&self.live).get_mut(id).map(|live| {
            live.executing = false;
            (live.provider.clone(), live.tool.clone())
        });
        let saved = self.store.update(&run.session_id, |s| {
            let r = s
                .runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?;
            // A stop whose state could not be saved (the vault locked and
            // was unlocked again) leaves the run running with its tokens
            // cancelled; it is not paused either.
            let stopped = tokens
                .as_ref()
                .is_some_and(|(provider, tool)| provider.is_stopped() || tool.is_cancelled());
            if r.status != RunStatus::Running || stopped {
                return Err("run stopped before its approval was requested".into());
            }
            r.status = RunStatus::WaitingApproval;
            s.approvals.push(approval.clone());
            Ok(())
        });
        if let Err(error) = saved {
            relock(&self.pending).remove(&approval.id);
            return Err(error);
        }
        self.emit(
            &run.session_id,
            EventKind::ApprovalCreated {
                approval_id: EntityId::new(&approval.id).map_err(|e| e.to_string())?,
                intent_id: request.intent_id,
                summary: "review native tool arguments".into(),
                scope: aidex_events::ApprovalScope::Once,
                expires_at: None,
            },
        );
        Ok(())
    }
    /// Handle the first pending call of `run`. `Ok(true)` when the run now
    /// waits for an approval.
    fn handle_call(
        self: &Arc<Self>,
        id: &str,
        run: &Run,
        s: &SessionDetail,
        call: &aidex_providers::ToolCall,
        tool_cancel: &aidex_tools::CancelToken,
    ) -> Result<bool, String> {
        if call.name == AGENT_SPAWN || call.name == AGENT_RESULT {
            if s.configuration.mode != Mode::Agent {
                return Err("tools are unavailable in chat and plan modes".into());
            }
            let result = if call.name == AGENT_SPAWN {
                let input: crate::children::SpawnArgs =
                    serde_json::from_str(&call.arguments).map_err(|_| "invalid child arguments")?;
                let child = self.start(
                    &run.session_id,
                    &input.prompt,
                    &format!("child:{}:{}", id, call.id),
                    Some(id.into()),
                )?;
                serde_json::json!({"child_run_id":child.id,"status":child.status}).to_string()
            } else {
                let input: crate::children::ResultArgs = serde_json::from_str(&call.arguments)
                    .map_err(|_| "invalid child result arguments")?;
                serde_json::to_string(&self.child_result(id, &input.run_id)?)
                    .map_err(|e| e.to_string())?
            };
            self.append_result(id, &call.id, result)?;
            return Ok(false);
        }
        if let Some(resolved) = self.resolve_tool(s, run, &call.name, tool_cancel) {
            let plan = match resolved.and_then(|tool| self.plan_extension(s, run, call, tool)) {
                Ok(plan) => plan,
                Err(refusal) => {
                    self.emit(
                        &run.session_id,
                        EventKind::ToolDenied {
                            call_id: EntityId::generate("call").map_err(|e| e.to_string())?,
                            reason: "native tool refused; details in encrypted run history".into(),
                        },
                    );
                    self.append_result(id, &call.id, format!("Tool refused: {refusal}"))?;
                    return Ok(false);
                }
            };
            self.emit(
                &run.session_id,
                EventKind::ToolRequested {
                    call_id: plan.call_id.clone(),
                    tool_id: EntityId::new(plan.tool.tool_id()).map_err(|e| e.to_string())?,
                    summary: "native tool arguments stored in encrypted history".into(),
                    risk: plan.tool.risk(),
                },
            );
            if plan.needs_approval {
                let request = ApprovalRequest {
                    arguments: plan.input.clone(),
                    fingerprint: plan.fingerprint.clone(),
                    preview: plan.preview.clone(),
                    intent_id: plan.call_id.clone(),
                    pending: PendingPlan::Extension(Box::new(plan)),
                };
                self.request_approval(id, run, call, request)?;
                return Ok(true);
            }
            self.execute_extension(id, call, &plan, tool_cancel)?;
            return Ok(false);
        }
        if s.configuration.mode != Mode::Agent {
            return Err("tools are unavailable in chat and plan modes".into());
        }
        let mut prepared = match self.broker.prepare(s, call) {
            Ok(prepared) => prepared,
            // Nothing started: the model reads why and can correct the call.
            Err(PrepareError::Repairable { code, message }) => {
                let result = serde_json::json!({
                    "is_error": true,
                    "code": code,
                    "message": aidex_events::redact_text(&message),
                    "effect": "not_started",
                });
                self.append_result(id, &call.id, result.to_string())?;
                return Ok(false);
            }
            Err(PrepareError::Fatal(error)) => return Err(error),
        };
        prepared.context.cancel = tool_cancel.clone();
        let call_id = prepared.plan.call_id().clone();
        self.emit(
            &run.session_id,
            EventKind::ToolRequested {
                call_id: call_id.clone(),
                tool_id: prepared.plan.tool_id().entity_id().clone(),
                summary: "native tool arguments stored in encrypted history".into(),
                risk: prepared.plan.risk(),
            },
        );
        if prepared.needs_approval {
            let request = ApprovalRequest {
                arguments: prepared.plan.input().clone(),
                fingerprint: prepared.plan.fingerprint().to_string(),
                preview: prepared
                    .plan
                    .preview()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|e| e.to_string())?,
                intent_id: call_id,
                pending: PendingPlan::Tool(Box::new(prepared)),
            };
            self.request_approval(id, run, call, request)?;
            return Ok(true);
        }
        self.execute(
            id,
            call,
            &prepared,
            PermitIssuer::Policy {
                rule: "native evaluated policy".into(),
            },
        )?;
        Ok(false)
    }
    pub(crate) fn pump(
        self: &Arc<Self>,
        id: &str,
        cancel: &CancelToken,
        tool_cancel: &aidex_tools::CancelToken,
    ) -> Result<(), String> {
        // A run cancelled, interrupted, or failed before this thread got to
        // it (a launch that raced a cancel) is left as it is.
        if self.transition(id, RunStatus::Running, None)?.status != RunStatus::Running {
            return Ok(());
        }
        loop {
            if cancel.is_cancelled() {
                self.transition(id, RunStatus::Cancelled, None)?;
                return Ok(());
            }
            let run = self.store.run(id)?;
            // Stopped since the last step without this thread's token (a
            // settle that could not reach it): nothing more runs.
            if run.status != RunStatus::Running {
                return Ok(());
            }
            let s = self.store.get(&run.session_id)?;
            if run.steps >= s.configuration.max_steps {
                return Err("native run step budget exhausted".into());
            }
            if let Some(call) = run.pending_calls.first() {
                self.store.update(&run.session_id, |s| {
                    s.runs
                        .iter_mut()
                        .find(|r| r.id == id)
                        .ok_or("run not found")?
                        .steps += 1;
                    Ok(())
                })?;
                if self.handle_call(id, &run, &s, call, tool_cancel)? {
                    return Ok(());
                }
                continue;
            }
            if run.steps >= s.configuration.max_steps {
                return Err("native run step budget exhausted".into());
            }
            let primary = self.primary_model(&s.configuration)?;
            let native_tools = primary.1.capabilities.tool_calls == ToolCallSupport::Native;
            let offer = self.offer(&s, &run, native_tools, tool_cancel);
            self.remember_offer(id, &offer);
            let messages = self.assemble(
                &s,
                &run,
                &offer,
                crate::context::instruction_budget(&primary.1),
            )?;
            let start = messages.len().saturating_sub(run.transcript.len());
            let attachments = self.turn_attachments(&s, &run)?;
            let mut request = ChatRequest::new(
                ModelId::new(&s.configuration.model_id).map_err(|e| e.to_string())?,
                messages,
            )
            .with_max_tokens(s.configuration.max_tokens);
            if offer.builtin {
                request.tools = self
                    .broker
                    .tools
                    .specs()
                    .into_iter()
                    .map(|spec| ToolSpec {
                        name: spec.name,
                        description: spec.description,
                        parameters: spec.parameters,
                    })
                    .collect();
                request.tools.push(ToolSpec{name:AGENT_SPAWN.into(),description:"Start a bounded child sharing this workspace and permission mode. Its status never completes the parent.".into(),parameters:serde_json::json!({"type":"object","properties":{"prompt":{"type":"string"}},"required":["prompt"],"additionalProperties":false})});
                request.tools.push(ToolSpec{name:AGENT_RESULT.into(),description:"Read the persisted status and final result of an immediate child run belonging to this parent. Poll again in a later turn if it is still active.".into(),parameters:serde_json::json!({"type":"object","properties":{"run_id":{"type":"string"}},"required":["run_id"],"additionalProperties":false})});
            }
            request.tools.extend(offer.specs());
            if request.tools.is_empty() {
                request.tool_choice = ToolChoice::None;
            }
            let message_index = self.store.update(&run.session_id, |s| {
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
            let turn = Turn {
                start,
                nonce: self.run_nonce(&run),
                index: message_index,
                attachments,
                streamed: Default::default(),
            };
            let response = self.infer(&request, &run, &s.configuration, cancel, primary, &turn)?;
            if cancel.is_cancelled() {
                self.transition(id, RunStatus::Cancelled, None)?;
                return Ok(());
            }
            if request.tools.is_empty() && !response.message.tool_calls.is_empty() {
                return Err("provider attempted tools in a non-executing mode".into());
            }
            let calls = response.message.tool_calls.clone();
            self.store.update(&run.session_id, |s| {
                let r = s
                    .runs
                    .iter_mut()
                    .find(|r| r.id == id)
                    .ok_or("run not found")?;
                r.transcript.push(response.message.clone());
                r.pending_calls = calls.clone();
                if let Some(index) = message_index {
                    s.messages[index] = response.message.clone();
                }
                Ok(())
            })?;
            if calls.is_empty() {
                // Children still working are waited for, and results the run
                // has not read are added for one more turn, while a step
                // remains for it. The stored count includes this turn and any
                // retries it reserved.
                if self.store.run(id)?.steps < s.configuration.max_steps
                    && self.settle_children(id, &run.session_id, cancel)?
                {
                    continue;
                }
                // Announced only when this transition saved the completion: a
                // run cancelled, interrupted, or failed meanwhile keeps that
                // state and is not reported as succeeded.
                let (_, saved) = self.settle(id, RunStatus::Completed, None)?;
                if saved {
                    self.emit(
                        &run.session_id,
                        EventKind::AgentFinished {
                            agent_id: EntityId::new(id).map_err(|e| e.to_string())?,
                            outcome: Outcome::Succeeded,
                        },
                    );
                }
                return Ok(());
            }
        }
    }
    pub(crate) fn execute(
        &self,
        id: &str,
        call: &aidex_providers::ToolCall,
        p: &crate::broker::Prepared,
        issuer: PermitIssuer,
    ) -> Result<(), String> {
        let run = self.store.run(id)?;
        if run.status == RunStatus::Cancelled {
            return Err("run cancelled".into());
        }
        // The registry publishes `tool.started`, `file.changed`,
        // `diff.created`, and `tool.completed` or `tool.failed`, and the
        // launcher `process.*`, all as metadata.
        let output = self
            .broker
            .execute_observed(p, issuer, Arc::clone(&self.events))?;
        let mut checkpoints = vec![];
        for (path, before) in &p.before {
            let relative = WorkspacePath::new(path).map_err(|e| e.to_string())?;
            let absolute = p
                .context
                .root
                .resolve(&relative, aidex_workspace::Access::Create)
                .map_err(|e| e.to_string())?;
            let after_digest = if absolute.exists() {
                digest(&std::fs::read(absolute).map_err(|e| e.to_string())?)
            } else {
                "absent".into()
            };
            checkpoints.push(Checkpoint {
                id: aidex_native_sessions::id("checkpoint")?,
                run_id: id.into(),
                path: path.clone(),
                before: before.clone(),
                after_digest,
            });
        }
        self.store.update(&run.session_id, |s| {
            s.checkpoints.extend(checkpoints);
            Ok(())
        })?;
        // History holds the output as a model reads it, with diff statistics
        // in place of an applied diff (`crate::tool_view`): any daemon may
        // send stored results to a model as they are, an older one too. The
        // complete output (the applied diff with the file's earlier lines)
        // stays with the run, in encrypted history, for clients.
        let full = serde_json::to_string(&output).map_err(|e| e.to_string())?;
        match crate::tool_view::model_tool_result(&full) {
            Some(view) => self.append_output(id, &call.id, view, Some(full)),
            None => self.append_result(id, &call.id, full),
        }
    }
}
