//! Explicit managed native accounts; all credential data stays inside adapters.
use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_adapters::RuntimeError;
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{Arc, Mutex},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Id {
    id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Flow {
    id: String,
    flow_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    id: String,
    source: String,
}
fn error(e: RuntimeError) -> RpcError {
    RpcError::new(
        match e.code.as_str() {
            "vault_locked" => ErrorCode::VaultLocked,
            "invalid_request" | "invalid_config" | "profile_invalid" => ErrorCode::InvalidArgument,
            "not_found" => ErrorCode::NotFound,
            "runtime_busy" | "conflict" => ErrorCode::Conflict,
            "cancelled" => ErrorCode::Cancelled,
            _ => ErrorCode::Unavailable,
        },
        e.message,
    )
    .with_data(json!({"reason":e.code}))
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register("runtime.profile.import", move |_, p| {
        let p: Import = parse_params(p)?;
        let mut domain_error = None;
        let result = s
            .native()
            .mutate_runtimes(|| {
                let status = s
                    .runtime()
                    .import_profile(&p.id, Path::new(&p.source))
                    .map_err(|e| {
                        domain_error = Some(error(e));
                        "Native profile import refused".to_owned()
                    })?;
                let executors = s.runtime().executors().map_err(|e| {
                    domain_error = Some(error(e));
                    "Runtime registry unavailable".to_owned()
                })?;
                Ok((status, executors))
            })
            .map_err(|e| domain_error.unwrap_or_else(|| RpcError::new(ErrorCode::Conflict, e)))?;
        to_result(&result)
    })?;
    let s = services.clone();
    router.register("runtime.login.start", move |_, p| {
        let p: Id = parse_params(p)?;
        let mut domain_error = None;
        let result = s
            .native()
            .mutate_runtimes(|| {
                let status = s.runtime().login_start(&p.id).map_err(|e| {
                    domain_error = Some(error(e));
                    "Official login refused".to_owned()
                })?;
                let executors = s.runtime().executors().map_err(|e| {
                    domain_error = Some(error(e));
                    "Runtime registry unavailable".to_owned()
                })?;
                Ok((status, executors))
            })
            .map_err(|e| domain_error.unwrap_or_else(|| RpcError::new(ErrorCode::Conflict, e)))?;
        to_result(&result)
    })?;
    let s = services.clone();
    let connected_flows = Arc::new(Mutex::new(BTreeSet::<String>::new()));
    router.register("runtime.login.status", move |_, p| {
        let p: Flow = parse_params(p)?;
        let status = s.runtime().login_status(&p.id, &p.flow_id).map_err(error)?;
        let mut connected = connected_flows.lock().map_err(|_| {
            RpcError::new(
                ErrorCode::Unavailable,
                "Runtime account synchronization unavailable",
            )
        })?;
        if status.state == "connected" && !connected.contains(&status.flow_id) {
            let mut domain_error = None;
            s.native()
                .mutate_runtimes(|| {
                    let executors = s.runtime().executors().map_err(|e| {
                        domain_error = Some(error(e));
                        "Runtime registry unavailable".to_owned()
                    })?;
                    Ok(((), executors))
                })
                .map_err(|e| {
                    domain_error.unwrap_or_else(|| RpcError::new(ErrorCode::Conflict, e))
                })?;
            if connected.len() >= 64 {
                connected.clear();
            }
            connected.insert(status.flow_id.clone());
        }
        to_result(&status)
    })?;
    let s = services.clone();
    router.register("runtime.login.cancel", move |_, p| {
        let p: Flow = parse_params(p)?;
        to_result(&s.runtime().login_cancel(&p.id, &p.flow_id).map_err(error)?)
    })?;
    let s = services.clone();
    router.register("runtime.login.logout", move |_, p| {
        let p: Id = parse_params(p)?;
        let mut domain_error = None;
        let result = s
            .native()
            .mutate_runtimes(|| {
                let status = s.runtime().logout(&p.id).map_err(|e| {
                    domain_error = Some(error(e));
                    "Native logout refused".to_owned()
                })?;
                let executors = s.runtime().executors().map_err(|e| {
                    domain_error = Some(error(e));
                    "Runtime registry unavailable".to_owned()
                })?;
                Ok((status, executors))
            })
            .map_err(|e| domain_error.unwrap_or_else(|| RpcError::new(ErrorCode::Conflict, e)))?;
        to_result(&result)
    })?;
    let s = services.clone();
    router.register("runtime.account.status", move |_, p| {
        let p: Id = parse_params(p)?;
        to_result(&s.runtime().account_status(&p.id).map_err(error)?)
    })?;
    Ok(())
}
