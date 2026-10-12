//! Listing caches, explicit refresh, single-provider listings, and the
//! catalog status the registry records for each provider.

mod support;

use std::sync::Arc;

use aidex_providers::{
    AuthConfig, FakeProvider, Locality, ModelDescriptor, ModelId, ModelProvider, ModelRef, NoVault,
    OfflineMode, OpenAiCompatibleProvider, ProviderConfig, ProviderError, ProviderErrorKind,
    ProviderId, ProviderKind, ProviderRegistry, RegistryError, SecretBytes, StaticCredentials,
};
use serde_json::json;
use support::{FakeServer, Reply};

fn fake(id: &str, models: &[&str]) -> FakeProvider {
    let provider_id = ProviderId::new(id).unwrap();
    models.iter().fold(
        FakeProvider::new(provider_id.clone(), Locality::Local),
        |fake, model| {
            fake.with_model(ModelDescriptor::new(
                ModelRef::new(provider_id.clone(), ModelId::new(*model).unwrap()),
                Locality::Local,
                8192,
            ))
        },
    )
}

fn anthropic_page(ids: &[&str]) -> Reply {
    let data: Vec<_> = ids
        .iter()
        .map(|id| json!({"type": "model", "id": id}))
        .collect();
    Reply::Json(200, json!({"data": data, "has_more": false}).to_string())
}

/// An Anthropic account on the fixture that is treated as remote, so the
/// egress gate applies.
fn remote_anthropic(server: &FakeServer, gate: Arc<OfflineMode>) -> ProviderRegistry {
    let id = ProviderId::new("claude").unwrap();
    let mut config = ProviderConfig::new(id.clone(), ProviderKind::Anthropic, server.base_url());
    config.locality = Some(Locality::Remote);
    config.auth = AuthConfig::VaultSecret {
        secret_name: "key".into(),
    };
    let keys = Arc::new(StaticCredentials::new());
    keys.insert(id, "key", SecretBytes::from("fixture-key".to_owned()));
    let registry = ProviderRegistry::in_memory(keys, gate);
    registry.add_provider(config).unwrap();
    registry
}

#[test]
fn cloud_listings_are_cached_but_still_gated_first() {
    let server = FakeServer::start(vec![anthropic_page(&["claude-sonnet-4-5"])]);
    let gate = Arc::new(OfflineMode::new(false));
    let registry = remote_anthropic(&server, gate.clone());
    let id = ProviderId::new("claude").unwrap();
    let provider = registry.provider(&id).unwrap();
    assert_eq!(provider.models().unwrap().len(), 1);
    assert_eq!(provider.models().unwrap().len(), 1);
    assert_eq!(server.requests().len(), 1, "the second listing is cached");

    gate.set_offline(true).unwrap();
    assert!(matches!(
        provider.models().unwrap_err(),
        ProviderError::OfflineRefused { .. }
    ));
    let listing = registry.models();
    assert!(listing.models.is_empty());
    assert_eq!(listing.failures[0].kind, ProviderErrorKind::OfflineRefused);
    assert_eq!(server.connections(), 1, "no connection while offline");
}

#[test]
fn invalidation_and_refresh_ask_the_provider_again() {
    let server = FakeServer::start(vec![
        anthropic_page(&["claude-sonnet-4-5"]),
        anthropic_page(&["claude-sonnet-4-5", "claude-opus-4-1"]),
        anthropic_page(&["claude-opus-4-1"]),
    ]);
    let registry = remote_anthropic(&server, Arc::new(OfflineMode::new(false)));
    let id = ProviderId::new("claude").unwrap();
    assert_eq!(registry.models().models.len(), 1);
    assert_eq!(registry.models().models.len(), 1, "cached");
    assert_eq!(server.requests().len(), 1);

    let refreshed = registry.refresh_models(Some(&id));
    assert_eq!(refreshed.models.len(), 2);
    assert_eq!(server.requests().len(), 2);

    registry.provider(&id).unwrap().invalidate_models();
    assert_eq!(registry.models().models.len(), 1);
    assert_eq!(server.requests().len(), 3);
}

#[test]
fn compatible_listing_cache_can_be_invalidated() {
    let server = FakeServer::start(vec![
        Reply::Json(200, r#"{"data":[{"id":"a"}]}"#.into()),
        Reply::Json(200, r#"{"data":[{"id":"a"},{"id":"b"}]}"#.into()),
    ]);
    let config = ProviderConfig::new(
        ProviderId::new("local").unwrap(),
        ProviderKind::OpenaiCompatible,
        server.base_url(),
    );
    let provider = OpenAiCompatibleProvider::from_config(
        &config,
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    )
    .unwrap();
    assert_eq!(provider.models().unwrap().len(), 1);
    assert_eq!(provider.models().unwrap().len(), 1);
    provider.invalidate_models();
    assert_eq!(provider.models().unwrap().len(), 2);
    assert_eq!(server.requests().len(), 2);
}

#[test]
fn single_provider_listings_and_refreshes_touch_only_that_provider() {
    let registry = ProviderRegistry::in_memory(
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    );
    let good = Arc::new(fake("good", &["m1", "m2"]));
    let other = Arc::new(fake("other", &["x"]));
    registry.register_shared(good.clone()).unwrap();
    registry.register_shared(other.clone()).unwrap();
    let good_id = ProviderId::new("good").unwrap();
    let other_id = ProviderId::new("other").unwrap();

    let listed = registry.models_for(&good_id).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(
        listed
            .iter()
            .all(|model| model.model.provider_id == good_id)
    );
    let refreshed = registry.refresh_models(Some(&good_id));
    assert_eq!(refreshed.models.len(), 2);
    assert!(refreshed.failures.is_empty());
    let entries = registry.providers();
    let status = |id: &ProviderId| {
        entries
            .iter()
            .find(|entry| &entry.descriptor.id == id)
            .unwrap()
            .catalog
            .clone()
    };
    assert_eq!(status(&good_id).unwrap().model_count, 2);
    assert_eq!(
        status(&other_id),
        None,
        "the other provider was never listed"
    );

    other.set_models_error(Some(ProviderError::unavailable("server down")));
    assert!(matches!(
        registry.models_for(&other_id),
        Err(RegistryError::Provider { .. })
    ));
    let failed = registry.provider_entry(&other_id).unwrap().catalog.unwrap();
    assert_eq!(failed.model_count, 0);
    let error = failed.error.unwrap();
    assert_eq!(error.kind, ProviderErrorKind::Unavailable);
    assert!(error.message.contains("server down"));

    let unknown = ProviderId::new("missing").unwrap();
    assert!(matches!(
        registry.models_for(&unknown),
        Err(RegistryError::NotFound(_))
    ));
    let missing = registry.refresh_models(Some(&unknown));
    assert!(missing.models.is_empty());
    assert_eq!(missing.failures[0].provider_id, unknown);

    let all = registry.refresh_models(None);
    assert_eq!(all.models.len(), 2);
    assert_eq!(all.failures.len(), 1);
    other.set_models_error(None);
    let recovered = registry.models();
    assert_eq!(recovered.models.len(), 3);
    assert!(
        registry
            .provider_entry(&other_id)
            .unwrap()
            .catalog
            .unwrap()
            .error
            .is_none()
    );
}

#[test]
fn catalog_status_records_success_and_failure_without_network_on_read() {
    let server = FakeServer::start(vec![
        Reply::Json(200, r#"{"data":[{"id":"a"},{"id":"b"}]}"#.into()),
        Reply::Json(500, r#"{"error":{"code":"server_error"}}"#.into()),
    ]);
    let registry = ProviderRegistry::in_memory(
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    );
    let id = ProviderId::new("local").unwrap();
    registry
        .add_provider(ProviderConfig::new(
            id.clone(),
            ProviderKind::OpenaiCompatible,
            server.base_url(),
        ))
        .unwrap();
    assert_eq!(registry.providers()[0].catalog, None);
    assert_eq!(
        server.requests().len(),
        0,
        "providers() opens no connection"
    );

    registry.models();
    let ok = registry.providers()[0].catalog.clone().unwrap();
    assert_eq!(ok.model_count, 2);
    assert!(ok.error.is_none());

    let failed = registry.refresh_models(None);
    assert_eq!(failed.failures.len(), 1);
    let status = registry.providers()[0].catalog.clone().unwrap();
    assert_eq!(status.model_count, 0);
    assert_eq!(status.error.unwrap().kind, ProviderErrorKind::Http);
    assert!(status.fetched_at >= ok.fetched_at);
    assert_eq!(server.requests().len(), 2);
    registry.providers();
    assert_eq!(server.requests().len(), 2);

    registry.remove_provider(&id).unwrap();
    registry
        .add_provider(ProviderConfig::new(
            id,
            ProviderKind::OpenaiCompatible,
            server.base_url(),
        ))
        .unwrap();
    assert_eq!(
        registry.providers()[0].catalog,
        None,
        "a re-added account starts without a status"
    );
}
