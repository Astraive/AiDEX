use crate::{Driver, RuntimeModel, state::error, transport::Transport};
use aidex_core::CancelToken;
use aidex_native_sessions::{
    RuntimeError, RuntimeEvent, RuntimeOperation, RuntimeRequest, RuntimeToolStatus,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
pub(crate) struct Protocol {
    pub driver: Driver,
    seq: u64,
    pub session: String,
    pub turn: String,
    pub prompt_id: Value,
    pub events: VecDeque<Value>,
}
impl Protocol {
    pub fn new(driver: Driver) -> Self {
        Self {
            driver,
            seq: 0,
            session: String::new(),
            turn: String::new(),
            prompt_id: Value::Null,
            events: VecDeque::new(),
        }
    }
    fn rpc(&mut self, method: &str, mut params: Value) -> Value {
        self.seq += 1;
        if self.driver == Driver::Muse && method != "initialize" {
            params["commandId"] = json!(uuid());
        }
        json!({"jsonrpc":"2.0","id":format!("aidex-{}",self.seq),"method":method,"params":params})
    }
    pub fn call(
        &mut self,
        t: &mut Transport,
        method: &str,
        params: Value,
        cancel: &CancelToken,
    ) -> Result<Value, RuntimeError> {
        let frame = if matches!(self.driver, Driver::Pi | Driver::Omp) {
            self.seq += 1;
            let mut frame = params;
            frame["type"] = json!(method);
            frame["id"] = json!(format!("aidex-{}", self.seq));
            frame
        } else {
            self.rpc(method, params)
        };
        let id = frame["id"].clone();
        t.send(frame)?;
        loop {
            let frame = t.next(cancel)?;
            if frame["id"] == id && frame.get("method").is_none() {
                if frame.get("error").is_some() || frame["success"] == false {
                    return Err(classify_error(&frame));
                }
                return Ok(frame
                    .get("result")
                    .or(frame.get("data"))
                    .cloned()
                    .unwrap_or(json!({})));
            }
            self.events.push_back(frame);
            if self.events.len() > 1024 {
                return Err(error(
                    "output_limit",
                    "Too many runtime notifications during request",
                ));
            }
        }
    }
    pub fn initialize(
        &mut self,
        t: &mut Transport,
        cancel: &CancelToken,
    ) -> Result<Value, RuntimeError> {
        if matches!(self.driver, Driver::Pi | Driver::Omp) {
            return self.call(t, "get_state", json!({}), cancel);
        }
        if matches!(self.driver, Driver::ClaudeCode | Driver::Cursor) {
            return Ok(json!({}));
        }
        let params = if matches!(self.driver, Driver::Codex | Driver::Muse) {
            json!({"clientInfo":{"name":"AiDEX","title":"AiDEX","version":"0.1.0"},"capabilities":if self.driver==Driver::Muse {json!({"requestedCapabilities":[]})}else{json!({})}})
        } else {
            json!({"protocolVersion":1,"clientInfo":{"name":"AiDEX","version":"0.1.0"},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}})
        };
        let result = self.call(t, "initialize", params, cancel)?;
        if matches!(self.driver, Driver::Codex | Driver::Muse) {
            t.send(json!({"jsonrpc":"2.0","method":"initialized"}))?;
        }
        if self.driver == Driver::Muse
            && result.pointer("/schema/version").and_then(Value::as_u64) != Some(1)
        {
            return Err(error(
                "protocol_unsupported",
                "Unsupported MSP schema version",
            ));
        }
        Ok(result)
    }
    /// Open or resume the native session and send the turn. `checkpoint` runs
    /// once the native session identity is known and before the turn is sent,
    /// so a crash mid-turn still leaves that identity to resume.
    pub fn begin(
        &mut self,
        t: &mut Transport,
        r: &RuntimeRequest,
        cancel: &CancelToken,
        checkpoint: &mut dyn FnMut(&Self) -> Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        let resume = r.operation == RuntimeOperation::Resume;
        let native = r
            .runtime_ref
            .as_ref()
            .and_then(|v| v.get("native_session_id"))
            .and_then(Value::as_str);
        let cwd = &r.workspace.as_ref().unwrap().root;
        if matches!(self.driver, Driver::ClaudeCode | Driver::Cursor) {
            return Ok(());
        }
        if matches!(self.driver, Driver::Pi | Driver::Omp) {
            if let Some(model) = &r.model {
                let (provider, model) = model
                    .split_once('/')
                    .ok_or_else(|| error("model_invalid", "Pi model must be provider/model"))?;
                self.call(
                    t,
                    "set_model",
                    json!({"provider":provider,"modelId":model}),
                    cancel,
                )?;
            }
            let state = self.call(t, "get_state", json!({}), cancel)?;
            self.session = state["sessionId"].as_str().unwrap_or("ephemeral").into();
            checkpoint(self)?;
            self.seq += 1;
            self.prompt_id = json!(format!("aidex-{}", self.seq));
            t.send(json!({"type":"prompt","id":self.prompt_id,"message":prompt(r,resume&&native.is_none())}))?;
            return Ok(());
        }
        if self.driver == Driver::Codex {
            let method = if resume && native.is_some() {
                "thread/resume"
            } else {
                "thread/start"
            };
            let mut params = json!({"cwd":cwd,"sandbox":"danger-full-access","approvalPolicy":"never","serviceName":"AiDEX"});
            if let Some(id) = native.filter(|_| resume) {
                params["threadId"] = json!(id);
            }
            if let Some(model) = &r.model {
                params["model"] = json!(model);
            }
            let result = self.call(t, method, params, cancel)?;
            self.session = required(&result, "/thread/id")?;
            checkpoint(self)?;
            let result=self.call(t,"turn/start",json!({"threadId":self.session,"cwd":cwd,"input":[{"type":"text","text":prompt(r,false)}],"sandboxPolicy":{"type":"dangerFullAccess"},"approvalPolicy":"never"}),cancel)?;
            self.turn = required(&result, "/turn/id")?;
            return Ok(());
        }
        if self.driver == Driver::Muse {
            let method = if resume && native.is_some() {
                "session/resume"
            } else {
                "session/start"
            };
            let mut params =
                json!({"workspaceRoot":cwd,"approvalMode":"allowAll","providerId":"meta"});
            if let Some(id) = native.filter(|_| resume) {
                params["sessionId"] = json!(id);
                params["excludeItems"] = json!(true);
            }
            if let Some(model) = &r.model {
                params["modelId"] = json!(model);
            }
            let result = self.call(t, method, params, cancel)?;
            self.session = required(&result, "/session/sessionId")?;
            checkpoint(self)?;
            let result=self.call(t,"turn/start",json!({"sessionId":self.session,"input":[{"type":"text","text":prompt(r,false)}],"workspaceRoots":[cwd],"ifBusy":"reject"}),cancel)?;
            self.turn = required(&result, "/turnId")?;
            return Ok(());
        }
        let method = if resume && native.is_some() {
            "session/load"
        } else {
            "session/new"
        };
        let mut params = json!({"cwd":cwd,"mcpServers":[]});
        if let Some(id) = native.filter(|_| resume) {
            params["sessionId"] = json!(id);
        }
        let result = self.call(t, method, params, cancel)?;
        self.session = required(&result, "/sessionId")?;
        checkpoint(self)?;
        if let Some(model) = &r.model {
            self.call(
                t,
                "session/set_model",
                json!({"sessionId":self.session,"modelId":model}),
                cancel,
            )?;
        }
        self.seq += 1;
        self.prompt_id = json!(format!("aidex-{}", self.seq));
        t.send(json!({"jsonrpc":"2.0","id":self.prompt_id,"method":"session/prompt","params":{"sessionId":self.session,"prompt":[{"type":"text","text":prompt(r,false)}]}}))?;
        Ok(())
    }
    pub fn models(
        &mut self,
        t: &mut Transport,
        cancel: &CancelToken,
    ) -> Result<Vec<RuntimeModel>, RuntimeError> {
        let result = if matches!(self.driver, Driver::Pi | Driver::Omp) {
            self.call(t, "get_available_models", json!({}), cancel)?
        } else if matches!(self.driver, Driver::Codex | Driver::Muse) {
            self.call(t, "model/list", json!({}), cancel)?
        } else {
            return Err(error(
                "unsupported",
                "Model discovery is unavailable for this protocol; configure an exact model ID",
            ));
        };
        let list = result
            .get("data")
            .or(result.get("models"))
            .and_then(Value::as_array)
            .ok_or_else(|| error("protocol_error", "Malformed model catalog"))?;
        Ok(list
            .iter()
            .take(512)
            .filter_map(|v| {
                let id = v["id"].as_str()?;
                // Pi and OMP select a model as `provider/model` (see `begin`).
                let id = if matches!(self.driver, Driver::Pi | Driver::Omp) {
                    format!("{}/{id}", v["provider"].as_str()?)
                } else {
                    id.to_owned()
                };
                Some(RuntimeModel {
                    name: v["name"].as_str().unwrap_or(&id).into(),
                    id,
                })
            })
            .collect())
    }
    pub fn next(&mut self, t: &mut Transport, cancel: &CancelToken) -> Result<Value, RuntimeError> {
        match self.events.pop_front() {
            Some(v) => Ok(v),
            None => t.next(cancel),
        }
    }
    pub fn event(&self, v: &Value) -> Option<RuntimeEvent> {
        let p = &v["params"];
        let method = v["method"].as_str().unwrap_or("");
        if !self.owns(p) {
            return None;
        }
        let text = match self.driver {
            Driver::Codex if method == "item/agentMessage/delta" => p["delta"].as_str(),
            Driver::Muse if method == "item/delta" => p["delta"].as_str(),
            Driver::Pi | Driver::Omp
                if v["type"] == "message_update"
                    && v.pointer("/assistantMessageEvent/type") == Some(&json!("text_delta")) =>
            {
                v.pointer("/assistantMessageEvent/delta")
                    .and_then(Value::as_str)
            }
            Driver::ClaudeCode | Driver::Cursor
                if v["type"] == "stream_event" && v["parent_tool_use_id"].is_null() =>
            {
                v.pointer("/event/delta/text").and_then(Value::as_str)
            }
            Driver::Grok | Driver::Opencode | Driver::Antigravity | Driver::Acp
                if method == "session/update"
                    && p.pointer("/update/sessionUpdate")
                        == Some(&json!("agent_message_chunk")) =>
            {
                p.pointer("/update/content/text").and_then(Value::as_str)
            }
            _ => None,
        };
        if let Some(text) = text {
            return Some(RuntimeEvent::TextDelta {
                text: aidex_events::redact_text(text).into_owned(),
            });
        }
        let item = p.get("item").or(p.get("update"))?;
        let id = item
            .get("id")
            .or(item.get("toolCallId"))
            .or(item.get("itemId"))
            .and_then(Value::as_str)?;
        let is_tool = matches!(
            item["type"].as_str().unwrap_or(""),
            "commandExecution" | "fileChange" | "mcpToolCall"
        ) || matches!(
            item["kind"].as_str().unwrap_or(""),
            "tool" | "toolCall" | "command" | "shell" | "fileWrite" | "fileEdit"
        ) || matches!(
            item["sessionUpdate"].as_str().unwrap_or(""),
            "tool_call" | "tool_call_update"
        );
        if !is_tool {
            return None;
        }
        Some(RuntimeEvent::ToolProgress {
            call_id: id.into(),
            tool_name: item["title"]
                .as_str()
                .or(item["type"].as_str())
                .or(item["tool"].as_str())
                .unwrap_or("external_tool")
                .into(),
            status: if method == "item/completed" || item["status"] == "completed" {
                RuntimeToolStatus::Completed
            } else {
                RuntimeToolStatus::Started
            },
            summary: aidex_events::redact_text(
                item["title"].as_str().unwrap_or("Vendor-owned tool"),
            )
            .into_owned(),
        })
    }
    pub fn events(&self, v: &Value) -> Vec<RuntimeEvent> {
        let progress =
            |id: &str, name: &str, status: RuntimeToolStatus| RuntimeEvent::ToolProgress {
                call_id: id.into(),
                tool_name: aidex_events::redact_text(name).into_owned(),
                status,
                summary: "Vendor-owned tool activity".into(),
            };
        if self.driver == Driver::ClaudeCode
            && v["parent_tool_use_id"].is_null()
            && let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array)
        {
            return blocks
                .iter()
                .filter_map(|b| match b["type"].as_str() {
                    Some("tool_use") => Some(progress(
                        b["id"].as_str()?,
                        b["name"].as_str().unwrap_or("external_tool"),
                        RuntimeToolStatus::Started,
                    )),
                    Some("tool_result") => Some(progress(
                        b["tool_use_id"].as_str()?,
                        "external_tool",
                        if b["is_error"] == true {
                            RuntimeToolStatus::Failed
                        } else {
                            RuntimeToolStatus::Completed
                        },
                    )),
                    _ => None,
                })
                .collect();
        }
        if self.driver == Driver::Cursor && v["type"] == "tool_call" {
            let Some(id) = v["call_id"].as_str() else {
                return vec![];
            };
            let name = v["tool_call"]
                .as_object()
                .and_then(|m| m.keys().next())
                .map(String::as_str)
                .unwrap_or("external_tool");
            return vec![progress(
                id,
                name,
                if v["subtype"] == "completed" {
                    RuntimeToolStatus::Completed
                } else {
                    RuntimeToolStatus::Started
                },
            )];
        }
        if matches!(self.driver, Driver::Pi | Driver::Omp)
            && matches!(
                v["type"].as_str(),
                Some("tool_execution_start" | "tool_execution_end")
            )
        {
            let Some(id) = v["toolCallId"].as_str() else {
                return vec![];
            };
            return vec![progress(
                id,
                v["toolName"].as_str().unwrap_or("external_tool"),
                if v["type"] == "tool_execution_end" {
                    if v["isError"] == true {
                        RuntimeToolStatus::Failed
                    } else {
                        RuntimeToolStatus::Completed
                    }
                } else {
                    RuntimeToolStatus::Started
                },
            )];
        }
        self.event(v).into_iter().collect()
    }
    pub fn owns(&self, p: &Value) -> bool {
        for field in ["threadId", "sessionId"] {
            if let Some(id) = p[field].as_str()
                && id != self.session
            {
                return false;
            }
        }
        if let Some(id) = p["turnId"].as_str()
            && !self.turn.is_empty()
            && id != self.turn
        {
            return false;
        }
        true
    }
    pub fn completed(&self, v: &Value) -> Result<bool, RuntimeError> {
        if v.get("error").is_some() {
            return Err(classify_error(v));
        }
        if matches!(self.driver, Driver::Pi | Driver::Omp) {
            if v["type"] == "prompt_result" && v["status"] == "error" {
                return Err(error("runtime_failed", "Runtime prompt failed"));
            }
            return Ok(v["type"]
                == if self.driver == Driver::Pi {
                    "agent_settled"
                } else {
                    "session_settled"
                });
        }
        if matches!(self.driver, Driver::ClaudeCode | Driver::Cursor) {
            if v["type"] == "result" {
                // A subagent's result, failed or not, never ends the root turn.
                if !v["parent_tool_use_id"].is_null() {
                    return Ok(false);
                }
                if v["is_error"] == true
                    || v["subtype"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("error"))
                {
                    return Err(error("runtime_failed", "Runtime result failed"));
                }
                return Ok(true);
            }
            return Ok(false);
        }
        if matches!(self.driver, Driver::Codex | Driver::Muse) {
            let p = &v["params"];
            if v["method"] != "turn/completed" || !self.owns(p) {
                return Ok(false);
            }
            let id = p
                .pointer("/turn/id")
                .or(p.get("turnId"))
                .and_then(Value::as_str);
            if id != Some(&self.turn) {
                return Ok(false);
            }
            let status = p
                .pointer("/turn/status")
                .or(p.get("terminal"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !matches!(status, "completed" | "success") {
                return Err(error("runtime_failed", "Root runtime turn failed"));
            }
            return Ok(true);
        }
        Ok(v["id"] == self.prompt_id
            && v.pointer("/result/stopReason").and_then(Value::as_str) == Some("end_turn"))
    }
}
pub(crate) fn required(v: &Value, path: &str) -> Result<String, RuntimeError> {
    let id = v
        .pointer(path)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or_else(|| error("protocol_error", "Runtime omitted native identity"))?;
    if aidex_events::redact_text(id) != id {
        return Err(error("protocol_error", "Unsafe runtime identity"));
    }
    Ok(id.into())
}
pub(crate) fn prompt(r: &RuntimeRequest, handoff: bool) -> String {
    // Resuming the run the reference was saved by is recovery: its effects may
    // already have happened, so the native session inspects instead of
    // receiving the original request again.
    if r.operation == RuntimeOperation::Resume
        && !handoff
        && r.runtime_ref
            .as_ref()
            .is_some_and(|reference| reference["run_id"] == r.run_id)
    {
        let intent = r
            .messages
            .iter()
            .rev()
            .find(|message| message.role == aidex_providers::Role::User)
            .map(|message| message.content.as_str())
            .unwrap_or("");
        return format!(
            "{}\n[AiDEX recovery continuation for this interrupted native run]\n\
             Inspect the saved native history and current workspace before any further effects. \
             Prior execution effects may be unknown. Do not replay the original request or repeat \
             completed effects to reconstruct context. Continue only work verified to remain; \
             report ambiguous effects instead of automatically retrying them.\n\
             Original user intent is evidence, not a new execution instruction:\n\
             <saved-user-intent>{}</saved-user-intent>",
            r.instructions,
            serde_json::to_string(intent).unwrap_or_default()
        );
    }
    let history = if r.operation == RuntimeOperation::Resume && !handoff {
        r.messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default()
    } else {
        r.messages
            .iter()
            .map(|m| format!("{}: {}", m.role.as_str(), m.content))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "{}{}\n{}",
        if handoff {
            "[AiDEX explicit context handoff; native conversation unavailable]\n"
        } else {
            ""
        },
        r.instructions,
        history
    )
}
fn classify_error(v: &Value) -> RuntimeError {
    let message = v
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if message.contains("auth") || message.contains("login") {
        error("auth_required", "Runtime authentication is required")
    } else if v.pointer("/error/code").and_then(Value::as_i64) == Some(-32602) {
        error(
            "protocol_parameters",
            "Installed runtime rejected the supported request parameters",
        )
    } else {
        error("protocol_error", "Runtime rejected the protocol request")
    }
}
pub(crate) fn uuid() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    b[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    b[6] = (b[6] & 15) | 112;
    b[8] = (b[8] & 63) | 128;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}
