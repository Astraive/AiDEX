//! Rollback compatibility: `update.rollback` can restart an older daemon on the
//! same drive, and it must still read and write the records this version saved.
//! `AppPreferencesV1` is a frozen copy of the shape such a daemon accepts.
use aidex_preferences::{
    APPEARANCE_SECRET, AppPreferences, AppearancePatch, AppearanceService, Density,
    ModelPreferences, ModelPreferencesService, PREFERENCES_SECRET, PreferencesError,
    PreferencesService, UiDensity,
};
use aidex_vault::{
    ConsumerScope, KdfParams, SecretName, SecretString, VaultService, VaultServiceConfig,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

const FIXTURE: &str = include_str!("fixtures/app-preferences-v1.json");
const ROLLBACK: &str = "AppPreferences gained a field or variant; an older daemon refuses it \
                        after update.rollback - use a separate record";
const V1_SHORTCUT_ACTIONS: [&str; 4] = ["new_session", "search", "toggle_sidebar", "providers"];

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ThemeV1 {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum DensityV1 {
    #[default]
    Comfortable,
    Compact,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PermissionModeV1 {
    Supervised,
    AutoAcceptEdits,
    Auto,
    FullAccess,
}

/// `AppPreferences` as the daemon before the appearance record reads it.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[allow(dead_code)] // The fields exist to be parsed.
struct AppPreferencesV1 {
    accent: String,
    theme: ThemeV1,
    density: DensityV1,
    reduce_motion: bool,
    keyboard_hints: bool,
    model_id: String,
    workspace_id: String,
    permission_mode: Option<PermissionModeV1>,
    instructions: String,
    fallback_model: Option<String>,
    max_steps: u32,
    max_tokens: u32,
    shortcuts: BTreeMap<String, String>,
}

/// Parse `bytes` the way an older daemon does, including its shortcut whitelist.
fn parse_v1(bytes: &[u8]) -> AppPreferencesV1 {
    let v1: AppPreferencesV1 = serde_json::from_slice(bytes).expect(ROLLBACK);
    assert!(
        v1.shortcuts
            .keys()
            .all(|action| V1_SHORTCUT_ACTIONS.contains(&action.as_str())),
        "{ROLLBACK}"
    );
    v1
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

/// The saved bytes of a preference record, read as its consumer.
fn raw(vault: &VaultService, name: &str) -> Vec<u8> {
    let name = SecretName::new(name).unwrap();
    let consumer: ConsumerScope = "process:aidex-preferences".parse().unwrap();
    let handle = vault
        .issue_handle(&name, &consumer, Duration::from_secs(10))
        .unwrap();
    let bytes = vault.resolve(&handle, &consumer).unwrap().expose().to_vec();
    vault.revoke_handle(&handle.handle_id);
    bytes
}

fn patch(value: Value) -> AppearancePatch {
    serde_json::from_value(value).unwrap()
}

#[test]
fn app_preferences_round_trip_in_the_v1_shape() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    assert_eq!(fixture.as_object().unwrap().len(), 13);
    let parsed: AppPreferences = serde_json::from_value(fixture.clone()).expect(ROLLBACK);
    assert_eq!(
        serde_json::to_value(&parsed).unwrap(),
        fixture,
        "{ROLLBACK}"
    );
    let v1 = parse_v1(FIXTURE.as_bytes());
    assert_eq!(v1.theme, ThemeV1::Light);
    assert_eq!(v1.density, DensityV1::Compact);
    assert_eq!(v1.permission_mode, Some(PermissionModeV1::AutoAcceptEdits));
    parse_v1(&serde_json::to_vec(&AppPreferences::default()).unwrap());
}

#[test]
fn v1_reader_accepts_mirrored_densities_only() {
    for density in [
        UiDensity::Compact,
        UiDensity::Comfortable,
        UiDensity::Spacious,
    ] {
        let record = json!({
            "accent": "#1a8cff",
            "density": aidex_preferences::legacy_density(density),
        });
        parse_v1(&serde_json::to_vec(&record).unwrap());
    }
    for refused in [
        json!({"density": "spacious"}),
        json!({"ui_scale": 100}),
        json!({"skill_ids": []}),
        json!({"runtime_id": null}),
    ] {
        assert!(
            serde_json::from_value::<AppPreferencesV1>(refused.clone()).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn every_record_the_services_write_parses_as_v1() {
    let dir = tempfile::tempdir().unwrap();
    let vault = unlocked_vault(dir.path());
    let prefs = PreferencesService::new(vault.clone());
    let appearance = AppearanceService::new(vault.clone());
    for change in [
        json!({"theme": "light"}),
        json!({"theme": "system"}),
        json!({"density": "compact"}),
        json!({"density": "comfortable"}),
        json!({"permission_mode": "supervised"}),
        json!({"permission_mode": "auto_accept_edits"}),
        json!({"permission_mode": "auto"}),
        json!({"permission_mode": "full_access"}),
        json!({"permission_mode": null}),
        json!({"fallback_model": "openai/gpt-5-mini"}),
        json!({"fallback_model": null}),
        json!({"shortcuts": {"search": "Mod+f", "toggle_sidebar": "Mod+b"}}),
    ] {
        prefs.update(change).unwrap();
        parse_v1(&raw(&vault, PREFERENCES_SECRET));
    }
    for density in [
        UiDensity::Spacious,
        UiDensity::Compact,
        UiDensity::Comfortable,
        UiDensity::Spacious,
        UiDensity::Compact,
    ] {
        let saved = appearance
            .set(
                AppearancePatch {
                    density: Some(density),
                    ..Default::default()
                },
                &prefs,
            )
            .unwrap();
        assert_eq!(saved.density, density);
        let expected = match density {
            UiDensity::Compact => DensityV1::Compact,
            UiDensity::Comfortable | UiDensity::Spacious => DensityV1::Comfortable,
        };
        assert_eq!(parse_v1(&raw(&vault, PREFERENCES_SECRET)).density, expected);
    }
}

#[test]
fn an_older_daemon_reads_its_records_after_appearance_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let vault = unlocked_vault(dir.path());
    let prefs = PreferencesService::new(vault.clone());
    let appearance = AppearanceService::new(vault.clone());
    prefs
        .update(json!({"density": "compact", "accent": "#abcdef"}))
        .unwrap();
    let saved = appearance
        .set(
            patch(json!({"density": "spacious", "ui_scale": 110})),
            &prefs,
        )
        .unwrap();
    assert_eq!(saved.density, UiDensity::Spacious);
    assert_eq!(saved.ui_scale, 110);

    // What an older daemon runs: its own services over the same vault.
    let older = PreferencesService::new(vault.clone());
    let read = older.get().expect(ROLLBACK);
    assert_eq!(read.density, Density::Comfortable);
    assert_eq!(read.accent, "#abcdef");
    let bytes = raw(&vault, PREFERENCES_SECRET);
    assert_eq!(parse_v1(&bytes).density, DensityV1::Comfortable);
    assert!(!String::from_utf8(bytes).unwrap().contains("ui_scale"));
    assert_eq!(
        ModelPreferencesService::new(vault.clone()).get().unwrap(),
        ModelPreferences::default()
    );
    let mut names: Vec<String> = vault
        .list_secrets()
        .unwrap()
        .iter()
        .map(|meta| meta.name.to_string())
        .collect();
    names.sort();
    assert_eq!(names, [APPEARANCE_SECRET, PREFERENCES_SECRET]);

    // The older app switches to compact: compact wins after the update.
    older.update(json!({"density": "compact"})).unwrap();
    assert_eq!(appearance.get(&prefs).unwrap().density, UiDensity::Compact);
    // Back to comfortable there: the spacious refinement returns (documented).
    older.update(json!({"density": "comfortable"})).unwrap();
    assert_eq!(appearance.get(&prefs).unwrap().density, UiDensity::Spacious);
}

#[test]
fn appearance_needs_the_vault() {
    let dir = tempfile::tempdir().unwrap();
    let vault = unlocked_vault(dir.path());
    let prefs = PreferencesService::new(vault.clone());
    let appearance = AppearanceService::new(vault.clone());
    appearance
        .set(patch(json!({"ui_scale": 110})), &prefs)
        .unwrap();
    vault.lock();
    assert!(matches!(
        appearance.get(&prefs),
        Err(PreferencesError::Vault(_))
    ));
    for change in [
        json!({}),
        json!({"ui_scale": 90}),
        json!({"density": "compact"}),
    ] {
        assert!(
            matches!(
                appearance.set(patch(change.clone()), &prefs),
                Err(PreferencesError::Vault(_))
            ),
            "{change}"
        );
    }
}

#[test]
fn refused_patch_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let vault = unlocked_vault(dir.path());
    let prefs = PreferencesService::new(vault.clone());
    let appearance = AppearanceService::new(vault.clone());
    let saved = appearance
        .set(
            patch(json!({"ui_scale": 110, "density": "compact", "sidebar_width": 300})),
            &prefs,
        )
        .unwrap();
    let before = raw(&vault, APPEARANCE_SECRET);
    for refused in [
        json!({"ui_scale": 200, "density": "spacious"}),
        json!({"chat_font_size": 11, "density": "comfortable"}),
        json!({"chat_font_size": 21}),
        json!({"code_font_size": 9, "chat_width": "wide"}),
        json!({"sidebar_width": 100}),
    ] {
        assert!(
            matches!(
                appearance.set(patch(refused.clone()), &prefs),
                Err(PreferencesError::Invalid(_))
            ),
            "{refused}"
        );
    }
    // Patches that do not parse never reach the service.
    for unparsable in [
        json!({"zoom": 1}),
        json!({"ui_scale": "big"}),
        json!({"density": "dense"}),
    ] {
        assert!(serde_json::from_value::<AppearancePatch>(unparsable.clone()).is_err());
    }
    assert_eq!(appearance.get(&prefs).unwrap(), saved);
    assert_eq!(raw(&vault, APPEARANCE_SECRET), before);
    assert_eq!(prefs.get().unwrap().density, Density::Compact);
}
