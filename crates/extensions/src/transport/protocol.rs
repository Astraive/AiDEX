use crate::{ExtensionError, McpConnection, McpTool, Result};
use aidex_tools::CancelToken;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(crate) trait Wire: Send {
    fn exchange(&mut self, message: Value, cancel: &CancelToken) -> Result<Option<Value>>;
    fn set_protocol(&mut self, _version: &str) {}
    fn disconnect(&mut self);
}
pub(crate) struct Connection {
    wire: Box<dyn Wire>,
    next: u64,
    initialized: bool,
}
impl Connection {
    pub fn new(wire: Box<dyn Wire>) -> Self {
        Self {
            wire,
            next: 1,
            initialized: false,
        }
    }
    fn request(&mut self, method: &str, params: Value, cancel: &CancelToken) -> Result<Value> {
        if cancel.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        let id = self.next;
        self.next = self.next.checked_add(1).ok_or(ExtensionError::TooLarge)?;
        let frame = self
            .wire
            .exchange(
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
                cancel,
            )?
            .ok_or_else(|| ExtensionError::Protocol("missing JSON-RPC response".into()))?;
        if frame.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || frame.get("id").and_then(Value::as_u64) != Some(id)
        {
            return Err(ExtensionError::Protocol(
                "mismatched JSON-RPC response".into(),
            ));
        }
        if frame.get("error").is_some() {
            return Err(ExtensionError::Protocol(
                "server rejected the request".into(),
            ));
        }
        frame
            .get("result")
            .cloned()
            .ok_or_else(|| ExtensionError::Protocol("response has no result".into()))
    }
}
impl McpConnection for Connection {
    fn initialize(&mut self, cancel: &CancelToken) -> Result<()> {
        let result=self.request("initialize",json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"aidex","version":"0.1.0"}}),cancel)?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| ExtensionError::Protocol("missing protocol version".into()))?;
        if !matches!(version, "2025-06-18" | "2025-03-26") {
            return Err(ExtensionError::Protocol(
                "unsupported negotiated protocol version".into(),
            ));
        }
        if result.pointer("/capabilities/tools").is_none() {
            return Err(ExtensionError::Protocol(
                "server does not advertise tools capability".into(),
            ));
        }
        self.wire.set_protocol(version);
        self.wire.exchange(
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            cancel,
        )?;
        self.initialized = true;
        Ok(())
    }
    fn list_tools(&mut self, cancel: &CancelToken) -> Result<Vec<McpTool>> {
        if !self.initialized {
            return Err(ExtensionError::Protocol(
                "connection is not initialized".into(),
            ));
        }
        let mut tools = Vec::new();
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        let mut names = BTreeSet::new();
        for _ in 0..32 {
            let params = cursor
                .as_ref()
                .map(|c| json!({"cursor":c}))
                .unwrap_or_else(|| json!({}));
            let page = self.request("tools/list", params, cancel)?;
            let values = page
                .get("tools")
                .and_then(Value::as_array)
                .ok_or_else(|| ExtensionError::Protocol("tools/list omitted tools".into()))?;
            if tools.len() + values.len() > 256 {
                return Err(ExtensionError::TooLarge);
            }
            for value in values {
                let tool: McpTool = serde_json::from_value(value.clone())
                    .map_err(|_| ExtensionError::Protocol("invalid tool definition".into()))?;
                if tool.name.is_empty()
                    || tool.name.len() > 128
                    || tool.name.chars().any(char::is_control)
                    || tool.description.len() > 8192
                    || !names.insert(tool.name.clone())
                    || tool.input_schema.get("type").and_then(Value::as_str) != Some("object")
                {
                    return Err(ExtensionError::Protocol(
                        "invalid or duplicate tool definition".into(),
                    ));
                }
                tools.push(tool);
            }
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(c) = &cursor else { return Ok(tools) };
            if c.len() > 4096 || !seen.insert(c.clone()) {
                return Err(ExtensionError::Protocol(
                    "pagination cursor repeated".into(),
                ));
            }
        }
        Err(ExtensionError::TooLarge)
    }
    fn call_tool(&mut self, name: &str, args: Value, cancel: &CancelToken) -> Result<Value> {
        if !self.initialized {
            return Err(ExtensionError::Protocol(
                "connection is not initialized".into(),
            ));
        }
        let result = self.request("tools/call", json!({"name":name,"arguments":args}), cancel)?;
        if !result.is_object()
            || !result.get("content").is_some_and(Value::is_array)
            || result.get("isError").is_some_and(|v| !v.is_boolean())
        {
            return Err(ExtensionError::Protocol(
                "invalid tool result envelope".into(),
            ));
        }
        Ok(result)
    }
    fn disconnect(&mut self) {
        self.wire.disconnect();
        self.initialized = false;
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.wire.disconnect();
    }
}
