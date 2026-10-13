//! `update.*` methods over the drive's updater (`aidex-updater`).
//!
//! `update.apply` and `update.rollback` stage, verify, self-test, and switch
//! `app/current.json`; they never replace the running daemon's own binaries. The
//! switched version takes effect on the next launch, so results carry
//! `restart_required` and clients tell the user to restart AiDEX.

use crate::{
    errors::RpcError,
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_core::ServiceErrorCode;
use aidex_updater::{
    ApplyOptions, RecoveryReport, RollbackOptions, UpdateError, UpdateSource, UpdateStatus, Updater,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

pub use aidex_updater::{ApplyReport, CheckReport, RollbackReport};

pub const STATUS: &str = "update.status";
pub const CHECK: &str = "update.check";
pub const APPLY: &str = "update.apply";
pub const ROLLBACK: &str = "update.rollback";

/// Params of `update.check` and `update.apply`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceParams {
    /// Absolute path of a package archive, an unpacked bundle, or a release directory.
    pub source: PathBuf,
    #[serde(default)]
    pub allow_downgrade: bool,
    /// Development only: accept a package without `bundle.sig`.
    #[serde(default)]
    pub allow_unsigned: bool,
}

/// Params of `update.rollback`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RollbackParams {
    #[serde(default)]
    pub allow_unsigned: bool,
}

/// Result of `update.status` (the `UpdateStatus` fields plus daemon facts).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct StatusResult {
    #[serde(flatten)]
    pub status: UpdateStatus,
    /// The running daemon's version.
    pub daemon_version: String,
    /// What recovery did when this daemon started (`null` if it has not run).
    pub recovery: Option<RecoveryReport>,
    /// Why startup recovery failed, if it did.
    pub recovery_error: Option<String>,
}

/// Map an update error to its wire error: the service code, plus `data.reason`
/// (stable, e.g. `bad_signature`, `downgrade`) and `data.integrity` (an authenticity
/// or integrity refusal).
pub fn update_error(error: &UpdateError) -> RpcError {
    RpcError::service(error.code(), error.to_string()).with_data(json!({
        "reason": error.reason(),
        "integrity": error.is_integrity(),
    }))
}

fn updater(services: &Services) -> Result<&Arc<Updater>, RpcError> {
    services.updates().map_err(|reason| {
        RpcError::service(
            ServiceErrorCode::Unavailable,
            format!("updates are unavailable on this daemon: {reason}"),
        )
        .with_data(json!({"reason": "updater_unavailable"}))
    })
}

fn source(params: &SourceParams) -> Result<UpdateSource, RpcError> {
    if !params.source.is_absolute() {
        return Err(RpcError::invalid_params(
            "`source` must be an absolute path (the daemon's working directory is not the client's)",
        ));
    }
    UpdateSource::from_path(&params.source).map_err(|error| update_error(&error))
}

fn options(params: &SourceParams) -> ApplyOptions {
    ApplyOptions {
        allow_downgrade: params.allow_downgrade,
        allow_unsigned: params.allow_unsigned,
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(STATUS, move |_context, params| {
        expect_no_params(&params)?;
        let status = updater(&shared)?
            .status()
            .map_err(|error| update_error(&error))?;
        let (recovery, recovery_error) = match shared.update_recovery() {
            Some(Ok(report)) => (Some(report), None),
            Some(Err(error)) => (None, Some(error)),
            None => (None, None),
        };
        to_result(&StatusResult {
            status,
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            recovery,
            recovery_error,
        })
    })?;

    let shared = Arc::clone(services);
    router.register(CHECK, move |_context, params| {
        let params: SourceParams = parse_params(params)?;
        let report = updater(&shared)?
            .check(&source(&params)?, &options(&params))
            .map_err(|error| update_error(&error))?;
        to_result(&report)
    })?;

    let shared = Arc::clone(services);
    router.register(APPLY, move |_context, params| {
        let params: SourceParams = parse_params(params)?;
        let report = updater(&shared)?
            .apply(&source(&params)?, &options(&params))
            .map_err(|error| update_error(&error))?;
        to_result(&report)
    })?;

    let shared = Arc::clone(services);
    router.register(ROLLBACK, move |_context, params| {
        let params: RollbackParams = if params.is_null() {
            RollbackParams::default()
        } else {
            parse_params(params)?
        };
        let report = updater(&shared)?
            .rollback(&RollbackOptions {
                allow_unsigned: params.allow_unsigned,
            })
            .map_err(|error| update_error(&error))?;
        to_result(&report)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::ErrorCode,
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use serde_json::Value;

    #[test]
    fn status_reports_an_empty_drive_and_sources_must_be_absolute() {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);

        let status: StatusResult =
            serde_json::from_value(router.call(&context, STATUS, Value::Null).unwrap()).unwrap();
        assert_eq!(status.status.current, None);
        assert!(!status.status.integrity.installed);
        assert_eq!(status.daemon_version, env!("CARGO_PKG_VERSION"));

        let error = router
            .call(&context, APPLY, json!({"source": "relative/package.zip"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidParams));
        let error = router
            .call(&context, CHECK, json!({"source": "/x", "bogus": true}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidParams));
        let error = router.call(&context, ROLLBACK, Value::Null).unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::NotFound));
    }

    /// `update.status` has exactly the keys of `UpdateStatus` and
    /// `IntegrityReport` in packages/client/src/dto/updates.ts.
    #[test]
    fn status_keys_match_the_client_types() {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        let keys = |value: &Value| {
            let mut keys: Vec<String> = value.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        };
        let status = router.call(&context, STATUS, Value::Null).unwrap();
        assert_eq!(
            keys(&status),
            [
                "current",
                "daemon_version",
                "integrity",
                "journal",
                "pending",
                "previous",
                "recovery",
                "recovery_error",
                "restart_required",
                "running_dir",
                "target",
                "versions",
            ]
        );
        assert_eq!(
            keys(&status["integrity"]),
            [
                "dir",
                "error",
                "installed",
                "manifest_sha256",
                "pending_update",
                "previous",
                "reason",
                "sbom",
                "scope",
                "signature",
                "signer_key_id",
                "target",
                "verified",
                "verified_files",
                "version",
            ]
        );
    }
}
