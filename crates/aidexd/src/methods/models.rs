//! `model.*`, `provider.*`, and `network.*` methods over the provider registry,
//! the model router, and the offline switch.
//!
//! Results expose context limits, tool support, locality, capability sources,
//! routing reasons, rejections, and per-provider listing failures. Provider
//! configuration accepts a vault secret *name* only; no method accepts or
//! returns a credential value. Account metadata (display name, preset,
//! enabled flag) is kept in the registry's `provider-accounts.toml` sidecar,
//! never in `providers.toml`.

use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_auth::AuthError;
use aidex_providers::{
    AccountMetadata, AuthConfig, Locality, ModelConfig, ModelDescriptor, ModelEvent, ModelId,
    ModelInput, ModelListing, ModelRef, PresetLink, PresetLinkError, ProviderConfig, ProviderEntry,
    ProviderError, ProviderFailure, ProviderHealth, ProviderId, ProviderKind, ProviderPatch,
    RegistryError,
};
use aidex_router::{
    HostSummary, NetworkState, PrivacyMode, RouteDecision, RouteError, RouteRequest, TaskKind,
};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::sync::Arc;

pub const MODEL_LIST: &str = "model.list";
pub const MODEL_PROVIDERS: &str = "model.providers";
pub const MODEL_ROUTE: &str = "model.route";
pub const MODEL_USE: &str = "model.use";
pub const MODEL_DEFAULT: &str = "model.default";
pub const PROVIDER_ADD: &str = "provider.add";
pub const PROVIDER_UPDATE: &str = "provider.update";
pub const PROVIDER_PATCH: &str = "provider.patch";
pub const PROVIDER_OPEN_LINK: &str = "provider.open_link";
pub const PROVIDER_PRESETS: &str = "provider.presets";
pub const PROVIDER_REMOVE: &str = "provider.remove";
pub const PROVIDER_HEALTH: &str = "provider.health";
pub const NETWORK_OFFLINE: &str = "network.offline";
pub const NETWORK_STATUS: &str = "network.status";

/// Result of `model.list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelList {
    pub models: Vec<ListedModel>,
    /// Providers whose listing failed (offline refusal, unreachable, ...).
    pub failures: Vec<ProviderFailure>,
    pub default_model: Option<ModelRef>,
    pub network: NetworkState,
}

/// One `model.list` entry: the model's descriptor plus the image and PDF
/// input it accepts through its provider account. `input` is computed on
/// every listing and never persisted (`providers.toml` and the descriptor
/// stay readable by older daemons).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct ListedModel {
    #[serde(flatten)]
    pub descriptor: ModelDescriptor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<ModelInput>,
}

impl std::ops::Deref for ListedModel {
    type Target = ModelDescriptor;

    fn deref(&self) -> &ModelDescriptor {
        &self.descriptor
    }
}

/// Params of `model.list`; absent, `null`, `[]`, and `{}` all mean the
/// listing of every enabled provider (cached listings are reused).
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelListParams {
    /// Drop cached listings first.
    #[serde(default)]
    pub refresh: bool,
    /// List only this provider; its failure is reported in `failures`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
}

/// Result of `model.providers`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
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
    /// Account name (1-64 characters). `provider.add`: absent or `null` means
    /// none. `provider.update`: absent keeps it, `null` clears it.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<Option<String>>,
    /// The preset the account was created from; same absent/`null` rules.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub preset_id: Option<Option<String>>,
    /// `provider.add`: absent means enabled. `provider.update`: absent keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl ProviderAddParams {
    fn names_metadata(&self) -> bool {
        self.display_name.is_some() || self.preset_id.is_some() || self.enabled.is_some()
    }

    /// The account metadata after applying these params over `base`.
    fn metadata(&self, base: AccountMetadata) -> AccountMetadata {
        AccountMetadata {
            display_name: self.display_name.clone().unwrap_or(base.display_name),
            preset_id: self.preset_id.clone().unwrap_or(base.preset_id),
            enabled: self.enabled.unwrap_or(base.enabled),
        }
    }
}

/// Params of `provider.patch`: omitted fields stay as they are.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderPatchParams {
    pub id: String,
    /// `null` clears the account name.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub display_name: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Params of `provider.open_link`. There is deliberately no URL field: only
/// links stored in the built-in preset table are opened.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpenLinkParams {
    pub preset_id: String,
    pub link: PresetLink,
}

/// Deserialize a field that is present (possibly `null`) as `Some(..)`; with
/// `#[serde(default)]` an absent field stays `None`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
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

/// Params that may be omitted entirely: absent, `null`, `[]`, and `{}` give
/// the defaults.
fn optional_params<T: DeserializeOwned + Default>(params: Value) -> Result<T, RpcError> {
    match &params {
        Value::Null => Ok(T::default()),
        Value::Array(items) if items.is_empty() => Ok(T::default()),
        _ => parse_params(params),
    }
}

fn single_failure(id: ProviderId, reason: &str) -> ModelListing {
    ModelListing {
        models: Vec::new(),
        failures: vec![ProviderFailure::new(
            id,
            &ProviderError::unavailable(reason),
        )],
    }
}

/// One provider's models; an unknown, disabled, or failing provider is a
/// failure entry rather than an error, like a full listing.
fn list_one(services: &Services, id: ProviderId, refresh: bool) -> ModelListing {
    let registry = services.models().registry();
    if refresh {
        return registry.refresh_models(Some(&id));
    }
    match registry.provider_entry(&id) {
        None => {
            let reason = format!("provider `{id}` is not registered");
            single_failure(id, &reason)
        }
        Some(entry) if !entry.enabled => single_failure(id, "provider is disabled"),
        Some(_) => match registry.models_for(&id) {
            Ok(models) => ModelListing {
                models,
                failures: Vec::new(),
            },
            Err(RegistryError::Provider { error, .. }) => ModelListing {
                models: Vec::new(),
                failures: vec![ProviderFailure::new(id, &error)],
            },
            Err(error) => single_failure(id, &error.to_string()),
        },
    }
}

fn list(services: &Services, params: ModelListParams) -> Result<ModelList, RpcError> {
    let models = services.models();
    let ModelListing {
        models: list,
        failures,
    } = match params.provider_id.as_deref() {
        Some(id) => list_one(services, provider_id(id)?, params.refresh),
        None if params.refresh => models.registry().refresh_models(None),
        None => models.registry().models(),
    };
    let registry = models.registry();
    Ok(ModelList {
        models: list
            .into_iter()
            .map(|descriptor| ListedModel {
                input: Some(registry.model_input(&descriptor)),
                descriptor,
            })
            .collect(),
        failures,
        default_model: models.registry().default_model(),
        network: models.network(),
    })
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

/// `provider.add` (`update == false`) and `provider.update`. The provider
/// configuration is replaced as a whole; account metadata goes to the sidecar.
fn add(
    services: &Services,
    params: ProviderAddParams,
    update: bool,
) -> Result<ProviderEntry, RpcError> {
    let id = provider_id(&params.id)?;
    let registry = services.models().registry();
    let metadata = if !update {
        Some(params.metadata(AccountMetadata::default()))
    } else if params.names_metadata() {
        let current = registry
            .provider_entry(&id)
            .map(|entry| AccountMetadata {
                display_name: entry.display_name,
                preset_id: entry.preset_id,
                enabled: entry.enabled,
            })
            .unwrap_or_default();
        Some(params.metadata(current))
    } else {
        None
    };
    let mut config = ProviderConfig::new(id, params.kind, params.base_url);
    config.locality = params.locality;
    config.discover_models = params.discover_models;
    config.models = params.models;
    if let Some(secret_name) = params.secret_name {
        config.auth = AuthConfig::VaultSecret { secret_name };
    }
    match metadata {
        Some(metadata) if !update => registry.add_provider_with(config, metadata),
        metadata => registry.update_provider_with(config, metadata),
    }
    .map_err(registry_error)
}

fn patch(services: &Services, params: ProviderPatchParams) -> Result<ProviderEntry, RpcError> {
    let id = provider_id(&params.id)?;
    services
        .models()
        .registry()
        .patch_provider(
            &id,
            ProviderPatch {
                display_name: params.display_name,
                enabled: params.enabled,
            },
        )
        .map_err(registry_error)
}

/// Remove a configured provider. Removing a ChatGPT (OAuth) account also
/// signs it out, clearing its stored tokens; that step is best effort, so an
/// unconfirmed remote revocation or a locked vault never blocks removal.
fn remove(services: &Services, id: &ProviderId) -> Result<(), RpcError> {
    let models = services.models();
    let oauth = models
        .registry()
        .provider_config(id)
        .as_ref()
        .is_some_and(super::auth::owned);
    models
        .registry()
        .remove_provider(id)
        .map_err(registry_error)?;
    if oauth {
        let _ = models.auth().disconnect(id.as_str());
    }
    Ok(())
}

/// The URL `provider.open_link` opens: only a link stored in the built-in
/// preset table, and only an `https://` one.
pub fn open_link_url(params: &OpenLinkParams) -> Result<String, RpcError> {
    let url = aidex_providers::preset_link(&params.preset_id, params.link).map_err(|error| {
        let code = match error {
            PresetLinkError::UnknownPreset => ErrorCode::InvalidParams,
            PresetLinkError::Missing => ErrorCode::NotFound,
            PresetLinkError::Insecure => ErrorCode::InvalidArgument,
        };
        RpcError::new(code, error.to_string())
    })?;
    aidex_auth::validate_browser_url(&url).map_err(|_| {
        RpcError::new(
            ErrorCode::InvalidArgument,
            PresetLinkError::Insecure.to_string(),
        )
    })?;
    Ok(url)
}

fn open_link_error(error: AuthError) -> RpcError {
    let (code, reason, message) = match error {
        AuthError::NetworkOffline => (
            ErrorCode::PolicyDenied,
            "offline_refused",
            "Offline mode blocks opening provider links.",
        ),
        AuthError::Invalid => (
            ErrorCode::InvalidArgument,
            "invalid_link",
            "This provider preset link is not an HTTPS URL.",
        ),
        _ => (
            ErrorCode::Unavailable,
            "browser_unavailable",
            "The system browser could not be opened.",
        ),
    };
    RpcError::new(code, message).with_data(json!({ "reason": reason }))
}

/// Going offline stops network work already admitted that no per-request
/// check would catch: active runs of conversations on an external runtime (a
/// vendor process makes its own network calls), pending official runtime
/// logins, and every vendor process and official CLI command still running
/// (an account status, import or sign-out), whose launches parked for an
/// approval are released too. Native runs keep going: their remote model calls
/// pass the offline switch on each request, and runs on local models are
/// unaffected. Best effort: a run that cannot be cancelled is also stopped by
/// the runtime gate, which closes with the switch.
fn stop_external_work(services: &Services) {
    super::runtime::cancel_external_runs(services);
    super::runtime::stop_runtime_processes(services, aidex_sandbox::StopReason::Cancelled);
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
        to_result(&list(&shared, optional_params(params)?)?)
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
    let shared = Arc::clone(services);
    router.register(PROVIDER_PATCH, move |_context, params| {
        to_result(&patch(&shared, parse_params(params)?)?)
    })?;
    let shared = Arc::clone(services);
    router.register(PROVIDER_OPEN_LINK, move |_context, params| {
        let params: OpenLinkParams = parse_params(params)?;
        let url = open_link_url(&params)?;
        let offline = Arc::clone(shared.models().offline());
        aidex_auth::open_in_browser(&url, &move || !offline.is_offline())
            .map_err(open_link_error)?;
        Ok(json!({ "opened": true }))
    })?;
    router.register(PROVIDER_PRESETS, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&json!({ "presets": aidex_providers::provider_presets() }))
    })?;
    let shared = Arc::clone(services);
    router.register(PROVIDER_REMOVE, move |_context, params| {
        let params: ProviderIdParams = parse_params(params)?;
        let id = provider_id(&params.id)?;
        remove(&shared, &id)?;
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
        if params.enabled {
            // Before the gate closes, so they settle as cancelled rather than
            // failed by their vendor process ending first.
            super::runtime::cancel_external_runs(&shared);
        }
        let set = models.offline().set_offline(params.enabled);
        if set.is_ok() {
            models.observer().observe(&ModelEvent::NetworkModeChanged {
                offline: params.enabled,
            });
        }
        // Going offline is in effect even when persisting it failed.
        if params.enabled {
            stop_external_work(&shared);
        }
        set.map_err(|error| RpcError::service(error.code(), error.to_string()))?;
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

    fn add_custom(router: &Router, context: &CallContext, extra: Value) {
        let mut params = json!({
            "id": "custom",
            "base_url": "http://127.0.0.1:1/v1",
            "discover_models": false,
            "models": [{"id": "custom-a", "context_window": 8192}],
        });
        for (key, value) in extra.as_object().unwrap() {
            params[key] = value.clone();
        }
        router.call(context, PROVIDER_ADD, params).unwrap();
    }

    fn listed(router: &Router, context: &CallContext, params: Value) -> ModelList {
        serde_json::from_value(router.call(context, MODEL_LIST, params).unwrap()).unwrap()
    }

    fn model_ids(list: &ModelList) -> Vec<String> {
        list.models
            .iter()
            .map(|model| model.model.to_string())
            .collect()
    }

    fn entry(router: &Router, context: &CallContext, id: &str) -> ProviderEntry {
        let list: ProviderList =
            serde_json::from_value(router.call(context, MODEL_PROVIDERS, Value::Null).unwrap())
                .unwrap();
        list.providers
            .into_iter()
            .find(|entry| entry.descriptor.id.as_str() == id)
            .unwrap()
    }

    #[test]
    fn model_list_params_refresh_and_select_one_provider() {
        let (_root, services, router, context) = fixture();
        register_fake(&services);
        add_custom(&router, &context, json!({}));
        for params in [Value::Null, json!({}), json!([]), json!({"refresh": true})] {
            let list = listed(&router, &context, params.clone());
            assert_eq!(
                model_ids(&list),
                ["custom/custom-a", "local/coder"],
                "{params}"
            );
            assert!(list.failures.is_empty());
        }
        for refresh in [false, true] {
            let list = listed(
                &router,
                &context,
                json!({"provider_id": "custom", "refresh": refresh}),
            );
            assert_eq!(model_ids(&list), ["custom/custom-a"]);
            assert!(list.failures.is_empty());
            let list = listed(
                &router,
                &context,
                json!({"provider_id": "absent", "refresh": refresh}),
            );
            assert!(list.models.is_empty());
            assert_eq!(list.failures.len(), 1);
            assert_eq!(list.failures[0].provider_id.as_str(), "absent");
            assert_eq!(list.failures[0].kind.as_str(), "unavailable");
        }
        // Listing records a catalog status without opening a connection later.
        let custom = entry(&router, &context, "custom");
        let catalog = custom.catalog.unwrap();
        assert_eq!(catalog.model_count, 1);
        assert!(catalog.error.is_none());
        for refused in [
            json!({"provider_id": "Not An Id"}),
            json!({"providers": ["custom"]}),
            json!("custom"),
        ] {
            assert_eq!(
                router
                    .call(&context, MODEL_LIST, refused)
                    .unwrap_err()
                    .kind(),
                Some(ErrorCode::InvalidParams)
            );
        }
    }

    #[test]
    fn model_list_reports_the_image_and_pdf_input_of_each_account() {
        let (_root, services, router, context) = fixture();
        register_fake(&services);
        add_custom(
            &router,
            &context,
            json!({
                "preset_id": "openai-chat",
                "models": [
                    {"id": "vision-a", "context_window": 8192, "vision": true},
                    {"id": "text-a", "context_window": 8192},
                ],
            }),
        );
        // Another compatible server reads images but no PDFs.
        add_custom(
            &router,
            &context,
            json!({
                "id": "other",
                "preset_id": "custom",
                "models": [{"id": "vision-b", "context_window": 8192, "vision": true}],
            }),
        );
        let raw = router.call(&context, MODEL_LIST, json!({})).unwrap();
        let input = |provider: &str, model: &str| {
            raw["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["model"]["provider_id"] == provider && m["model"]["model_id"] == model)
                .map(|m| m["input"].clone())
                .unwrap_or_else(|| panic!("{provider}/{model} listed: {raw}"))
        };
        assert_eq!(
            input("custom", "vision-a"),
            json!({"images": true, "pdf": true})
        );
        assert_eq!(
            input("custom", "text-a"),
            json!({"images": false, "pdf": false})
        );
        assert_eq!(
            input("other", "vision-b"),
            json!({"images": true, "pdf": false})
        );
        assert_eq!(
            input("local", "coder"),
            json!({"images": false, "pdf": false})
        );
        let list: ModelList = serde_json::from_value(raw.clone()).unwrap();
        let vision = list
            .models
            .iter()
            .find(|m| m.model.to_string() == "custom/vision-a")
            .unwrap();
        assert!(
            vision.capabilities.vision,
            "entries deref to the descriptor"
        );
        assert_eq!(
            vision.input,
            Some(ModelInput {
                images: true,
                pdf: true
            })
        );
        assert_eq!(serde_json::to_value(&list).unwrap(), raw);
    }

    #[test]
    fn account_metadata_is_kept_in_the_sidecar_and_returned_by_model_providers() {
        let (_root, services, router, context) = fixture();
        add_custom(
            &router,
            &context,
            json!({"display_name": "Work gateway", "preset_id": "custom"}),
        );
        let added = entry(&router, &context, "custom");
        assert_eq!(added.display_name.as_deref(), Some("Work gateway"));
        assert_eq!(added.preset_id.as_deref(), Some("custom"));
        assert!(added.enabled);
        assert!(!added.discover_models);
        assert_eq!(added.declared_models[0].id.as_str(), "custom-a");
        let config = services.paths().config();
        let providers =
            std::fs::read_to_string(config.join(aidex_providers::PROVIDERS_FILE)).unwrap();
        for key in ["display_name", "preset_id", "enabled", "Work gateway"] {
            assert!(!providers.contains(key), "providers.toml holds {key}");
        }
        let accounts =
            std::fs::read_to_string(config.join(aidex_providers::PROVIDER_ACCOUNTS_FILE)).unwrap();
        assert!(accounts.contains("Work gateway"));

        // An update that names no metadata keeps it; named fields replace it.
        let update = json!({
            "id": "custom",
            "base_url": "http://127.0.0.1:2/v1",
            "discover_models": false,
            "models": [{"id": "custom-a", "context_window": 8192}],
        });
        router
            .call(&context, PROVIDER_UPDATE, update.clone())
            .unwrap();
        let kept = entry(&router, &context, "custom");
        assert_eq!(kept.display_name.as_deref(), Some("Work gateway"));
        assert_eq!(kept.preset_id.as_deref(), Some("custom"));
        let mut named = update.clone();
        named["display_name"] = Value::Null;
        named["enabled"] = json!(false);
        let updated: ProviderEntry =
            serde_json::from_value(router.call(&context, PROVIDER_UPDATE, named).unwrap()).unwrap();
        assert_eq!(updated.display_name, None);
        assert_eq!(updated.preset_id.as_deref(), Some("custom"));
        assert!(!updated.enabled);
        assert!(listed(&router, &context, Value::Null).models.is_empty());

        let mut invalid = update;
        invalid["display_name"] = json!("bad\u{7}name");
        assert_eq!(
            router
                .call(&context, PROVIDER_UPDATE, invalid)
                .unwrap_err()
                .kind(),
            Some(ErrorCode::InvalidArgument)
        );
        // A new account with an explicit `enabled: false` starts disabled.
        router
            .call(
                &context,
                PROVIDER_ADD,
                json!({"id": "spare", "base_url": "http://127.0.0.1:3/v1", "discover_models": false,
                       "models": [{"id": "m", "context_window": 4096}], "enabled": false}),
            )
            .unwrap();
        assert!(!entry(&router, &context, "spare").enabled);
    }

    #[test]
    fn provider_patch_renames_and_disables_accounts() {
        let (_root, services, router, context) = fixture();
        register_fake(&services);
        add_custom(&router, &context, json!({"preset_id": "custom"}));
        router
            .call(
                &context,
                MODEL_USE,
                json!({"provider_id": "custom", "model_id": "custom-a"}),
            )
            .unwrap();

        let renamed: ProviderEntry = serde_json::from_value(
            router
                .call(
                    &context,
                    PROVIDER_PATCH,
                    json!({"id": "custom", "display_name": "  Lab box  "}),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(renamed.display_name.as_deref(), Some("Lab box"));
        assert!(renamed.enabled);

        let disabled: ProviderEntry = serde_json::from_value(
            router
                .call(
                    &context,
                    PROVIDER_PATCH,
                    json!({"id": "custom", "enabled": false}),
                )
                .unwrap(),
        )
        .unwrap();
        assert!(!disabled.enabled);
        assert_eq!(disabled.display_name.as_deref(), Some("Lab box"));
        assert_eq!(disabled.preset_id.as_deref(), Some("custom"));
        let list = listed(&router, &context, Value::Null);
        assert_eq!(model_ids(&list), ["local/coder"]);
        assert!(list.failures.is_empty(), "disabled accounts are skipped");
        assert_eq!(list.default_model, None, "disabling clears its default");
        let one = listed(&router, &context, json!({"provider_id": "custom"}));
        assert!(one.models.is_empty());
        assert_eq!(one.failures[0].kind.as_str(), "unavailable");
        assert!(one.failures[0].message.contains("disabled"));
        let health: ProviderHealthResult = serde_json::from_value(
            router
                .call(&context, PROVIDER_HEALTH, json!({"id": "custom"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(health.health.status, aidex_providers::HealthStatus::Refused);
        // Disabled accounts stay configured and listed by `model.providers`.
        assert!(!entry(&router, &context, "custom").enabled);

        let cleared: ProviderEntry = serde_json::from_value(
            router
                .call(
                    &context,
                    PROVIDER_PATCH,
                    json!({"id": "custom", "display_name": null, "enabled": true}),
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(cleared.display_name, None);
        assert!(cleared.enabled);
        assert_eq!(
            model_ids(&listed(&router, &context, Value::Null)),
            ["custom/custom-a", "local/coder"]
        );

        for (params, code) in [
            (
                json!({"id": "local", "enabled": false}),
                ErrorCode::Conflict,
            ),
            (
                json!({"id": "absent", "enabled": false}),
                ErrorCode::NotFound,
            ),
            (
                json!({"id": "custom", "display_name": ""}),
                ErrorCode::InvalidArgument,
            ),
            (
                json!({"id": "custom", "display_name": "x".repeat(65)}),
                ErrorCode::InvalidArgument,
            ),
            (
                json!({"id": "custom", "preset_id": "openai"}),
                ErrorCode::InvalidParams,
            ),
            (json!({"id": "Not An Id"}), ErrorCode::InvalidParams),
        ] {
            assert_eq!(
                router
                    .call(&context, PROVIDER_PATCH, params.clone())
                    .unwrap_err()
                    .kind(),
                Some(code),
                "{params}"
            );
        }
    }

    #[test]
    fn disabled_accounts_are_refused_when_a_conversation_runs() {
        let (_root, services, mut router, context) = fixture();
        crate::methods::native::register(&mut router, &services).unwrap();
        services
            .vault()
            .init(
                &aidex_vault::SecretString::new("fixture passphrase only".into()),
                false,
            )
            .unwrap();
        add_custom(&router, &context, json!({}));
        let session = router
            .call(
                &context,
                "conversation.create",
                json!({"provider_id": "custom", "model_id": "custom-a", "mode": "chat"}),
            )
            .unwrap();
        router
            .call(
                &context,
                PROVIDER_PATCH,
                json!({"id": "custom", "enabled": false}),
            )
            .unwrap();
        let refused = router
            .call(
                &context,
                "message.send",
                json!({"session_id": session["id"], "content": "hello", "command_id": "send-1"}),
            )
            .unwrap_err();
        assert_eq!(refused.kind(), Some(ErrorCode::Unavailable));
        assert!(refused.message.contains("provider is unavailable"));
    }

    #[test]
    fn provider_open_link_opens_only_preset_https_links() {
        let (_root, services, router, context) = fixture();
        for preset in aidex_providers::provider_presets() {
            for link in [PresetLink::ApiKey, PresetLink::Docs] {
                let params = OpenLinkParams {
                    preset_id: preset.id.clone(),
                    link,
                };
                match preset.link(link) {
                    Some(stored) => {
                        let url = open_link_url(&params).unwrap();
                        assert_eq!(url, stored);
                        assert!(url.starts_with("https://"));
                    }
                    None => assert_eq!(
                        open_link_url(&params).unwrap_err().kind(),
                        Some(ErrorCode::NotFound)
                    ),
                }
            }
        }
        assert_eq!(
            open_link_url(&OpenLinkParams {
                preset_id: "openai".into(),
                link: PresetLink::ApiKey,
            })
            .unwrap(),
            "https://platform.openai.com/api-keys"
        );
        assert_eq!(
            open_link_url(&OpenLinkParams {
                preset_id: "ollama".into(),
                link: PresetLink::ApiKey,
            })
            .unwrap_err()
            .kind(),
            Some(ErrorCode::NotFound)
        );
        for (params, code) in [
            (
                json!({"preset_id": "no-such-preset", "link": "api_key"}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"preset_id": "openai", "link": "homepage"}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"preset_id": "openai", "link": "api_key", "url": "https://attacker.invalid/"}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"url": "https://attacker.invalid/"}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"preset_id": "ollama", "link": "api_key"}),
                ErrorCode::NotFound,
            ),
        ] {
            assert_eq!(
                router
                    .call(&context, PROVIDER_OPEN_LINK, params.clone())
                    .unwrap_err()
                    .kind(),
                Some(code),
                "{params}"
            );
        }
        // Offline mode refuses before any browser is launched.
        services.models().offline().set_offline(true).unwrap();
        let offline = router
            .call(
                &context,
                PROVIDER_OPEN_LINK,
                json!({"preset_id": "openai", "link": "api_key"}),
            )
            .unwrap_err();
        assert_eq!(offline.kind(), Some(ErrorCode::PolicyDenied));
        assert_eq!(offline.data.unwrap()["reason"], "offline_refused");
    }

    #[test]
    fn presets_carry_catalog_metadata() {
        let (_root, _services, router, context) = fixture();
        let presets = router
            .call(&context, PROVIDER_PRESETS, Value::Null)
            .unwrap();
        let presets = presets["presets"].as_array().unwrap();
        let openai = presets.iter().find(|p| p["id"] == "openai").unwrap();
        assert_eq!(openai["oauth_supported"], true);
        assert_eq!(openai["auth_methods"], json!(["oauth_chatgpt", "api_key"]));
        assert_eq!(openai["category"], "cloud");
        assert_eq!(openai["icon"], "openai");
        let ollama = presets.iter().find(|p| p["id"] == "ollama").unwrap();
        assert_eq!(ollama["category"], "local");
        assert_eq!(ollama["auth_methods"], json!(["none"]));
        for id in ["groq", "together", "huggingface", "vllm", "custom"] {
            assert!(presets.iter().any(|p| p["id"] == id), "{id}");
        }
    }

    fn put_secret(
        services: &Services,
        name: &str,
        consumer: &str,
        kind: aidex_vault::SecretKind,
        value: &[u8],
    ) {
        services
            .vault()
            .put_secret(
                &aidex_vault::SecretName::new(name).unwrap(),
                kind,
                aidex_vault::SecretBytes::new(value.to_vec()),
                aidex_vault::SecretPolicy::new([consumer.parse().unwrap()]).unwrap(),
            )
            .unwrap();
    }

    fn has_secret(services: &Services, name: &str) -> bool {
        services
            .vault()
            .secret_meta(&aidex_vault::SecretName::new(name).unwrap())
            .is_ok()
    }

    fn chatgpt_account(services: &Services, id: &str) {
        let mut config = ProviderConfig::new(
            ProviderId::new(id).unwrap(),
            ProviderKind::OpenaiResponses,
            "https://api.openai.com/v1",
        );
        config.auth = AuthConfig::VaultSecret {
            secret_name: aidex_auth::AuthService::secret_name(id),
        };
        services.models().registry().add_provider(config).unwrap();
    }

    #[test]
    fn removing_a_chatgpt_account_also_clears_its_tokens() {
        let (_root, services, router, context) = fixture();
        let passphrase = aidex_vault::SecretString::new("fixture passphrase only".into());
        services.vault().init(&passphrase, false).unwrap();
        // No sign-out request may leave the machine.
        services.models().offline().set_offline(true).unwrap();
        chatgpt_account(&services, "openai-chatgpt");
        put_secret(
            &services,
            "oauth.openai-chatgpt.profile",
            "process:aidex-auth",
            aidex_vault::SecretKind::Generic,
            br#"{"client_id":"oaiapp_fixture","subject":"fixture-subject","issuer":"https://auth.openai.com","email":"fixture@example.invalid","tokens":null,"needs_reauth":false}"#,
        );
        put_secret(
            &services,
            "oauth.openai-chatgpt.access",
            "provider:openai-chatgpt",
            aidex_vault::SecretKind::Token,
            b"fixture-access-token",
        );
        router
            .call(&context, PROVIDER_REMOVE, json!({"id": "openai-chatgpt"}))
            .unwrap();
        assert!(
            services
                .models()
                .registry()
                .provider_entry(&ProviderId::new("openai-chatgpt").unwrap())
                .is_none()
        );
        assert!(!has_secret(&services, "oauth.openai-chatgpt.access"));
        // The registration stays, as after `auth.disconnect`.
        assert!(has_secret(&services, "oauth.openai-chatgpt.profile"));

        // Best effort: an account without a profile, or a locked vault, never
        // blocks removal.
        chatgpt_account(&services, "openai-chatgpt-1");
        router
            .call(&context, PROVIDER_REMOVE, json!({"id": "openai-chatgpt-1"}))
            .unwrap();
        chatgpt_account(&services, "openai-chatgpt-2");
        services.vault().lock();
        router
            .call(&context, PROVIDER_REMOVE, json!({"id": "openai-chatgpt-2"}))
            .unwrap();
        services.vault().unlock(&passphrase).unwrap();
        assert!(services.models().registry().providers().is_empty());
    }
}
