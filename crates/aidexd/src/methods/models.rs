//! `model.*`, `provider.*`, and `network.*` methods over the provider registry,
//! the model router, and the offline switch.
//!
//! Results expose context limits, tool support, locality, capability sources,
//! routing reasons, rejections, and per-provider listing failures. Provider
//! configuration accepts a vault secret *name* only; no method accepts or
//! returns a credential value.

use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_providers::{
    AuthConfig, Locality, ModelConfig, ModelDescriptor, ModelEvent, ModelId, ModelListing,
    ModelRef, ProviderConfig, ProviderEntry, ProviderError, ProviderFailure, ProviderHealth,
    ProviderId, ProviderKind, RegistryError,
};
use aidex_router::{
    HostSummary, NetworkState, PrivacyMode, RouteDecision, RouteError, RouteRequest, TaskKind,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub const MODEL_LIST: &str = "model.list";
pub const MODEL_PROVIDERS: &str = "model.providers";
pub const MODEL_ROUTE: &str = "model.route";
pub const MODEL_USE: &str = "model.use";
pub const MODEL_DEFAULT: &str = "model.default";
pub const PROVIDER_ADD: &str = "provider.add";
pub const PROVIDER_UPDATE: &str = "provider.update";
pub const PROVIDER_PRESETS: &str = "provider.presets";
pub const PROVIDER_REMOVE: &str = "provider.remove";
pub const PROVIDER_HEALTH: &str = "provider.health";
pub const NETWORK_OFFLINE: &str = "network.offline";
pub const NETWORK_STATUS: &str = "network.status";

/// Result of `model.list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelList {
    pub models: Vec<ModelDescriptor>,
    /// Providers whose listing failed (offline refusal, unreachable, ...).
    pub failures: Vec<ProviderFailure>,
    pub default_model: Option<ModelRef>,
    pub network: NetworkState,
}

/// Result of `model.providers`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderList {
    pub providers: Vec<ProviderEntry>,
}

/// Params of `model.route`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RouteParams {
    pub task: TaskKind,
    /// Defaults to `hybrid`.
    #[serde(default)]
    pub privacy: Option<PrivacyMode>,
    #[serde(default)]
    pub required_context: u32,
    #[serde(default)]
    pub needs_tools: bool,
    #[serde(default)]
    pub needs_json_schema: bool,
    #[serde(default)]
    pub secret_heavy: bool,
    /// Explicit `provider/model` override.
    #[serde(default)]
    pub model: Option<String>,
}

/// Result of `model.route`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RouteResult {
    pub decision: RouteDecision,
    /// The selected model's descriptor (context limit, tools, locality).
    pub model: ModelDescriptor,
    pub network: NetworkState,
    pub failures: Vec<ProviderFailure>,
}

/// Params of `model.use`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UseParams {
    pub provider_id: String,
    pub model_id: String,
}

/// Result of `model.use` and `model.default`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DefaultModel {
    pub default_model: Option<ModelRef>,
    /// Present after `model.use` (the descriptor just validated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelDescriptor>,
}

/// Params of `provider.add`. There is deliberately no field for a key: name a
/// vault secret whose policy allows the consumer `provider:<id>`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderAddParams {
    pub id: String,
    #[serde(default = "default_kind")]
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(default)]
    pub secret_name: Option<String>,
    #[serde(default)]
    pub locality: Option<Locality>,
    #[serde(default = "default_true")]
    pub discover_models: bool,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

fn default_kind() -> ProviderKind {
    ProviderKind::OpenaiCompatible
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderIdParams {
    pub id: String,
}

/// Result of `provider.remove`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderRemoved {
    pub removed: ProviderId,
}

/// Result of `provider.health` (the health fields flattened next to the id).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProviderHealthResult {
    pub provider_id: ProviderId,
    #[serde(flatten)]
    pub health: ProviderHealth,
}

/// Params of `network.offline`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OfflineParams {
    pub enabled: bool,
}

/// Result of `network.offline` and `network.status`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NetworkStatus {
    pub offline: bool,
    pub network: NetworkState,
}

fn kind_data(kind: &str) -> serde_json::Value {
    json!({ "kind": kind })
}

/// Map a provider failure to its stable wire code; `data.kind` names it.
pub fn provider_error(error: &ProviderError) -> RpcError {
    let code = match error {
        ProviderError::OfflineRefused { .. } | ProviderError::PolicyDenied { .. } => {
            ErrorCode::PolicyDenied
        }
        ProviderError::Unauthorized | ProviderError::Credential(_) => ErrorCode::VaultLocked,
        ProviderError::Cancelled => ErrorCode::Cancelled,
        ProviderError::InvalidRequest(_) | ProviderError::ContextOverflow { .. } => {
            ErrorCode::InvalidArgument
        }
        ProviderError::ModelNotFound { .. } => ErrorCode::NotFound,
        ProviderError::Unavailable { .. }
        | ProviderError::RateLimited { .. }
        | ProviderError::Timeout
        | ProviderError::Protocol(_)
        | ProviderError::Http { .. } => ErrorCode::ModelUnavailable,
    };
    RpcError::new(code, error.to_string()).with_data(kind_data(error.kind().as_str()))
}

fn registry_error(error: RegistryError) -> RpcError {
    match &error {
        RegistryError::Provider { error: inner, .. } => {
            let mut rpc = provider_error(inner);
            rpc.message = error.to_string();
            rpc
        }
        RegistryError::Invalid(_) => RpcError::new(ErrorCode::InvalidArgument, error.to_string()),
        _ => RpcError::service(error.code(), error.to_string()),
    }
}

fn route_error(error: &RouteError, failures: &[ProviderFailure]) -> RpcError {
    let code = match error {
        RouteError::OverrideNotFound { .. } => ErrorCode::NotFound,
        RouteError::NoEligibleModel { .. } | RouteError::OverrideRejected { .. } => {
            ErrorCode::ModelUnavailable
        }
    };
    let reasons = match error {
        RouteError::NoEligibleModel { reasons, .. } => reasons.clone(),
        _ => Vec::new(),
    };
    RpcError::new(code, error.to_string()).with_data(json!({
        "kind": "no_route",
        "reasons": reasons,
        "rejected": error.rejected(),
        "failures": failures,
    }))
}

fn provider_id(text: &str) -> Result<ProviderId, RpcError> {
    ProviderId::new(text).map_err(|error| RpcError::invalid_params(error.to_string()))
}

fn list(services: &Services) -> ModelList {
    let models = services.models();
    let ModelListing {
        models: list,
        failures,
    } = models.registry().models();
    ModelList {
        models: list,
        failures,
        default_model: models.registry().default_model(),
        network: models.network(),
    }
}

fn route(services: &Services, params: RouteParams) -> Result<RouteResult, RpcError> {
    let models = services.models();
    let user_override = params
        .model
        .as_deref()
        .map(ModelRef::parse)
        .transpose()
        .map_err(|error| RpcError::invalid_params(error.to_string()))?;
    let listing = models.registry().models();
    let request = RouteRequest {
        task: params.task,
        privacy: params.privacy.unwrap_or_default(),
        network: models.network(),
        required_context: params.required_context,
        needs_tools: params.needs_tools,
        needs_json_schema: params.needs_json_schema,
        secret_heavy: params.secret_heavy,
        user_override,
        preferred: models.registry().default_model(),
        // Host RAM/GPU discovery is not available yet (aidex-host reports
        // platform identifiers only), so RAM constraints are not applied.
        host: HostSummary::default(),
    };
    let decision = models
        .router()
        .route(&request, &listing.models)
        .map_err(|error| route_error(&error, &listing.failures))?;
    let model = listing
        .models
        .iter()
        .find(|model| model.model == decision.primary)
        .cloned()
        .ok_or_else(|| RpcError::internal("routed model is missing from the listing"))?;
    // `model.route` only explains a decision; `model.selected` is emitted by
    // whoever actually runs the model (the harness).
    Ok(RouteResult {
        decision,
        model,
        network: models.network(),
        failures: listing.failures,
    })
}

fn add(
    services: &Services,
    params: ProviderAddParams,
    update: bool,
) -> Result<ProviderEntry, RpcError> {
    let mut config = ProviderConfig::new(provider_id(&params.id)?, params.kind, params.base_url);
    config.locality = params.locality;
    config.discover_models = params.discover_models;
    config.models = params.models;
    if let Some(secret_name) = params.secret_name {
        config.auth = AuthConfig::VaultSecret { secret_name };
    }
    let registry = services.models().registry();
    let descriptor = if update {
        registry.update_provider(config)
    } else {
        registry.add_provider(config)
    }
    .map_err(registry_error)?;
    Ok(ProviderEntry {
        descriptor,
        origin: aidex_providers::ProviderOrigin::Configured,
    })
}

fn network_status(services: &Services) -> NetworkStatus {
    let models = services.models();
    NetworkStatus {
        offline: models.offline().is_offline(),
        network: models.network(),
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(MODEL_LIST, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&list(&shared))
    })?;
    let shared = Arc::clone(services);
    router.register(MODEL_PROVIDERS, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&ProviderList {
            providers: shared.models().registry().providers(),
        })
    })?;
    let shared = Arc::clone(services);
    router.register(MODEL_ROUTE, move |_context, params| {
        to_result(&route(&shared, parse_params(params)?)?)
    })?;
    let shared = Arc::clone(services);
    router.register(MODEL_USE, move |_context, params| {
        let params: UseParams = parse_params(params)?;
        let model = ModelRef::new(
            provider_id(&params.provider_id)?,
            ModelId::new(params.model_id)
                .map_err(|error| RpcError::invalid_params(error.to_string()))?,
        );
        let descriptor = shared
            .models()
            .registry()
            .set_default_model(model.clone())
            .map_err(registry_error)?;
        to_result(&DefaultModel {
            default_model: Some(model),
            model: Some(descriptor),
        })
    })?;
    let shared = Arc::clone(services);
    router.register(MODEL_DEFAULT, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&DefaultModel {
            default_model: shared.models().registry().default_model(),
            model: None,
        })
    })?;
    let shared = Arc::clone(services);
    router.register(PROVIDER_ADD, move |_context, params| {
        to_result(&add(&shared, parse_params(params)?, false)?)
    })?;
    let shared = Arc::clone(services);
    router.register(PROVIDER_UPDATE, move |_context, params| {
        to_result(&add(&shared, parse_params(params)?, true)?)
    })?;
    router.register(PROVIDER_PRESETS, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&json!({ "presets": aidex_providers::provider_presets() }))
    })?;
    let shared = Arc::clone(services);
    router.register(PROVIDER_REMOVE, move |_context, params| {
        let params: ProviderIdParams = parse_params(params)?;
        let id = provider_id(&params.id)?;
        shared
            .models()
            .registry()
            .remove_provider(&id)
            .map_err(registry_error)?;
        to_result(&ProviderRemoved { removed: id })
    })?;
    let shared = Arc::clone(services);
    router.register(PROVIDER_HEALTH, move |_context, params| {
        let params: ProviderIdParams = parse_params(params)?;
        let id = provider_id(&params.id)?;
        let health = shared
            .models()
            .registry()
            .health(&id)
            .map_err(registry_error)?;
        to_result(&ProviderHealthResult {
            provider_id: id,
            health,
        })
    })?;
    let shared = Arc::clone(services);
    router.register(NETWORK_OFFLINE, move |_context, params| {
        let params: OfflineParams = parse_params(params)?;
        let models = shared.models();
        models
            .offline()
            .set_offline(params.enabled)
            .map_err(|error| RpcError::service(error.code(), error.to_string()))?;
        if params.enabled {
            // Close already admitted network work and idle processes with keys.
            // The live gate above also rejects racing reconnects.
            shared.native().cancel_all();
            shared.extensions().disconnect_all();
            let _ = shared.runtime().cancel_logins();
        }
        models.observer().observe(&ModelEvent::NetworkModeChanged {
            offline: params.enabled,
        });
        to_result(&network_status(&shared))
    })?;
    let shared = Arc::clone(services);
    router.register(NETWORK_STATUS, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&network_status(&shared))
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
    use aidex_providers::{FakeProvider, ModelRole, ToolCallSupport};

    #[test]
    fn presets_and_account_update_expose_protocol_and_persist_custom_models() {
        let (_root, services, router, context) = fixture();
        let presets = router.call(&context, "provider.presets", serde_json::Value::Null);
        assert!(
            presets.is_ok(),
            "provider presets must be exposed: {presets:?}"
        );
        let presets = presets.unwrap();
        let presets = presets["presets"].as_array().unwrap();
        assert!(
            presets
                .iter()
                .any(|p| p["id"] == "anthropic" && p["kind"] == "anthropic")
        );
        assert!(
            presets
                .iter()
                .any(|p| p["id"] == "openrouter" && p["kind"] == "openai-compatible")
        );
        router.call(&context, PROVIDER_ADD, json!({"id":"custom","base_url":"http://127.0.0.1:1/v1","discover_models":false,"models":[{"id":"custom-a","context_window":8192}]})).unwrap();
        let updated = router.call(&context, "provider.update", json!({"id":"custom","base_url":"http://127.0.0.1:2/v1","discover_models":false,"models":[{"id":"custom-b","context_window":16384,"tool_calls":"native"}]}));
        assert!(
            updated.is_ok(),
            "account update must be exposed: {updated:?}"
        );
        assert_eq!(
            services.models().registry().models().models[0]
                .model
                .model_id
                .as_str(),
            "custom-b"
        );
        assert_eq!(
            services.models().registry().models().models[0].context_window,
            16384
        );
        let stored = services
            .models()
            .registry()
            .provider_config(&ProviderId::new("custom").unwrap())
            .unwrap();
        assert_eq!(stored.base_url, "http://127.0.0.1:2/v1");
    }

    fn fixture() -> (tempfile::TempDir, Arc<Services>, Router, CallContext) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        (root, services, router, context)
    }

    fn register_fake(services: &Services) {
        let id = ProviderId::new("local").unwrap();
        let provider = FakeProvider::new(id.clone(), Locality::Local).with_model(
            ModelDescriptor::new(
                ModelRef::new(id, ModelId::new("coder").unwrap()),
                Locality::Local,
                16_384,
            )
            .with_roles([ModelRole::Code])
            .with_tool_calls(ToolCallSupport::Structured),
        );
        services
            .models()
            .registry()
            .register(Box::new(provider))
            .unwrap();
    }

    #[test]
    fn route_explains_the_choice_and_use_persists_the_default() {
        let (_root, services, router, context) = fixture();
        register_fake(&services);
        let routed: RouteResult = serde_json::from_value(
            router
                .call(
                    &context,
                    MODEL_ROUTE,
                    json!({"task": "code", "needs_tools": true}),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(routed.decision.primary.to_string(), "local/coder");
        assert_eq!(routed.model.context_window, 16_384);
        assert!(!routed.decision.reasons.is_empty());

        let refused = router
            .call(
                &context,
                MODEL_ROUTE,
                json!({"task": "code", "required_context": 100000}),
            )
            .unwrap_err();
        assert_eq!(refused.kind(), Some(ErrorCode::ModelUnavailable));
        let data = refused.data.unwrap();
        assert_eq!(data["rejected"][0]["reason"], "context_too_small");

        router
            .call(
                &context,
                MODEL_USE,
                json!({"provider_id": "local", "model_id": "coder"}),
            )
            .unwrap();
        let default: DefaultModel =
            serde_json::from_value(router.call(&context, MODEL_DEFAULT, json!({})).unwrap())
                .unwrap();
        assert_eq!(default.default_model.unwrap().to_string(), "local/coder");
        let missing = router
            .call(
                &context,
                MODEL_USE,
                json!({"provider_id": "local", "model_id": "nope"}),
            )
            .unwrap_err();
        assert_eq!(missing.kind(), Some(ErrorCode::NotFound));
    }

    #[test]
    fn provider_add_rejects_keys_and_offline_refuses_remote_listing() {
        let (_root, _services, router, context) = fixture();
        let with_key = router
            .call(
                &context,
                PROVIDER_ADD,
                json!({"id": "openai", "base_url": "https://api.openai.com/v1", "api_key": "sk-x"}),
            )
            .unwrap_err();
        assert_eq!(with_key.kind(), Some(ErrorCode::InvalidParams));
        let entry: ProviderEntry = serde_json::from_value(
            router
                .call(
                    &context,
                    PROVIDER_ADD,
                    json!({"id": "openai", "base_url": "https://api.openai.com/v1", "secret_name": "openai-key"}),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(entry.descriptor.locality, Locality::Remote);
        let status: NetworkStatus = serde_json::from_value(
            router
                .call(&context, NETWORK_OFFLINE, json!({"enabled": true}))
                .unwrap(),
        )
        .unwrap();
        assert!(status.offline);
        let listing: ModelList =
            serde_json::from_value(router.call(&context, MODEL_LIST, json!({})).unwrap()).unwrap();
        assert_eq!(listing.network, NetworkState::Offline);
        assert_eq!(listing.failures[0].kind.as_str(), "offline_refused");
        let health: ProviderHealthResult = serde_json::from_value(
            router
                .call(&context, PROVIDER_HEALTH, json!({"id": "openai"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(health.health.status, aidex_providers::HealthStatus::Refused);
        router
            .call(&context, PROVIDER_REMOVE, json!({"id": "openai"}))
            .unwrap();
        assert_eq!(
            router
                .call(&context, PROVIDER_REMOVE, json!({"id": "openai"}))
                .unwrap_err()
                .kind(),
            Some(ErrorCode::NotFound)
        );
    }
}
