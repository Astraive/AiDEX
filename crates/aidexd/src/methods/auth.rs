//! Metadata-only OAuth RPC adapter. Token exchange and refresh stay native.
use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::to_result,
    services::Services,
};
use aidex_auth::{AuthError, AuthService};
use aidex_providers::{
    AccountMetadata, AuthConfig, ProviderConfig, ProviderId, ProviderKind, ProviderRegistry,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::sync::Arc;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    provider_id: String,
    method: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    flow_id: Option<String>,
    provider_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FlowId {
    flow_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Provider {
    provider_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Open {
    authorization_url: String,
}
fn params<T: DeserializeOwned>(value: Value) -> Result<T, RpcError> {
    serde_json::from_value(if value.is_null() { json!({}) } else { value })
        .map_err(|_| RpcError::invalid_params("invalid auth parameters"))
}
fn error(error: AuthError) -> RpcError {
    RpcError::new(
        match error {
            AuthError::Locked => ErrorCode::VaultLocked,
            AuthError::Invalid => ErrorCode::InvalidArgument,
            AuthError::NotFound => ErrorCode::NotFound,
            AuthError::Conflict => ErrorCode::Conflict,
            _ => ErrorCode::Unavailable,
        },
        error.to_string(),
    )
    .with_data(json!({"reason":error.code()}))
}
/// Whether `config` is a ChatGPT account whose credential this daemon's OAuth
/// service owns (`oauth.<id>.access`).
pub(crate) fn owned(config: &ProviderConfig) -> bool {
    config.kind == ProviderKind::OpenaiResponses
        && config.base_url.trim_end_matches('/') == "https://api.openai.com/v1"
        && matches!(&config.auth,AuthConfig::VaultSecret {secret_name} if AuthService::owns_secret(config.id.as_str(),secret_name))
}
fn select_provider(registry: &ProviderRegistry, requested: &str) -> Result<String, RpcError> {
    let id =
        ProviderId::new(requested).map_err(|_| RpcError::invalid_params("invalid provider id"))?;
    if registry.provider_config(&id).as_ref().is_some_and(owned) {
        return Ok(requested.into());
    }
    let providers = registry.providers();
    if !providers.iter().any(|p| p.descriptor.id == id) {
        return Ok(requested.into());
    }
    // Existing API accounts retain their configuration and credentials.
    for index in 0..1000 {
        let candidate = if index == 0 {
            "openai-chatgpt".into()
        } else {
            format!("openai-chatgpt-{index}")
        };
        if !providers
            .iter()
            .any(|p| p.descriptor.id.as_str() == candidate)
        {
            return Ok(candidate);
        }
    }
    Err(error(AuthError::Conflict))
}
/// The ChatGPT account endpoint every OAuth-owned account uses.
const CHATGPT_BASE_URL: &str = "https://api.openai.com/v1";
/// The provider preset recorded for accounts created by ChatGPT sign-in.
const CHATGPT_PRESET: &str = "openai";

/// Register (or re-point) the ChatGPT account after sign-in. A reconnect keeps
/// the account's declared models, discovery flag, and account metadata; only
/// the kind, endpoint, and credential name are reasserted.
fn activate(
    registry: &Arc<ProviderRegistry>,
    provider: &str,
    secret: &str,
) -> Result<(), AuthError> {
    let id = ProviderId::new(provider).map_err(|_| AuthError::Invalid)?;
    let auth = AuthConfig::VaultSecret {
        secret_name: secret.into(),
    };
    if let Some(mut existing) = registry.provider_config(&id) {
        if !owned(&existing) {
            return Err(AuthError::Conflict);
        }
        existing.kind = ProviderKind::OpenaiResponses;
        existing.base_url = CHATGPT_BASE_URL.into();
        existing.auth = auth;
        registry.update_provider(existing).map(drop)
    } else {
        let mut config =
            ProviderConfig::new(id.clone(), ProviderKind::OpenaiResponses, CHATGPT_BASE_URL);
        config.auth = auth;
        registry
            .add_provider_with(
                config,
                AccountMetadata {
                    preset_id: Some(CHATGPT_PRESET.into()),
                    ..AccountMetadata::default()
                },
            )
            .map(drop)
    }
    .map_err(|_| AuthError::Unavailable)?;
    // Catalog calls resolve credentials; they run after the auth commit releases
    // its account mutex, and never report model listing as inference entitlement.
    let registry = registry.clone();
    let _ = std::thread::Builder::new()
        .name("aidex-chatgpt-catalog".into())
        .spawn(move || {
            if registry.default_model().is_some() {
                return;
            }
            // Only this account is listed; a disabled account sets no default.
            let Ok(models) = registry.models_for(&id) else {
                return;
            };
            if let Some(model) = models.into_iter().find(|model| model.capabilities.chat)
                && registry.default_model().is_none()
            {
                let _ = registry.set_default_model(model.model);
            }
        });
    Ok(())
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = services.clone();
    router.register("auth.start", move |_, value| {
        let params: Start = params(value)?;
        if params.method != "chatgpt" {
            return Err(RpcError::invalid_params("supported auth method is chatgpt"));
        }
        let provider = select_provider(shared.models().registry(), &params.provider_id)?;
        let registry = shared.models().registry().clone();
        to_result(
            &shared
                .models()
                .auth()
                .start(&provider, move |provider, secret| {
                    activate(&registry, provider, secret)
                })
                .map_err(error)?,
        )
    })?;
    let shared = services.clone();
    router.register("auth.status", move |_, value| {
        let params: Status = params(value)?;
        to_result(
            &shared
                .models()
                .auth()
                .status(params.flow_id.as_deref(), params.provider_id.as_deref())
                .map_err(error)?,
        )
    })?;
    let shared = services.clone();
    router.register("auth.cancel", move |_, value| {
        let params: FlowId = params(value)?;
        to_result(
            &shared
                .models()
                .auth()
                .cancel(&params.flow_id)
                .map_err(error)?,
        )
    })?;
    let shared = services.clone();
    router.register("auth.disconnect", move |_, value| {
        let params: Provider = params(value)?;
        to_result(
            &shared
                .models()
                .auth()
                .disconnect(&params.provider_id)
                .map_err(error)?,
        )
    })?;
    let shared = services.clone();
    router.register("auth.open", move |_, value| {
        let params: Open = params(value)?;
        shared
            .models()
            .auth()
            .open(&params.authorization_url)
            .map_err(error)?;
        Ok(json!({"opened":true}))
    })?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use serde_json::json;
    fn fixture() -> (tempfile::TempDir, Arc<Services>, Router, CallContext) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("auth-test", "1"), shutdown);
        (root, services, router, context)
    }
    #[test]
    fn oauth_rpc_requires_unlocked_vault_and_never_accepts_injected_open_urls() {
        let (_root, services, router, context) = fixture();
        services
            .vault()
            .init(
                &aidex_vault::SecretString::new("fixture passphrase only".into()),
                false,
            )
            .unwrap();
        services.vault().lock();
        let err = router
            .call(
                &context,
                "auth.start",
                json!({"provider_id":"openai","method":"chatgpt"}),
            )
            .unwrap_err();
        assert_eq!(err.kind(), Some(ErrorCode::VaultLocked));
        let err=router.call(&context,"auth.open",json!({"authorization_url":"https://auth.openai.com/api/accounts/authorize?injected=true"})).unwrap_err();
        assert_eq!(err.kind(), Some(ErrorCode::InvalidArgument));
        let err = router
            .call(
                &context,
                "auth.start",
                json!({"provider_id":"openai","method":"cli-token"}),
            )
            .unwrap_err();
        assert_eq!(err.kind(), Some(ErrorCode::InvalidParams));
        services
            .vault()
            .unlock(&aidex_vault::SecretString::new(
                "fixture passphrase only".into(),
            ))
            .unwrap();
        services.models().offline().set_offline(true).unwrap();
        let err = router
            .call(
                &context,
                "auth.start",
                json!({"provider_id":"openai","method":"chatgpt"}),
            )
            .unwrap_err();
        assert_eq!(err.kind(), Some(ErrorCode::Unavailable));
        assert_eq!(err.data.unwrap()["reason"], "offline_refused");
    }

    #[test]
    fn oauth_registration_preserves_api_accounts_and_existing_default() {
        use aidex_providers::{
            FakeProvider, Locality, ModelDescriptor, ModelRef, NoVault, OfflineMode,
        };
        let registry = Arc::new(ProviderRegistry::in_memory(
            Arc::new(NoVault::default()),
            Arc::new(OfflineMode::new(true)),
        ));
        let mut api = ProviderConfig::new(
            ProviderId::new("openai").unwrap(),
            ProviderKind::OpenaiResponses,
            "https://api.openai.com/v1",
        );
        api.auth = AuthConfig::VaultSecret {
            secret_name: "existing-api-account".into(),
        };
        registry.add_provider(api.clone()).unwrap();
        assert_eq!(
            select_provider(&registry, "openai").unwrap(),
            "openai-chatgpt"
        );
        let default = ModelRef::parse("local/fixture-coder").unwrap();
        registry
            .register(Box::new(
                FakeProvider::new(default.provider_id.clone(), Locality::Local)
                    .with_model(ModelDescriptor::new(default.clone(), Locality::Local, 8192)),
            ))
            .unwrap();
        registry.set_default_model(default.clone()).unwrap();
        activate(&registry, "openai-chatgpt", "oauth.openai-chatgpt.access").unwrap();
        assert_eq!(registry.provider_config(&api.id).unwrap(), api);
        assert_eq!(registry.default_model(), Some(default));
        assert_eq!(
            select_provider(&registry, "openai").unwrap(),
            "openai-chatgpt-1"
        );
        assert_eq!(
            select_provider(&registry, "openai-chatgpt").unwrap(),
            "openai-chatgpt"
        );
        let id = ProviderId::new("openai-chatgpt").unwrap();
        assert!(owned(&registry.provider_config(&id).unwrap()));
        let mut normalized = registry.provider_config(&id).unwrap();
        normalized.base_url = "https://api.openai.com/v1/".into();
        registry.update_provider(normalized).unwrap();
        assert_eq!(
            select_provider(&registry, "openai-chatgpt").unwrap(),
            "openai-chatgpt"
        );
        assert_eq!(
            activate(&registry, "openai", "oauth.openai.access"),
            Err(AuthError::Conflict)
        );
        assert_eq!(registry.provider_config(&api.id).unwrap(), api);
    }

    #[test]
    fn reconnect_keeps_declared_models_and_account_metadata() {
        use aidex_providers::{
            ModelConfig, ModelId, NoVault, OfflineMode, ProviderPatch, ToolCallSupport,
        };
        let registry = Arc::new(ProviderRegistry::in_memory(
            Arc::new(NoVault::default()),
            Arc::new(OfflineMode::new(true)),
        ));
        let id = ProviderId::new("openai-chatgpt").unwrap();
        let secret = AuthService::secret_name(id.as_str());
        activate(&registry, id.as_str(), &secret).unwrap();
        let created = registry.provider_entry(&id).unwrap();
        assert_eq!(created.preset_id.as_deref(), Some("openai"));
        assert_eq!(created.display_name, None);
        assert!(created.enabled);
        assert!(created.discover_models);

        let mut customized = registry.provider_config(&id).unwrap();
        let mut declared = ModelConfig::new(ModelId::new("gpt-5.1-codex").unwrap(), 400_000);
        declared.tool_calls = ToolCallSupport::Native;
        customized.models = vec![declared];
        customized.discover_models = false;
        customized.base_url = "https://api.openai.com/v1/".into();
        registry.update_provider(customized.clone()).unwrap();
        registry
            .patch_provider(
                &id,
                ProviderPatch {
                    display_name: Some(Some("Work ChatGPT".into())),
                    enabled: Some(false),
                },
            )
            .unwrap();

        activate(&registry, id.as_str(), &secret).unwrap();
        let reconnected = registry.provider_config(&id).unwrap();
        assert_eq!(reconnected.models, customized.models);
        assert!(!reconnected.discover_models);
        assert_eq!(reconnected.locality, customized.locality);
        // Only kind, endpoint, and credential are reasserted.
        assert_eq!(reconnected.base_url, "https://api.openai.com/v1");
        assert_eq!(reconnected.kind, ProviderKind::OpenaiResponses);
        assert_eq!(
            reconnected.auth,
            AuthConfig::VaultSecret {
                secret_name: secret.clone()
            }
        );
        let entry = registry.provider_entry(&id).unwrap();
        assert_eq!(entry.display_name.as_deref(), Some("Work ChatGPT"));
        assert_eq!(entry.preset_id.as_deref(), Some("openai"));
        assert!(!entry.enabled, "a reconnect does not re-enable the account");
    }
}
