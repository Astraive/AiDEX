//! Provider turns and scoped tool exchange; no blind retry of side effects.
use crate::{
    NativeEngine,
    engine::{PendingPlan, relock},
    inference::Turn,
    native_tools::{AGENT_RESULT, AGENT_SPAWN},
};
use aidex_core::{CancelToken, EntityId, WorkspacePath};
use aidex_events::{EventKind, Outcome};
use aidex_native_sessions::*;
use aidex_providers::{ChatMessage, ChatRequest, ModelId, ToolCallSupport, ToolChoice, ToolSpec};
use aidex_tools::PermitIssuer;
use std::{sync::Arc, time::Instant};

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
        relock(&self.live).remove(id);
        self.store.update(&run.session_id, |s| {
            s.approvals.push(approval.clone());
            s.runs
                .iter_mut()
                .find(|r| r.id == id)
                .ok_or("run not found")?
                .status = RunStatus::WaitingApproval;
            Ok(())
        })?;
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
        let mut prepared = self.broker.prepare(s, call)?;
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
        self.transition(id, RunStatus::Running, None)?;
        loop {
            if cancel.is_cancelled() {
                self.transition(id, RunStatus::Cancelled, None)?;
                return Ok(());
            }
            let run = self.store.run(id)?;
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
            let messages = self.assemble(&s, &run, &offer)?;
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
                self.transition(id, RunStatus::Completed, None)?;
                self.emit(
                    &run.session_id,
                    EventKind::AgentFinished {
                        agent_id: EntityId::new(id).map_err(|e| e.to_string())?,
                        outcome: Outcome::Succeeded,
                    },
                );
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
        self.emit(
            &run.session_id,
            EventKind::ToolStarted {
                call_id: p.plan.call_id().clone(),
            },
        );
        let started = Instant::now();
        let output = self.broker.execute(p, issuer)?;
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
        self.emit(
            &run.session_id,
            EventKind::ToolCompleted {
                call_id: p.plan.call_id().clone(),
                duration_ms: started.elapsed().as_millis() as u64,
                summary: "native tool completed; output in encrypted history".into(),
            },
        );
        self.append_result(id, &call.id, output.content.to_string())
    }
}
