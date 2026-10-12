//! Model-provider egress and credentials pass the broker's policy.

mod common;

use std::sync::Arc;

use aidex_broker::{ModelEgress, Principal};
use aidex_policy::Profile;
use aidex_providers::{
    CredentialError, CredentialResolver, EgressGate, OfflineMode, ProviderError, ProviderId,
};
use aidex_vault::{
    ConsumerScope, KdfParams, SecretBytes, SecretKind, SecretName, SecretPolicy, SecretString,
    VaultService, VaultServiceConfig,
};
use common::{Fixture, Options};

fn provider() -> ProviderId {
    ProviderId::new("openai").unwrap()
}

fn egress(fixture: &Fixture) -> ModelEgress {
    ModelEgress::new(Arc::clone(&fixture.broker)).unwrap()
}

#[test]
fn remote_model_calls_follow_the_active_profile() {
    let balanced = Fixture::new(Options::default());
    egress(&balanced)
        .check_remote(&provider(), "api.openai.com")
        .unwrap();
    assert!(balanced.names().contains(&"policy.decision"));

    for profile in [Profile::Safe, Profile::OfflineSecure] {
        let strict = Fixture::new(Options::profile(profile));
        let error = egress(&strict)
            .check_remote(&provider(), "api.openai.com")
            .unwrap_err();
        assert!(
            matches!(error, ProviderError::PolicyDenied { .. }),
            "{profile}: {error:?}"
        );
    }

    let offline = egress(&balanced).with_offline_switch(Arc::new(OfflineMode::new(true)));
    assert!(matches!(
        offline.check_remote(&provider(), "api.openai.com"),
        Err(ProviderError::OfflineRefused { .. })
    ));

    balanced
        .broker
        .emergency_stop("drill", &Principal::LocalUser)
        .unwrap();
    assert!(matches!(
        egress(&balanced).check_remote(&provider(), "api.openai.com"),
        Err(ProviderError::PolicyDenied { .. })
    ));
}

const DENY_SECRETS: &str = r#"
schema_version = 1
scope = "global"

[[rules]]
id = "no-provider-secrets"
action = "secret.use"
verdict = "deny"
"#;

#[test]
fn provider_credentials_are_audited_handles_and_policy_can_deny_them() {
    let vault_dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(
            vault_dir.path(),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(
            &SecretString::new("correct horse battery staple".into()),
            false,
        )
        .unwrap();
    let value = format!(
        "value{}",
        aidex_core::EntityId::generate("v").unwrap().as_str()
    );
    vault
        .put_secret(
            &SecretName::new("openai-key").unwrap(),
            SecretKind::ApiKey,
            SecretBytes::from(value.clone()),
            SecretPolicy::new([ConsumerScope::provider("openai").unwrap()]).unwrap(),
        )
        .unwrap();

    let fixture = Fixture::new(Options {
        vault: Some(vault.clone()),
        ..Options::default()
    });
    let secret = egress(&fixture).resolve(&provider(), "openai-key").unwrap();
    assert_eq!(secret.expose(), value.as_bytes());
    let names = fixture.names();
    assert!(
        names.contains(&"secret.granted") && names.contains(&"secret.revoked"),
        "{names:?}"
    );
    assert!(!fixture.event_text().contains(&value));
    // The secret's policy names only `provider:openai`.
    assert!(matches!(
        egress(&fixture).resolve(&ProviderId::new("other").unwrap(), "openai-key"),
        Err(CredentialError::Denied { .. })
    ));

    let denied = Fixture::new(Options {
        vault: Some(vault),
        ..Options::default().document("10-deny.toml", DENY_SECRETS)
    });
    assert!(matches!(
        egress(&denied).resolve(&provider(), "openai-key"),
        Err(CredentialError::Denied { .. })
    ));
    assert!(!denied.names().contains(&"secret.granted"));

    let without_vault = Fixture::new(Options::default());
    assert!(matches!(
        egress(&without_vault).resolve(&provider(), "openai-key"),
        Err(CredentialError::Unavailable { .. })
    ));
}
