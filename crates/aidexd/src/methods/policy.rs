//! `policy.*` methods: the broker's active settings, profile changes by the local
//! user, document reloads, and dry-run explanations.

use crate::{
    errors::RpcError,
    methods::approvals::{broker_error, entity},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_broker::{Origin, PolicyChange, Principal, ToolInvocation};
use aidex_policy::Profile;
use aidex_tools::ToolId;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

pub use aidex_broker::{Explanation, PolicyStatus};

pub const SHOW: &str = "policy.show";
pub const PROFILE_SET: &str = "policy.profile.set";
pub const RELOAD: &str = "policy.reload";
pub const EXPLAIN: &str = "policy.explain";

/// Session id used by explanations that name no session.
const EXPLAIN_SESSION: &str = "ses_policy_explain";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileParams {
    /// `safe`, `balanced`, `autonomous`, or `offline_secure`.
    profile: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExplainParams {
    tool: String,
    #[serde(default)]
    input: Value,
    #[serde(default)]
    session: Option<String>,
    /// Defaults to the selected workspace.
    #[serde(default)]
    workspace: Option<String>,
    /// `agent` (default: what an agent would be allowed) or `user`.
    #[serde(default)]
    origin: Option<String>,
}

/// Accept `offline-secure` as a spelling of `offline_secure`.
pub fn parse_profile(text: &str) -> Result<Profile, RpcError> {
    text.trim()
        .replace('-', "_")
        .parse::<Profile>()
        .map_err(|_| {
            RpcError::invalid_params(format!(
                "unknown profile `{text}` (expected safe, balanced, autonomous, or offline_secure)"
            ))
        })
}

/// The workspace a request names, or the selected one.
pub(crate) fn workspace_id(
    services: &Services,
    workspace: Option<&str>,
) -> Result<aidex_core::EntityId, RpcError> {
    match workspace {
        Some(reference) => {
            let found = services.workspaces().find(reference).map_err(|error| {
                RpcError::service(error.code(), error.to_string())
                    .with_data(serde_json::json!({"reason": error.reason()}))
            })?;
            Ok(found.id)
        }
        None => services
            .workspaces()
            .selected()
            .map_err(|error| RpcError::service(error.code(), error.to_string()))?
            .map(|selected| selected.id)
            .ok_or_else(|| {
                RpcError::invalid_params(
                    "no workspace given and none is selected (`aidex workspace select`)",
                )
            }),
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(SHOW, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&shared.broker().policy_status())
    })?;

    let shared = Arc::clone(services);
    router.register(PROFILE_SET, move |_context, params| {
        let params: ProfileParams = parse_params(params)?;
        let profile = parse_profile(&params.profile)?;
        let status = shared
            .broker()
            .change_policy(PolicyChange::Profile(profile), &Principal::LocalUser)
            .map_err(|error| broker_error(&error))?;
        to_result(&status)
    })?;

    let shared = Arc::clone(services);
    router.register(RELOAD, move |_context, params| {
        expect_no_params(&params)?;
        let status = shared
            .broker()
            .change_policy(PolicyChange::ReloadDocuments, &Principal::LocalUser)
            .map_err(|error| broker_error(&error))?;
        to_result(&status)
    })?;

    let shared = Arc::clone(services);
    router.register(EXPLAIN, move |_context, params| {
        let params: ExplainParams = parse_params(params)?;
        let tool = ToolId::new(params.tool.trim())
            .map_err(|error| RpcError::invalid_params(format!("tool: {error}")))?;
        let session = entity(
            "session",
            params.session.as_deref().unwrap_or(EXPLAIN_SESSION),
        )?;
        let workspace = workspace_id(&shared, params.workspace.as_deref())?;
        let input = match params.input {
            Value::Null => Value::Object(serde_json::Map::new()),
            other => other,
        };
        let origin = match params.origin.as_deref().unwrap_or("agent") {
            "agent" => Origin::Agent,
            "user" => Origin::User,
            other => {
                return Err(RpcError::invalid_params(format!(
                    "origin: unknown origin `{other}` (expected agent or user)"
                )));
            }
        };
        let invocation = ToolInvocation::new(session, workspace, tool, input, origin);
        let explanation = shared
            .broker()
            .explain(&invocation)
            .map_err(|error| broker_error(&error))?;
        to_result(&explanation)
    })?;
    Ok(())
}
