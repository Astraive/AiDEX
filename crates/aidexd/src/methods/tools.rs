//! `tool.*` methods: tool descriptors and local-user invocations through the
//! broker (the harness submits agent invocations in-process).

use crate::{
    errors::RpcError,
    methods::{
        approvals::{broker_error, entity},
        policy::workspace_id,
    },
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_broker::{Origin, ToolInvocation};
use aidex_tools::{ToolDescriptor, ToolId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub use aidex_broker::BrokerOutcome;

pub const LIST: &str = "tool.list";
pub const INVOKE: &str = "tool.invoke";
pub const CONTINUE: &str = "tool.continue";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeParams {
    session: String,
    /// Defaults to the session's workspace.
    #[serde(default)]
    workspace: Option<String>,
    tool: String,
    #[serde(default)]
    input: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueParams {
    approval_id: String,
}

/// Result of `tool.list`.
#[derive(Serialize)]
pub struct ToolList<'a> {
    pub tools: Vec<&'a ToolDescriptor>,
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(LIST, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&ToolList {
            tools: shared.broker().tools(),
        })
    })?;

    let shared = Arc::clone(services);
    router.register(INVOKE, move |_context, params| {
        let params: InvokeParams = parse_params(params)?;
        let session_id = entity("session", &params.session)?;
        let session = shared.sessions().get(&session_id).map_err(|error| {
            RpcError::service(error.code(), error.to_string())
                .with_data(json!({"reason": "session_not_found"}))
        })?;
        let workspace = match params.workspace.as_deref() {
            Some(reference) => workspace_id(&shared, Some(reference))?,
            None => session.workspace_id.clone(),
        };
        if workspace != session.workspace_id {
            return Err(RpcError::invalid_params(format!(
                "session {session_id} belongs to workspace {}, not {workspace}",
                session.workspace_id
            )));
        }
        let tool = ToolId::new(params.tool.trim())
            .map_err(|error| RpcError::invalid_params(format!("tool: {error}")))?;
        let input = match params.input {
            Value::Null => Value::Object(serde_json::Map::new()),
            other => other,
        };
        let invocation = ToolInvocation::new(session_id, workspace, tool, input, Origin::User);
        let outcome = shared
            .broker()
            .submit(invocation)
            .map_err(|error| broker_error(&error))?;
        to_result(&outcome)
    })?;

    let shared = Arc::clone(services);
    router.register(CONTINUE, move |_context, params| {
        let params: ContinueParams = parse_params(params)?;
        let outcome = shared
            .broker()
            .continue_after_approval(&entity("approval_id", &params.approval_id)?)
            .map_err(|error| broker_error(&error))?;
        to_result(&outcome)
    })?;
    Ok(())
}
