use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::to_result,
    services::Services,
};
use aidex_web::{SearchProvider, SearchSettings, WebError};
use serde::Deserialize;
use std::sync::Arc;
fn error(e: WebError) -> RpcError {
    let code = match e {
        WebError::Invalid => ErrorCode::InvalidParams,
        WebError::CredentialUnavailable => ErrorCode::VaultLocked,
        WebError::Offline => ErrorCode::PolicyDenied,
        WebError::Conflict => ErrorCode::Conflict,
        _ => ErrorCode::Unavailable,
    };
    RpcError::new(code, e.to_string())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configure {
    provider: SearchProvider,
    enabled: bool,
    api_key: Option<String>,
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register("web.status", move |_, p| {
        crate::rpc::expect_no_params(&p)?;
        to_result(&s.web().status().map_err(error)?)
    })?;
    let s = services.clone();
    router.register("web.configure", move |_, p| {
        if s.vault().status().state != aidex_vault::VaultState::Unlocked {
            return Err(RpcError::new(
                ErrorCode::VaultLocked,
                "Unlock the vault before configuring search",
            ));
        }
        let p: Configure = serde_json::from_value(p).map_err(|_| {
            RpcError::invalid_params(
                "expected {provider: brave|tavily, enabled: boolean, api_key?: string}",
            )
        })?;
        to_result(
            &s.web()
                .configure(
                    SearchSettings {
                        provider: p.provider,
                        enabled: p.enabled,
                        ..Default::default()
                    },
                    p.api_key.map(aidex_vault::SecretBytes::from),
                )
                .map_err(error)?,
        )
    })?;
    let s = services.clone();
    router.register("web.disconnect", move |_, p| {
        crate::rpc::expect_no_params(&p)?;
        if s.vault().status().state != aidex_vault::VaultState::Unlocked {
            return Err(RpcError::new(
                ErrorCode::VaultLocked,
                "Unlock the vault before disconnecting search",
            ));
        }
        to_result(&s.web().disconnect().map_err(error)?)
    })?;
    Ok(())
}
