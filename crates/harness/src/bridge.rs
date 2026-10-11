//! SDK additions stay inside the same sealed tool/policy boundary as native work.
use crate::NativeEngine;
use aidex_core::CancelToken;
use aidex_native_sessions::{Mode, PermissionMode, Run, RunStatus};
use aidex_providers::{ChatMessage, ToolCall};
use aidex_runtime_bridge::{
    BridgeConnection, BridgeDispatcher, BridgeError, BridgeScope, BridgeServer, BridgeTool,
};
use aidex_tools::PermitIssuer;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
const TOOLS: [&str; 6] = [
    "skill_read",
    "mcp_tools",
    "mcp_call",
    "mcp_stdio_call",
    "web_search",
    "web_fetch",
];
struct Dispatch(Weak<NativeEngine>);
impl BridgeDispatcher for Dispatch {
    fn active(&self, scope: &BridgeScope) -> bool {
        self.0
            .upgrade()
            .is_some_and(|engine| engine.bridge_scope(scope, false).is_ok())
    }
    fn tools(&self, scope: &BridgeScope) -> Result<Vec<BridgeTool>, BridgeError> {
        self.0
            .upgrade()
            .ok_or_else(|| problem("inactive_run"))?
            .extension_tools(scope)
    }
    fn call(
        &self,
        scope: &BridgeScope,
        request_id: &str,
        name: &str,
        arguments: Value,
        cancel: &CancelToken,
    ) -> Result<Value, BridgeError> {
        self.0
            .upgrade()
            .ok_or_else(|| problem("inactive_run"))?
            .invoke_extension(scope, request_id, name, arguments, cancel)
    }
}
fn problem(code: &'static str) -> BridgeError {
    BridgeError::new(
        code,
        match code {
            "run_paused" => "Run is paused for approval; no additional effect may execute",
            "approval_denied" => "User denied the tool; no effect was started",
            "cancelled" => "Scope stopped; inspect history before requesting a fresh call",
            "not_offered" => "This tool is not exposed by the scoped AiDEX bridge",
            "already_requested" => {
                "This call was already recorded; inspect history instead of replaying it"
            }
            _ => "Run/session scope is unavailable or changed; no automatic retry",
        },
    )
}
impl NativeEngine {
    fn bridge_scope(
        &self,
        scope: &BridgeScope,
        executing: bool,
    ) -> Result<(Run, String), BridgeError> {
        let run = self
            .store
            .run(&scope.run_id)
            .map_err(|_| problem("inactive_run"))?;
        let session = self
            .store
            .get(&scope.session_id)
            .map_err(|_| problem("inactive_run"))?;
        if run.session_id != scope.session_id
            || !run.status.active()
            || session.configuration.mode != Mode::Agent
            || !session
                .configuration
                .runtime_id
                .as_deref()
                .is_some_and(|id| id != "native")
        {
            return Err(problem("inactive_run"));
        }
        let live = self.live.lock().map_err(|_| problem("inactive_run"))?;
        let active = live.get(&run.id).ok_or_else(|| problem("inactive_run"))?;
        if active.provider.is_stopped() {
            return Err(problem("cancelled"));
        }
        if executing
            && (!active.executing || !matches!(run.status, RunStatus::Pending | RunStatus::Running))
        {
            return Err(problem("run_paused"));
        }
        Ok((run, active.generation.clone()))
    }
    pub fn extension_tools(&self, scope: &BridgeScope) -> Result<Vec<BridgeTool>, BridgeError> {
        self.bridge_scope(scope, false)?;
        let session = self
            .store
            .get(&scope.session_id)
            .map_err(|_| problem("inactive_run"))?;
        Ok(self.broker.tools.descriptors().into_iter().filter(|descriptor|TOOLS.contains(&descriptor.id.model_name().as_str())&&(session.configuration.permission_mode==PermissionMode::FullAccess||!descriptor.capabilities.iter().any(|tag|tag=="permissions:full_access"))).map(|descriptor|BridgeTool{name:descriptor.id.model_name(),description:descriptor.description.clone(),input_schema:descriptor.input_schema.clone(),annotations:json!({"readOnlyHint":descriptor.risk.is_read_only(),"destructiveHint":descriptor.risk.destructive,"openWorldHint":descriptor.risk.network,"idempotentHint":descriptor.risk.is_read_only()})}).collect())
    }
    pub(crate) fn runtime_bridge(
        self: &Arc<Self>,
        run: &Run,
        cancel: &CancelToken,
    ) -> Result<BridgeConnection, String> {
        let scope = BridgeScope {
            session_id: run.session_id.clone(),
            run_id: run.id.clone(),
        };
        if let Some(lease) = self
            .bridge_leases
            .lock()
            .map_err(|_| "bridge lease lock unavailable")?
            .get(&run.id)
        {
            if lease.active() {
                return Ok(lease.connection());
            }
            return Err("runtime MCP lease expired; start a fresh runtime session".into());
        }
        let server = {
            let mut slot = self
                .bridge_server
                .lock()
                .map_err(|_| "bridge server lock unavailable")?;
            if slot.is_none() {
                *slot = Some(BridgeServer::bind().map_err(|e| e.to_string())?);
            }
            slot.as_ref().unwrap().clone()
        };
        let lease = server
            .lease(
                scope,
                Arc::new(Dispatch(Arc::downgrade(self))),
                cancel.clone(),
                Duration::from_secs(3600),
            )
            .map_err(|e| e.to_string())?;
        let connection = lease.connection();
        self.bridge_leases
            .lock()
            .map_err(|_| "bridge lease lock unavailable")?
            .insert(run.id.clone(), lease);
        Ok(connection)
    }
    pub(crate) fn revoke_bridge(&self, session: &str, run: &str) {
        if let Ok(mut gates) = self.bridge_calls.lock() {
            gates.remove(run);
        }
        if let Ok(mut leases) = self.bridge_leases.lock() {
            leases.remove(run);
        }
        if let Ok(server) = self.bridge_server.lock()
            && let Some(server) = server.as_ref()
        {
            server.revoke_run(session, run);
        }
    }
    pub fn invoke_extension(
        &self,
        scope: &BridgeScope,
        request_id: &str,
        name: &str,
        arguments: Value,
        cancel: &CancelToken,
    ) -> Result<Value, BridgeError> {
        let (_, generation) = self.bridge_scope(scope, true)?;
        if !TOOLS.contains(&name)
            || !self
                .extension_tools(scope)?
                .iter()
                .any(|tool| tool.name == name)
        {
            return Err(problem("not_offered"));
        }
        if !arguments.is_object() || request_id.len() > 256 {
            return Err(BridgeError::new(
                "invalid_input",
                "tool arguments must be an object and request identity bounded",
            ));
        }
        let call_id = format!(
            "bridge_{}",
            &aidex_native_sessions::digest(format!("{}:{request_id}", scope.run_id).as_bytes())
                [..32]
        );
        let gate = {
            let mut gates = self
                .bridge_calls
                .lock()
                .map_err(|_| problem("inactive_run"))?;
            gates
                .entry(scope.run_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _gate = loop {
            if cancel.is_stopped() {
                return Err(problem("cancelled"));
            }
            match gate.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err(problem("inactive_run")),
            }
        };
        let (run, current_generation) = self.bridge_scope(scope, true)?;
        if generation != current_generation {
            return Err(problem("inactive_run"));
        }
        let session = self
            .store
            .get(&scope.session_id)
            .map_err(|_| problem("inactive_run"))?;
        if run
            .transcript
            .iter()
            .any(|message| message.tool_calls.iter().any(|call| call.id == call_id))
        {
            return Err(problem("already_requested"));
        }
        let call = ToolCall {
            id: call_id.clone(),
            name: name.into(),
            arguments: arguments.to_string(),
            provider_metadata: None,
        };
        self.store
            .update(&scope.session_id, |session| {
                let maximum = session.configuration.max_steps;
                let active = session
                    .runs
                    .iter_mut()
                    .find(|run| run.id == scope.run_id)
                    .ok_or("run missing")?;
                if active.status != RunStatus::Running || active.steps >= maximum {
                    return Err("run is paused or step budget exhausted".into());
                }
                active.steps += 1;
                let mut message = ChatMessage::assistant("");
                message.tool_calls.push(call.clone());
                active.transcript.push(message.clone());
                active.pending_calls.push(call.clone());
                if active.parent_run_id.is_none() {
                    session.messages.push(message);
                }
                Ok(())
            })
            .map_err(|_| problem("inactive_run"))?;
        let result = (|| {
            let mut prepared = self.broker.prepare(&session, &call).map_err(|error| {
                BridgeError::new(
                    "planning_denied",
                    aidex_events::redact_text(&error.to_string()).into_owned(),
                )
            })?;
            self.emit(
                &scope.session_id,
                aidex_events::EventKind::ToolRequested {
                    call_id: prepared.plan.call_id().clone(),
                    tool_id: prepared.plan.tool_id().entity_id().clone(),
                    summary: "SDK extension arguments stored in encrypted history".into(),
                    risk: prepared.plan.risk(),
                },
            );
            let issuer = if prepared.needs_approval {
                self.bridge_approvals
                    .request(self, &run, &call, &prepared, cancel)?
            } else {
                PermitIssuer::Policy {
                    rule: "runtime bridge evaluated policy".into(),
                }
            };
            let (_, latest_generation) = self.bridge_scope(scope, true)?;
            if latest_generation != generation || cancel.is_stopped() {
                return Err(problem("cancelled"));
            }
            prepared.context.cancel = self
                .live
                .lock()
                .map_err(|_| problem("inactive_run"))?
                .get(&scope.run_id)
                .ok_or_else(|| problem("inactive_run"))?
                .tool
                .clone();
            let tool_cancel = prepared.context.cancel.clone();
            let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
            std::thread::scope(|threads| {
                let finished_watch = finished.clone();
                let expected_generation = generation.clone();
                threads.spawn(move || {
                    while !finished_watch.load(std::sync::atomic::Ordering::SeqCst) {
                        if cancel.is_stopped()
                            || self
                                .bridge_scope(scope, true)
                                .map_or(true, |(_, current)| current != expected_generation)
                        {
                            tool_cancel.cancel();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                });
                let output = self.broker.execute(&prepared, issuer);
                finished.store(true, std::sync::atomic::Ordering::SeqCst);
                output.map_err(|_|BridgeError::new("tool_failed","tool failed or scope stopped; inspect encrypted history before a fresh request")).and_then(|output|serde_json::to_value(output).map_err(|_|problem("tool_failed")))
            })
        })();
        let output = match &result {
            Ok(value) => value.clone(),
            Err(error) => {
                json!({"is_error":true,"code":error.code,"message":error.message,"no_automatic_retry":true})
            }
        };
        if self
            .store
            .update(&scope.session_id, |session| {
                let active = session
                    .runs
                    .iter_mut()
                    .find(|run| run.id == scope.run_id)
                    .ok_or("run missing")?;
                if let Some(index) = active
                    .pending_calls
                    .iter()
                    .position(|pending| pending.id == call_id)
                {
                    active.pending_calls.remove(index);
                    let message = ChatMessage::tool_result(&call_id, output.to_string());
                    active.transcript.push(message.clone());
                    if active.parent_run_id.is_none() {
                        session.messages.push(message);
                    }
                } else {
                    active.transcript.push(ChatMessage::assistant(format!(
                        "Late SDK extension result (untrusted data):{output}"
                    )));
                }
                Ok(())
            })
            .is_err()
        {
            cancel.cancel();
            return Err(BridgeError::new(
                "persistence_failed",
                "result could not be saved; run cancelled; inspect effects before continuation",
            ));
        }
        result
    }
    pub(crate) fn wait_bridge_calls(&self, run: &str, cancel: &CancelToken) -> Result<(), String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(185);
        loop {
            if cancel.is_stopped() {
                return Err("run cancelled while waiting for SDK extension work".into());
            }
            if std::time::Instant::now() >= deadline {
                return Err("SDK extension work did not settle; inspect effects before explicit continuation".into());
            }
            let pending = self
                .store
                .run(run)?
                .pending_calls
                .iter()
                .any(|call| call.id.starts_with("bridge_"));
            if !pending && !self.bridge_approvals.pending_for_run(run) {
                let gate = self
                    .bridge_calls
                    .lock()
                    .map_err(|_| "bridge call gate unavailable")?
                    .get(run)
                    .cloned();
                let guard = match gate.as_ref().map(|gate| gate.try_lock()) {
                    Some(Ok(guard)) => Some(guard),
                    Some(Err(std::sync::TryLockError::WouldBlock)) => {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    }
                    Some(Err(_)) => return Err("bridge call gate unavailable".into()),
                    None => None,
                };
                // Close effect admission while holding the same per-run gate.
                // Calls already queued recheck the executing scope after it opens.
                if let Some(live) = self
                    .live
                    .lock()
                    .map_err(|_| "native run registry unavailable")?
                    .get_mut(run)
                {
                    live.executing = false;
                }
                drop(guard);
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
