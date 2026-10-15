use aidex_preferences::PreferencesService;
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use serde_json::json;
use std::sync::Arc;

#[test]
fn preferences_survive_restart_without_plaintext_private_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(
            dir.path(),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    let password = SecretString::new("correct test passphrase".into());
    vault.init(&password, false).unwrap();
    let prefs = PreferencesService::new(vault.clone());
    prefs.update(json!({"accent":"#abcdef","instructions":"Private user instructions, fixture only","permission_mode":"supervised"})).unwrap();
    let updated = prefs.update(json!({"reduce_motion":true})).unwrap();
    assert_eq!(updated.accent, "#abcdef");
    assert_eq!(
        updated.instructions,
        "Private user instructions, fixture only"
    );
    assert!(updated.reduce_motion);
    let mut folders = vec![dir.path().to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                assert!(
                    !String::from_utf8_lossy(&std::fs::read(path).unwrap())
                        .contains("Private user instructions")
                );
            }
        }
    }
    vault.lock();
    assert!(prefs.get().is_err());
    vault.unlock(&password).unwrap();
    let reopened = PreferencesService::new(vault);
    assert_eq!(reopened.get().unwrap().accent, "#abcdef");
}

#[test]
fn invalid_or_unknown_preference_fields_never_replace_saved_settings() {
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(
            dir.path(),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(&SecretString::new("correct test passphrase".into()), false)
        .unwrap();
    let prefs = PreferencesService::new(vault);
    prefs.update(json!({"accent":"#abcdef"})).unwrap();
    assert!(
        prefs
            .update(json!({"api_key":"never persist a credential here"}))
            .is_err()
    );
    assert!(prefs.update(json!({"max_steps":0})).is_err());
    assert!(
        prefs
            .update(json!({"accent":"url(javascript:invalid)"}))
            .is_err()
    );
    assert_eq!(prefs.get().unwrap().accent, "#abcdef");
}

fn unlocked_vault(dir: &std::path::Path) -> Arc<VaultService> {
    let vault = Arc::new(
        VaultService::open(
            dir,
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(&SecretString::new("correct test passphrase".into()), false)
        .unwrap();
    vault
}

#[test]
fn model_preferences_are_a_separate_record_with_partial_updates() {
    use aidex_preferences::{ModelPreferencesPatch, ModelPreferencesService};
    let dir = tempfile::tempdir().unwrap();
    let vault = unlocked_vault(dir.path());
    let models = ModelPreferencesService::new(vault.clone());
    assert_eq!(models.get().unwrap(), Default::default());

    let saved = models
        .set(ModelPreferencesPatch {
            favorite_models: Some(vec![
                "openai/gpt-5".into(),
                "runtime:codex/default".into(),
                "openai/gpt-5".into(),
            ]),
            hidden_models: None,
        })
        .unwrap();
    assert_eq!(
        saved.favorite_models,
        ["openai/gpt-5", "runtime:codex/default"]
    );
    assert!(saved.hidden_models.is_empty());

    let saved = models
        .set(ModelPreferencesPatch {
            favorite_models: None,
            hidden_models: Some(vec!["openai/dall-e-3".into()]),
        })
        .unwrap();
    assert_eq!(
        saved.favorite_models,
        ["openai/gpt-5", "runtime:codex/default"]
    );
    assert_eq!(saved.hidden_models, ["openai/dall-e-3"]);

    // A refused update leaves the saved record alone.
    assert!(
        models
            .set(ModelPreferencesPatch {
                favorite_models: Some(vec!["no-slash".into()]),
                hidden_models: None,
            })
            .is_err()
    );
    assert_eq!(models.get().unwrap(), saved);

    // App preferences are untouched and still readable on their own.
    let prefs = PreferencesService::new(vault.clone());
    assert_eq!(prefs.get().unwrap().accent, "#1a8cff");
    prefs.update(json!({"accent":"#abcdef"})).unwrap();
    assert_eq!(
        ModelPreferencesService::new(vault.clone()).get().unwrap(),
        saved
    );

    vault.lock();
    assert!(matches!(
        models.get(),
        Err(aidex_preferences::PreferencesError::Vault(_))
    ));
    assert!(matches!(
        models.set(ModelPreferencesPatch::default()),
        Err(aidex_preferences::PreferencesError::Vault(_))
    ));
}

#[test]
fn appearance_is_a_separate_record_with_partial_updates() {
    use aidex_preferences::{
        Appearance, AppearancePatch, AppearanceService, ChatWidth, MessageSpacing, UiDensity,
    };
    let dir = tempfile::tempdir().unwrap();
    let vault = unlocked_vault(dir.path());
    let prefs = PreferencesService::new(vault.clone());
    let appearance = AppearanceService::new(vault.clone());
    assert_eq!(appearance.get(&prefs).unwrap(), Appearance::default());

    let patch = |value| serde_json::from_value::<AppearancePatch>(value).unwrap();
    let saved = appearance
        .set(
            patch(json!({"chat_width":"wide","chat_font_size":16,"sidebar_width":320})),
            &prefs,
        )
        .unwrap();
    assert_eq!(saved.chat_width, ChatWidth::Wide);
    assert_eq!(saved.chat_font_size, 16);
    assert_eq!(saved.sidebar_width, Some(320));
    let saved = appearance
        .set(
            patch(json!({"message_spacing":"relaxed","sidebar_width":null})),
            &prefs,
        )
        .unwrap();
    assert_eq!(saved.chat_width, ChatWidth::Wide);
    assert_eq!(saved.message_spacing, MessageSpacing::Relaxed);
    assert_eq!(saved.sidebar_width, None);

    // Density follows settings.set, which every version can write.
    prefs.update(json!({"density":"compact"})).unwrap();
    assert_eq!(appearance.get(&prefs).unwrap().density, UiDensity::Compact);

    // The record survives a relock with fresh services.
    let password = SecretString::new("correct test passphrase".into());
    vault.lock();
    assert!(appearance.get(&prefs).is_err());
    vault.unlock(&password).unwrap();
    let reopened = AppearanceService::new(vault.clone())
        .get(&PreferencesService::new(vault))
        .unwrap();
    assert_eq!(
        reopened,
        Appearance {
            density: UiDensity::Compact,
            ..saved
        }
    );
}
