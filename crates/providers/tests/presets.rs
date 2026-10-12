//! Every provider preset builds a configuration the registry accepts.

use std::{collections::HashSet, sync::Arc};

use aidex_providers::{
    AccountMetadata, AuthConfig, AuthMethod, ModelConfig, ModelId, NoVault, OfflineMode,
    ProviderConfig, ProviderId, ProviderKind, ProviderRegistry, provider_presets,
};

#[test]
fn preset_ids_are_unique_provider_ids() {
    let presets = provider_presets();
    let mut seen = HashSet::new();
    for preset in &presets {
        assert!(seen.insert(preset.id.as_str()), "duplicate {}", preset.id);
        assert!(ProviderId::new(&preset.id).is_ok(), "{}", preset.id);
    }
}

#[test]
fn every_preset_builds_through_the_registry() {
    let registry = ProviderRegistry::in_memory(
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    );
    let presets = provider_presets();
    for preset in &presets {
        let base_url = preset.base_url.clone().unwrap_or_else(|| {
            // Presets without an endpoint need the user's own resource URL.
            match preset.kind {
                ProviderKind::AzureOpenai => "https://example-resource.openai.azure.com/openai",
                ProviderKind::Bedrock => "https://bedrock-runtime.us-east-1.amazonaws.com",
                _ => "https://llm.example.com/v1",
            }
            .to_owned()
        });
        let mut config =
            ProviderConfig::new(ProviderId::new(&preset.id).unwrap(), preset.kind, base_url);
        config.discover_models = preset.discover_models;
        if !preset.discover_models {
            assert!(
                matches!(
                    preset.kind,
                    ProviderKind::AzureOpenai | ProviderKind::Bedrock
                ),
                "{} must discover models",
                preset.id
            );
            config.models.push(ModelConfig::new(
                ModelId::new("declared-deployment").unwrap(),
                8192,
            ));
        }
        config.auth = if preset.auth_methods == [AuthMethod::None] {
            AuthConfig::None
        } else {
            AuthConfig::VaultSecret {
                secret_name: format!("provider-{}", preset.id),
            }
        };
        let entry = registry
            .add_provider_with(
                config,
                AccountMetadata {
                    display_name: Some(preset.display_name.clone()),
                    preset_id: Some(preset.id.clone()),
                    enabled: true,
                },
            )
            .unwrap_or_else(|error| panic!("{}: {error}", preset.id));
        assert_eq!(entry.preset_id.as_deref(), Some(preset.id.as_str()));
        assert_eq!(entry.descriptor.kind, preset.kind);
    }
    assert_eq!(registry.providers().len(), presets.len());
}
