//! Account metadata in `provider-accounts.toml`, rollback-safe
//! `providers.toml`, and disabled-provider semantics.

mod support;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use aidex_providers::{
    AccountMetadata, AuthConfig, ConfigError, FakeProvider, HealthStatus, Locality, ModelConfig,
    ModelDescriptor, ModelEvent, ModelId, ModelRef, NoVault, OfflineMode, PROVIDER_ACCOUNTS_FILE,
    PROVIDERS_FILE, ProviderAccountsConfig, ProviderConfig, ProviderId, ProviderKind,
    ProviderOrigin, ProviderPatch, ProviderRegistry, ProvidersConfig, RecordingObserver,
    RegistryError,
};
use support::{FakeServer, Reply, temp_drive};

fn load(paths: &aidex_drive::DrivePaths) -> Result<ProviderRegistry, RegistryError> {
    ProviderRegistry::load(
        paths,
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    )
}

/// A key-less local account declaring `model` (no discovery, no network).
fn declared(id: &str, model: &str) -> ProviderConfig {
    let mut config = ProviderConfig::new(
        ProviderId::new(id).unwrap(),
        ProviderKind::OpenaiCompatible,
        "http://127.0.0.1:9/v1",
    );
    config.discover_models = false;
    config
        .models
        .push(ModelConfig::new(ModelId::new(model).unwrap(), 8192));
    config
}

fn named(display_name: &str, preset_id: &str) -> AccountMetadata {
    AccountMetadata {
        display_name: Some(display_name.into()),
        preset_id: Some(preset_id.into()),
        enabled: true,
    }
}

#[test]
fn metadata_lives_in_the_sidecar_and_providers_toml_keeps_its_schema() {
    let (_root, paths) = temp_drive();
    let registry = load(&paths).unwrap();
    let mut keyed = ProviderConfig::new(
        ProviderId::new("anthropic").unwrap(),
        ProviderKind::Anthropic,
        "https://api.anthropic.com/v1",
    );
    keyed.auth = AuthConfig::VaultSecret {
        secret_name: "provider-anthropic".into(),
    };
    let entry = registry
        .add_provider_with(keyed.clone(), named("  Work Claude  ", "anthropic"))
        .unwrap();
    assert_eq!(entry.display_name.as_deref(), Some("Work Claude"));
    assert_eq!(entry.preset_id.as_deref(), Some("anthropic"));
    assert!(entry.enabled && entry.discover_models);
    assert_eq!(entry.origin, ProviderOrigin::Configured);
    let local = declared("ollama", "llama3.1:8b");
    registry
        .add_provider_with(
            local.clone(),
            AccountMetadata {
                enabled: false,
                ..named("Home", "ollama")
            },
        )
        .unwrap();

    // providers.toml is exactly what the unchanged v1 schema writes.
    let providers_path = paths.config().join(PROVIDERS_FILE);
    let text = std::fs::read_to_string(&providers_path).unwrap();
    for key in ["display_name", "preset_id", "enabled", "Work Claude"] {
        assert!(!text.contains(key), "{key} leaked into providers.toml");
    }
    let parsed = ProvidersConfig::load(&providers_path).unwrap();
    assert_eq!(parsed.providers, [keyed, local.clone()]);
    let expected = ProvidersConfig {
        version: 1,
        default_model: None,
        providers: vec![parsed.providers[0].clone(), local],
    };
    assert_eq!(text, toml::to_string(&expected).unwrap());

    let sidecar =
        ProviderAccountsConfig::load(&paths.config().join(PROVIDER_ACCOUNTS_FILE)).unwrap();
    assert_eq!(sidecar.accounts.len(), 2);

    let reloaded = load(&paths).unwrap();
    let entries = reloaded.providers();
    assert_eq!(entries[0].display_name.as_deref(), Some("Work Claude"));
    assert_eq!(entries[1].display_name.as_deref(), Some("Home"));
    assert!(!entries[1].enabled);
    assert_eq!(entries[1].declared_models.len(), 1);
    assert!(!entries[1].discover_models);

    reloaded
        .remove_provider(&ProviderId::new("ollama").unwrap())
        .unwrap();
    let sidecar =
        ProviderAccountsConfig::load(&paths.config().join(PROVIDER_ACCOUNTS_FILE)).unwrap();
    let ids: Vec<_> = sidecar.accounts.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["anthropic"], "remove drops the account's metadata");
}

#[test]
fn registries_without_a_sidecar_load_and_ignore_unknown_accounts() {
    let (_root, paths) = temp_drive();
    std::fs::create_dir_all(paths.config()).unwrap();
    // A providers.toml written by an older daemon, without any sidecar.
    let config = ProvidersConfig {
        version: 1,
        default_model: Some("local/tiny".into()),
        providers: vec![declared("local", "tiny")],
    };
    std::fs::write(
        paths.config().join(PROVIDERS_FILE),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    let registry = load(&paths).unwrap();
    let entry = &registry.providers()[0];
    assert!(entry.enabled);
    assert_eq!(entry.display_name, None);
    assert_eq!(entry.preset_id, None);
    assert!(
        !paths.config().join(PROVIDER_ACCOUNTS_FILE).exists(),
        "loading never creates the sidecar"
    );

    // Entries for ids that are not configured are ignored, then dropped on
    // the next write.
    let sidecar = paths.config().join(PROVIDER_ACCOUNTS_FILE);
    std::fs::write(
        &sidecar,
        "version = 1\n[[account]]\nid = \"gone\"\nenabled = false\n[[account]]\nid = \"local\"\ndisplay_name = \"Tiny box\"\npreset_id = \"from-a-newer-build\"\n",
    )
    .unwrap();
    let registry = load(&paths).unwrap();
    let entry = &registry.providers()[0];
    assert_eq!(entry.display_name.as_deref(), Some("Tiny box"));
    assert_eq!(entry.preset_id.as_deref(), Some("from-a-newer-build"));
    registry
        .patch_provider(
            &ProviderId::new("local").unwrap(),
            ProviderPatch {
                display_name: Some(None),
                enabled: None,
            },
        )
        .unwrap();
    let written = std::fs::read_to_string(&sidecar).unwrap();
    assert!(!written.contains("gone"), "{written}");
    assert!(!written.contains("Tiny box"), "{written}");
    assert!(written.contains("from-a-newer-build"), "{written}");

    std::fs::write(&sidecar, "version = 2\n").unwrap();
    assert!(matches!(
        load(&paths),
        Err(RegistryError::Config(ConfigError::UnsupportedVersion {
            found: 2,
            ..
        }))
    ));
    std::fs::write(
        &sidecar,
        "version = 1\n[[account]]\nid = \"local\"\nsecret = \"x\"\n",
    )
    .unwrap();
    assert!(matches!(
        load(&paths),
        Err(RegistryError::Config(ConfigError::Invalid { .. }))
    ));
}

#[test]
fn updates_keep_metadata_unless_new_metadata_is_given() {
    let (_root, paths) = temp_drive();
    let registry = load(&paths).unwrap();
    let id = ProviderId::new("local").unwrap();
    registry
        .add_provider_with(declared("local", "tiny"), named("Box", "llamacpp"))
        .unwrap();
    registry
        .update_provider(declared("local", "small"))
        .unwrap();
    let entry = registry.provider_entry(&id).unwrap();
    assert_eq!(entry.display_name.as_deref(), Some("Box"));
    assert_eq!(entry.declared_models[0].id.as_str(), "small");
    let entry = registry
        .update_provider_with(declared("local", "small"), Some(AccountMetadata::default()))
        .unwrap();
    assert_eq!(entry.display_name, None);
    assert_eq!(entry.preset_id, None);
    assert!(matches!(
        registry.add_provider_with(
            declared("other", "m"),
            AccountMetadata {
                display_name: Some("\u{7}bell".into()),
                ..AccountMetadata::default()
            }
        ),
        Err(RegistryError::Invalid(_))
    ));
    assert!(matches!(
        registry.add_provider_with(
            declared("other", "m"),
            AccountMetadata {
                preset_id: Some("Not A Preset".into()),
                ..AccountMetadata::default()
            }
        ),
        Err(RegistryError::Invalid(_))
    ));
    assert!(
        registry
            .provider_config(&ProviderId::new("other").unwrap())
            .is_none()
    );
}

#[test]
fn disabled_providers_stay_configured_but_are_not_listed_or_used() {
    let (_root, paths) = temp_drive();
    let observer = Arc::new(RecordingObserver::new());
    let registry = load(&paths).unwrap().with_observer(observer.clone());
    let id = ProviderId::new("local").unwrap();
    registry.add_provider(declared("local", "tiny")).unwrap();
    registry
        .register(Box::new(
            FakeProvider::new(ProviderId::new("runtime").unwrap(), Locality::Local).with_model(
                ModelDescriptor::new(ModelRef::parse("runtime/r").unwrap(), Locality::Local, 4096),
            ),
        ))
        .unwrap();
    let tiny = ModelRef::parse("local/tiny").unwrap();
    registry.set_default_model(tiny.clone()).unwrap();

    let entry = registry
        .patch_provider(
            &id,
            ProviderPatch {
                display_name: Some(Some("Old box".into())),
                enabled: Some(false),
            },
        )
        .unwrap();
    assert!(!entry.enabled);
    assert_eq!(entry.display_name.as_deref(), Some("Old box"));
    assert_eq!(
        registry.default_model(),
        None,
        "disabling clears the default"
    );
    let events: Vec<_> = observer.events();
    assert_eq!(
        events.last(),
        Some(&ModelEvent::DefaultChanged { model: None })
    );

    assert_eq!(registry.providers().len(), 2, "still configured");
    assert!(registry.provider(&id).is_none());
    assert!(registry.provider_config(&id).is_some());
    let listing = registry.models();
    let names: Vec<_> = listing.models.iter().map(|m| m.model.to_string()).collect();
    assert_eq!(names, ["runtime/r"]);
    assert!(listing.failures.is_empty(), "disabled is not a failure");
    assert!(matches!(
        registry.set_default_model(tiny.clone()),
        Err(RegistryError::NotFound(_))
    ));
    assert!(matches!(
        registry.models_for(&id),
        Err(RegistryError::NotFound(_))
    ));
    let refreshed = registry.refresh_models(Some(&id));
    assert!(refreshed.failures[0].message.contains("disabled"));
    let health = registry.health(&id).unwrap();
    assert_eq!(health.status, HealthStatus::Refused);
    assert_eq!(health.message.as_deref(), Some("provider is disabled"));

    // The flag persists and re-enabling restores everything.
    let reloaded = load(&paths).unwrap();
    assert!(!reloaded.providers()[0].enabled);
    assert!(reloaded.provider(&id).is_none());
    let entry = reloaded
        .patch_provider(
            &id,
            ProviderPatch {
                display_name: None,
                enabled: Some(true),
            },
        )
        .unwrap();
    assert!(entry.enabled);
    assert_eq!(entry.display_name.as_deref(), Some("Old box"));
    assert!(reloaded.provider(&id).is_some());
    reloaded.set_default_model(tiny).unwrap();

    assert!(matches!(
        registry.patch_provider(
            &ProviderId::new("runtime").unwrap(),
            ProviderPatch::default()
        ),
        Err(RegistryError::NotConfigured(_))
    ));
    assert!(matches!(
        registry.patch_provider(
            &ProviderId::new("missing").unwrap(),
            ProviderPatch::default()
        ),
        Err(RegistryError::NotFound(_))
    ));
    assert!(matches!(
        registry.patch_provider(
            &id,
            ProviderPatch {
                display_name: Some(Some("   ".into())),
                enabled: None
            }
        ),
        Err(RegistryError::Invalid(_))
    ));
}

#[test]
fn updating_a_default_holder_to_disabled_clears_the_default() {
    let registry = ProviderRegistry::in_memory(
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    );
    registry.add_provider(declared("local", "tiny")).unwrap();
    registry
        .set_default_model(ModelRef::parse("local/tiny").unwrap())
        .unwrap();
    let entry = registry
        .update_provider_with(
            declared("local", "tiny"),
            Some(AccountMetadata {
                enabled: false,
                ..AccountMetadata::default()
            }),
        )
        .unwrap();
    assert!(!entry.enabled);
    assert_eq!(registry.default_model(), None);
}

#[test]
fn a_provider_disabled_while_its_default_is_checked_never_becomes_the_default() {
    let release = Arc::new(AtomicBool::new(false));
    let server = FakeServer::start(vec![Reply::Held(
        release.clone(),
        Box::new(Reply::Json(200, r#"{"data":[{"id":"tiny"}]}"#.into())),
    )]);
    let observer = Arc::new(RecordingObserver::new());
    let registry = Arc::new(
        ProviderRegistry::in_memory(
            Arc::new(NoVault::default()),
            Arc::new(OfflineMode::new(false)),
        )
        .with_observer(observer.clone()),
    );
    let id = ProviderId::new("local").unwrap();
    registry
        .add_provider(ProviderConfig::new(
            id.clone(),
            ProviderKind::OpenaiCompatible,
            server.base_url(),
        ))
        .unwrap();

    let setter = {
        let registry = Arc::clone(&registry);
        std::thread::spawn(move || {
            registry.set_default_model(ModelRef::parse("local/tiny").unwrap())
        })
    };
    // The listing check is in flight; disable the provider before it answers.
    server.wait_for_requests(1);
    registry
        .patch_provider(
            &id,
            ProviderPatch {
                display_name: None,
                enabled: Some(false),
            },
        )
        .unwrap();
    release.store(true, Ordering::SeqCst);

    assert!(matches!(
        setter.join().unwrap(),
        Err(RegistryError::NotFound(_))
    ));
    assert_eq!(registry.default_model(), None);
    assert!(
        !observer
            .events()
            .iter()
            .any(|event| matches!(event, ModelEvent::DefaultChanged { model: Some(_) }))
    );
}
