use crate::{
    BridgeError, http,
    server::{Cached, Record, digest},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rpc {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Initialize {
    protocol_version: String,
    capabilities: Value,
    client_info: Value,
    #[serde(rename = "_meta")]
    _meta: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    cursor: Option<String>,
    #[serde(rename = "_meta")]
    _meta: Option<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Call {
    name: String,
    #[serde(default = "object")]
    arguments: Value,
    #[serde(rename = "_meta")]
    _meta: Option<Value>,
}
fn object() -> Value {
    json!({})
}
fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn tool_error(error: BridgeError) -> Value {
    json!({"isError":true,"content":[{"type":"text","text":format!("{}: {}",error.code,error.message)}],"structuredContent":{"code":error.code,"message":error.message}})
}
pub(crate) fn handle(record: &Record, input: Value) -> (u16, Option<Value>) {
    let rpc: Rpc = match serde_json::from_value(input) {
        Ok(rpc) => rpc,
        Err(_) => {
            return (
                400,
                Some(rpc_error(Value::Null, -32600, "invalid JSON-RPC request")),
            );
        }
    };
    if rpc.jsonrpc != "2.0"
        || rpc.id.as_ref().is_some_and(|id| {
            (!id.is_string() && !id.is_i64() && !id.is_u64())
                || id.as_str().is_some_and(|value| value.len() > 128)
        })
    {
        return (
            400,
            Some(rpc_error(Value::Null, -32600, "invalid JSON-RPC identity")),
        );
    }
    let id = rpc.id.clone().unwrap_or(Value::Null);
    if rpc.method == "notifications/initialized" {
        if rpc.id.is_some()
            || (!rpc.params.is_null() && !rpc.params.is_object())
            || record.phase.load(Ordering::SeqCst) == 0
        {
            return (400, None);
        }
        record.phase.store(2, Ordering::SeqCst);
        return (202, None);
    }
    if rpc.id.is_none() {
        return (400, None);
    }
    let result = match rpc.method.as_str() {
        "initialize" => {
            let params: Initialize = match serde_json::from_value(rpc.params) {
                Ok(params) => params,
                Err(_) => {
                    return (
                        200,
                        Some(rpc_error(id, -32602, "invalid initialize parameters")),
                    );
                }
            };
            if !params.capabilities.is_object()
                || !params.client_info.is_object()
                || params
                    .client_info
                    .get("name")
                    .and_then(Value::as_str)
                    .is_none()
                || params
                    .client_info
                    .get("version")
                    .and_then(Value::as_str)
                    .is_none()
            {
                return (200, Some(rpc_error(id, -32602, "invalid client metadata")));
            }
            let version = if params.protocol_version == "2025-03-26" {
                "2025-03-26"
            } else {
                "2025-06-18"
            };
            record.phase.store(1, Ordering::SeqCst);
            json!({"protocolVersion":version,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"aidex-runtime","version":"0.1.0"}})
        }
        "ping" => json!({}),
        "tools/list" => {
            if record.phase.load(Ordering::SeqCst) != 2 {
                return (
                    200,
                    Some(rpc_error(
                        id,
                        -32000,
                        "initialize this MCP connection first",
                    )),
                );
            }
            let params: List = match serde_json::from_value(if rpc.params.is_null() {
                json!({})
            } else {
                rpc.params
            }) {
                Ok(params) => params,
                Err(_) => {
                    return (
                        200,
                        Some(rpc_error(id, -32602, "invalid tools/list parameters")),
                    );
                }
            };
            if params.cursor.is_some() {
                return (
                    200,
                    Some(rpc_error(id, -32602, "catalog has no continuation cursor")),
                );
            }
            match record.dispatcher.tools(&record.scope) {
                Ok(tools) => json!({"tools":tools}),
                Err(error) => return (200, Some(rpc_error(id, -32000, &error.message))),
            }
        }
        "tools/call" => {
            if record.phase.load(Ordering::SeqCst) != 2 {
                return (
                    200,
                    Some(rpc_error(
                        id,
                        -32000,
                        "initialize this MCP connection first",
                    )),
                );
            }
            let call: Call = match serde_json::from_value(rpc.params.clone()) {
                Ok(call) => call,
                Err(_) => {
                    return (
                        200,
                        Some(rpc_error(id, -32602, "invalid tools/call parameters")),
                    );
                }
            };
            if !call.arguments.is_object() {
                return (
                    200,
                    Some(rpc_error(id, -32602, "tool arguments must be an object")),
                );
            }
            let tools = match record.dispatcher.tools(&record.scope) {
                Ok(tools) => tools,
                Err(error) => return (200, Some(rpc_error(id, -32000, &error.message))),
            };
            if !tools.iter().any(|tool| tool.name == call.name) {
                return (
                    200,
                    Some(rpc_error(id, -32602, "tool is not offered by this run")),
                );
            }
            let key = id.to_string();
            let fingerprint = digest(rpc.params.to_string().as_bytes());
            {
                let mut calls = record.calls.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(cached) = calls.get(&key) {
                    if cached.fingerprint != fingerprint {
                        return (
                            200,
                            Some(rpc_error(
                                id,
                                -32602,
                                "request ID reused with different arguments",
                            )),
                        );
                    }
                    return (
                        200,
                        Some(cached.result.clone().unwrap_or_else(|| {
                            rpc_error(
                                id,
                                -32001,
                                "request is still running; do not repeat effects",
                            )
                        })),
                    );
                }
                if calls.len() >= 128 {
                    return (
                        200,
                        Some(rpc_error(id, -32000, "bridge request budget exhausted")),
                    );
                }
                calls.insert(
                    key.clone(),
                    Cached {
                        fingerprint,
                        result: None,
                    },
                );
            }
            let cancel = record.cancel.with_deadline(
                record
                    .expires
                    .min(std::time::Instant::now() + Duration::from_secs(180)),
            );
            let result = match record.dispatcher.call(
                &record.scope,
                &key,
                &call.name,
                call.arguments,
                &cancel,
            ) {
                Ok(value) => {
                    json!({"isError":false,"content":[{"type":"text","text":value.to_string()}],"structuredContent":value})
                }
                Err(error) => tool_error(error),
            };
            let mut reply = json!({"jsonrpc":"2.0","id":id,"result":result});
            if reply.to_string().len() > http::LIMIT {
                reply = json!({"jsonrpc":"2.0","id":id,"result":tool_error(BridgeError::new("output_too_large","output saved in encrypted run history; response exceeds 1 MiB"))});
            }
            let mut calls = record.calls.lock().unwrap_or_else(|e| e.into_inner());
            let used = calls
                .values()
                .filter_map(|call| call.result.as_ref())
                .map(|value| value.to_string().len())
                .sum::<usize>();
            let cached = if used + reply.to_string().len() > 2 * http::LIMIT {
                json!({"jsonrpc":"2.0","id":id,"result":tool_error(BridgeError::new("result_saved","effect was already handled; inspect encrypted run history instead of replaying"))})
            } else {
                reply.clone()
            };
            if let Some(entry) = calls.get_mut(&key) {
                entry.result = Some(cached);
            }
            return (200, Some(reply));
        }
        _ => {
            return (
                200,
                Some(rpc_error(
                    id,
                    -32601,
                    "method is not supported by the runtime bridge",
                )),
            );
        }
    };
    (200, Some(json!({"jsonrpc":"2.0","id":id,"result":result})))
}
