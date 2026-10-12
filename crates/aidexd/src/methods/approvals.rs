//! `broker.approval.*` methods over the execution broker's approval store. (The
//! native harness's run approvals are `approval.list|respond|arguments` in
//! `methods/native.rs`; they are a separate store.)
//!
//! Every IPC client is the local user (authenticated loopback); remote devices
//! will resolve approvals through the remote protocol (#29), recorded with their
//! device principal.

use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_broker::{
    BrokerError, Principal,
    approvals::{ApprovalFilter, ApprovalResolution, ApprovalStatus},
};
use aidex_core::EntityId;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub use aidex_broker::approvals::{ApprovalCard, ApprovalRecord, ApprovalState};

pub const LIST: &str = "broker.approval.list";
pub const SHOW: &str = "broker.approval.show";
pub const RESOLVE: &str = "broker.approval.resolve";
pub const REVOKE: &str = "broker.approval.revoke";

/// Params of `approval.list`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
    /// `pending`, `approved`, `denied`, `expired`, `revoked`, or `cancelled`.
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveParams {
    id: String,
    /// `approve_once`, `approve_session`, or `deny`.
    decision: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeParams {
    id: String,
    #[serde(default)]
    reason: Option<String>,
}

/// Result of `approval.list` (newest first).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApprovalList {
    pub approvals: Vec<ApprovalRecord>,
}

/// Map a broker error to its wire error with `data.reason`. An emergency stop is
/// `policy_denied` (-32021).
pub fn broker_error(error: &BrokerError) -> RpcError {
    let rpc = match error {
        BrokerError::Stopped { .. } => RpcError::new(ErrorCode::PolicyDenied, error.to_string()),
        _ => RpcError::service(error.code(), error.to_string()),
    };
    rpc.with_data(json!({ "reason": error.reason() }))
}

pub(crate) fn entity(field: &str, value: &str) -> Result<EntityId, RpcError> {
    EntityId::new(value.trim())
        .map_err(|error| RpcError::invalid_params(format!("{field}: {error}")))
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(LIST, move |_context, params| {
        let params: ListParams = parse_params(params)?;
        let filter = ApprovalFilter {
            session_id: params
                .session
                .as_deref()
                .map(|id| entity("session", id))
                .transpose()?,
            workspace_id: params
                .workspace
                .as_deref()
                .map(|id| entity("workspace", id))
                .transpose()?,
            status: params
                .state
                .as_deref()
                .map(|state| {
                    state
                        .parse::<ApprovalStatus>()
                        .map_err(|error| RpcError::invalid_params(error.to_string()))
                })
                .transpose()?,
            limit: params.limit,
        };
        let approvals = shared
            .broker()
            .list_approvals(&filter)
            .map_err(|error| broker_error(&error))?;
        to_result(&ApprovalList { approvals })
    })?;

    let shared = Arc::clone(services);
    router.register(SHOW, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let record = shared
            .broker()
            .approval(&entity("id", &params.id)?)
            .map_err(|error| broker_error(&error))?;
        to_result(&record)
    })?;

    let shared = Arc::clone(services);
    router.register(RESOLVE, move |_context, params| {
        let params: ResolveParams = parse_params(params)?;
        let decision: ApprovalResolution = params
            .decision
            .parse()
            .map_err(|error: BrokerError| RpcError::invalid_params(error.to_string()))?;
        let record = shared
            .broker()
            .resolve_approval(&entity("id", &params.id)?, decision, &Principal::LocalUser)
            .map_err(|error| broker_error(&error))?;
        to_result(&record)
    })?;

    let shared = Arc::clone(services);
    router.register(REVOKE, move |_context, params| {
        let params: RevokeParams = parse_params(params)?;
        let record = shared
            .broker()
            .revoke_approval(
                &entity("id", &params.id)?,
                &Principal::LocalUser,
                params.reason.as_deref(),
            )
            .map_err(|error| broker_error(&error))?;
        to_result(&record)
    })?;
    Ok(())
}
