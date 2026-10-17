//! Appearance: interface scale, density, chat and code text sizes, chat width,
//! sidebar width and collapse, and message spacing.
//!
//! The record lives in its own vault secret, apart from [`AppPreferences`], so a
//! daemon that predates it still reads `aidex-preferences` after an update
//! rollback. Unlike the other records it is versioned in both directions:
//!
//! - A patch is closed: [`AppearancePatch`] refuses unknown fields, unknown
//!   values, and numbers outside their range.
//! - The saved record is read leniently: numbers are clamped, unknown values and
//!   wrong types read as their defaults, unreadable bytes read as all defaults,
//!   and fields a newer version wrote are kept (at most 32 of them in 4 KiB), so
//!   a later version can add options without yet another record.
//!
//! Density is split across two records. `AppPreferences.density` (compact or
//! comfortable) stays the base that every version reads and writes; this record
//! only adds whether comfortable is refined to spacious.
//!
//! [`AppPreferences`]: crate::AppPreferences
use crate::{Density, PreferencesError, PreferencesService, record};
use aidex_vault::VaultService;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex};

/// Vault secret holding the appearance record.
pub const APPEARANCE_SECRET: &str = "aidex-appearance";
/// Smallest interface scale, in percent.
pub const MIN_UI_SCALE: u16 = 75;
/// Largest interface scale, in percent.
pub const MAX_UI_SCALE: u16 = 125;
/// Interface scale when none was saved, in percent.
pub const DEFAULT_UI_SCALE: u16 = 100;
/// Smallest chat text size, in CSS px.
pub const MIN_CHAT_FONT_SIZE: u8 = 12;
/// Largest chat text size, in CSS px.
pub const MAX_CHAT_FONT_SIZE: u8 = 20;
/// Chat text size when none was saved, in CSS px.
pub const DEFAULT_CHAT_FONT_SIZE: u8 = 14;
/// Smallest code text size, in CSS px.
pub const MIN_CODE_FONT_SIZE: u8 = 10;
/// Largest code text size, in CSS px.
pub const MAX_CODE_FONT_SIZE: u8 = 18;
/// Code text size when none was saved, in CSS px.
pub const DEFAULT_CODE_FONT_SIZE: u8 = 13;
/// Narrowest saved sidebar, in CSS px.
pub const MIN_SIDEBAR_WIDTH: u16 = 208;
/// Widest saved sidebar, in CSS px.
pub const MAX_SIDEBAR_WIDTH: u16 = 480;
/// Most fields a newer version wrote that a read keeps.
const MAX_EXTRA_FIELDS: usize = 32;
/// Largest serialized size of the kept fields, in bytes.
const MAX_EXTRA_BYTES: usize = 4096;

/// Spacing and control sizes. `Spacious` exists only in this record; the
/// application preferences keep compact or comfortable.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiDensity {
    Compact,
    #[default]
    Comfortable,
    Spacious,
}

/// Widest the chat transcript and composer grow.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatWidth {
    Narrow,
    #[default]
    Comfortable,
    Wide,
    Full,
}

/// Space between messages and between their paragraphs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageSpacing {
    Compact,
    #[default]
    Comfortable,
    Relaxed,
}

/// `appearance.get|set` result, with the density already resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    /// Whole-window zoom in percent, 75-125.
    pub ui_scale: u16,
    pub density: UiDensity,
    pub chat_width: ChatWidth,
    /// Chat message text, CSS px 12-20.
    pub chat_font_size: u8,
    /// Code, diff, and terminal text, CSS px 10-18.
    pub code_font_size: u8,
    /// CSS px 208-480, or `None` for the shell default.
    pub sidebar_width: Option<u16>,
    /// Whether the sidebar becomes an overlay in narrow windows.
    pub sidebar_auto_collapse: bool,
    pub message_spacing: MessageSpacing,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            ui_scale: DEFAULT_UI_SCALE,
            density: UiDensity::Comfortable,
            chat_width: ChatWidth::Comfortable,
            chat_font_size: DEFAULT_CHAT_FONT_SIZE,
            code_font_size: DEFAULT_CODE_FONT_SIZE,
            sidebar_width: None,
            sidebar_auto_collapse: true,
            message_spacing: MessageSpacing::Comfortable,
        }
    }
}

/// `appearance.set` params: an omitted field stays as saved, `null` resets
/// `sidebar_width` to the shell default, and unknown fields are refused.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppearancePatch {
    #[serde(default)]
    pub ui_scale: Option<u16>,
    #[serde(default)]
    pub density: Option<UiDensity>,
    #[serde(default)]
    pub chat_width: Option<ChatWidth>,
    #[serde(default)]
    pub chat_font_size: Option<u8>,
    #[serde(default)]
    pub code_font_size: Option<u8>,
    #[serde(default, deserialize_with = "nullable")]
    pub sidebar_width: Option<Option<u16>>,
    #[serde(default)]
    pub sidebar_auto_collapse: Option<bool>,
    #[serde(default)]
    pub message_spacing: Option<MessageSpacing>,
}

/// Distinguish an absent field (`None`) from an explicit `null` (`Some(None)`).
fn nullable<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<u16>>, D::Error> {
    Option::<u16>::deserialize(deserializer).map(Some)
}

impl AppearancePatch {
    /// Why the patch is refused, if a value is outside its range.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self
            .ui_scale
            .is_some_and(|value| !(MIN_UI_SCALE..=MAX_UI_SCALE).contains(&value))
        {
            return Err("ui_scale must be 75-125");
        }
        if self
            .chat_font_size
            .is_some_and(|value| !(MIN_CHAT_FONT_SIZE..=MAX_CHAT_FONT_SIZE).contains(&value))
        {
            return Err("chat_font_size must be 12-20");
        }
        if self
            .code_font_size
            .is_some_and(|value| !(MIN_CODE_FONT_SIZE..=MAX_CODE_FONT_SIZE).contains(&value))
        {
            return Err("code_font_size must be 10-18");
        }
        if self
            .sidebar_width
            .flatten()
            .is_some_and(|value| !(MIN_SIDEBAR_WIDTH..=MAX_SIDEBAR_WIDTH).contains(&value))
        {
            return Err("sidebar_width must be 208-480 or null");
        }
        Ok(())
    }

    /// Apply the patch to the saved record. Returns the base density to keep in
    /// the application preferences when the patch sets a density.
    fn apply(&self, stored: &mut Stored) -> Option<Density> {
        let saved = &mut stored.appearance;
        if let Some(value) = self.ui_scale {
            saved.ui_scale = value;
        }
        if let Some(value) = self.chat_width {
            saved.chat_width = value;
        }
        if let Some(value) = self.chat_font_size {
            saved.chat_font_size = value;
        }
        if let Some(value) = self.code_font_size {
            saved.code_font_size = value;
        }
        if let Some(value) = self.sidebar_width {
            saved.sidebar_width = value;
        }
        if let Some(value) = self.sidebar_auto_collapse {
            saved.sidebar_auto_collapse = value;
        }
        if let Some(value) = self.message_spacing {
            saved.message_spacing = value;
        }
        self.density.map(|density| {
            saved.density = refinement(density);
            legacy_density(density)
        })
    }
}

/// The application preferences' density for `density`: what a version without
/// this record shows.
pub fn legacy_density(density: UiDensity) -> Density {
    match density {
        UiDensity::Compact => Density::Compact,
        UiDensity::Comfortable | UiDensity::Spacious => Density::Comfortable,
    }
}

/// What this record saves for `density`: only whether comfortable is spacious.
fn refinement(density: UiDensity) -> UiDensity {
    match density {
        UiDensity::Spacious => UiDensity::Spacious,
        UiDensity::Compact | UiDensity::Comfortable => UiDensity::Comfortable,
    }
}

/// The density in effect: the application preferences' base, refined by this
/// record. A compact base always wins, so an older version that switched to
/// compact after a rollback keeps compact after the update.
fn resolve(base: Density, refined: UiDensity) -> UiDensity {
    match (base, refined) {
        (Density::Compact, _) => UiDensity::Compact,
        (Density::Comfortable, UiDensity::Spacious) => UiDensity::Spacious,
        (Density::Comfortable, _) => UiDensity::Comfortable,
    }
}

/// The saved record: every option but the base density (its `density` is only
/// spacious or comfortable), plus the fields a newer version wrote.
#[derive(Clone, Debug, Default, PartialEq)]
struct Stored {
    appearance: Appearance,
    extra: Map<String, Value>,
}

impl Stored {
    fn resolved(&self, base: Density) -> Appearance {
        Appearance {
            density: resolve(base, self.appearance.density),
            ..self.appearance.clone()
        }
    }
}

/// A whole number clamped to `min..=max`; anything else reads as `default`.
fn clamped(value: Option<Value>, min: u16, max: u16, default: u16) -> u16 {
    value
        .as_ref()
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value.clamp(min.into(), max.into())).ok())
        .unwrap_or(default)
}

fn clamped_u8(value: Option<Value>, min: u8, max: u8, default: u8) -> u8 {
    u8::try_from(clamped(value, min.into(), max.into(), default.into())).unwrap_or(default)
}

/// A known value, or the default for an unknown one or a wrong type.
fn known<T: for<'de> Deserialize<'de> + Default>(value: Option<Value>) -> T {
    value
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

/// Read a saved record leniently; it never fails.
fn decode(bytes: &[u8]) -> Stored {
    let mut fields = match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    };
    let ui_scale = clamped(
        fields.remove("ui_scale"),
        MIN_UI_SCALE,
        MAX_UI_SCALE,
        DEFAULT_UI_SCALE,
    );
    let density = refinement(known(fields.remove("density")));
    let chat_width = known(fields.remove("chat_width"));
    let chat_font_size = clamped_u8(
        fields.remove("chat_font_size"),
        MIN_CHAT_FONT_SIZE,
        MAX_CHAT_FONT_SIZE,
        DEFAULT_CHAT_FONT_SIZE,
    );
    let code_font_size = clamped_u8(
        fields.remove("code_font_size"),
        MIN_CODE_FONT_SIZE,
        MAX_CODE_FONT_SIZE,
        DEFAULT_CODE_FONT_SIZE,
    );
    let sidebar_width = fields
        .remove("sidebar_width")
        .as_ref()
        .and_then(Value::as_u64)
        .and_then(|value| {
            u16::try_from(value.clamp(MIN_SIDEBAR_WIDTH.into(), MAX_SIDEBAR_WIDTH.into())).ok()
        });
    let sidebar_auto_collapse = fields
        .remove("sidebar_auto_collapse")
        .as_ref()
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let message_spacing = known(fields.remove("message_spacing"));
    let bounded = fields.len() <= MAX_EXTRA_FIELDS
        && serde_json::to_vec(&fields).is_ok_and(|bytes| bytes.len() <= MAX_EXTRA_BYTES);
    Stored {
        appearance: Appearance {
            ui_scale,
            density,
            chat_width,
            chat_font_size,
            code_font_size,
            sidebar_width,
            sidebar_auto_collapse,
            message_spacing,
        },
        extra: if bounded { fields } else { Map::new() },
    }
}

/// The record's bytes: the kept fields, overwritten by the known ones.
fn encode(stored: &Stored) -> Result<Vec<u8>, PreferencesError> {
    let Ok(Value::Object(known)) = serde_json::to_value(&stored.appearance) else {
        return Err(PreferencesError::Unavailable);
    };
    let mut fields = stored.extra.clone();
    fields.extend(known);
    serde_json::to_vec(&fields).map_err(|_| PreferencesError::Unavailable)
}

/// The application preferences' density. An unreadable record reads as
/// comfortable here (`settings.get` reports it); vault errors are returned.
fn base_density(prefs: &PreferencesService) -> Result<Density, PreferencesError> {
    match prefs.get() {
        Ok(saved) => Ok(saved.density),
        Err(PreferencesError::Invalid(_)) => Ok(Density::default()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
thread_local! {
    /// Tests only: runs once between the base density write and the record write.
    static BETWEEN_WRITES: std::cell::Cell<Option<fn(&VaultService)>> =
        const { std::cell::Cell::new(None) };
}

/// Reads and writes the appearance record; both need the vault unlocked.
///
/// Pass the same [`PreferencesService`] that serves `settings.*`: density
/// writes go through it, and its lock is what keeps them from racing other
/// preference writes. Locks are taken in the order appearance, then
/// preferences, and `settings.*` never takes the appearance lock.
pub struct AppearanceService {
    vault: Arc<VaultService>,
    transaction: Mutex<()>,
}

impl AppearanceService {
    pub fn new(vault: Arc<VaultService>) -> Self {
        Self {
            vault,
            transaction: Mutex::new(()),
        }
    }

    /// The saved appearance (defaults when none was saved), with the density
    /// resolved against the application preferences.
    pub fn get(&self, prefs: &PreferencesService) -> Result<Appearance, PreferencesError> {
        let _lock = self
            .transaction
            .lock()
            .map_err(|_| PreferencesError::Unavailable)?;
        let stored = self.read()?;
        Ok(stored.resolved(base_density(prefs)?))
    }

    /// Apply a validated patch and return the result. A density change first
    /// writes the base density to the application preferences (only when it
    /// differs), then this record; the record is written only when it changed.
    /// If either write fails the error is returned, and the density in effect is
    /// still one the user could have chosen.
    pub fn set(
        &self,
        patch: AppearancePatch,
        prefs: &PreferencesService,
    ) -> Result<Appearance, PreferencesError> {
        let _lock = self
            .transaction
            .lock()
            .map_err(|_| PreferencesError::Unavailable)?;
        patch.validate().map_err(PreferencesError::Invalid)?;
        let saved = self.read()?;
        let mut next = saved.clone();
        let base = match patch.apply(&mut next) {
            Some(wanted) => {
                let current = base_density(prefs)?;
                if current == wanted {
                    current
                } else {
                    prefs
                        .update(serde_json::json!({ "density": wanted }))?
                        .density
                }
            }
            None => base_density(prefs)?,
        };
        #[cfg(test)]
        if let Some(hook) = BETWEEN_WRITES.take() {
            hook(&self.vault);
        }
        if next != saved {
            record::write(&self.vault, APPEARANCE_SECRET, encode(&next)?)?;
        }
        Ok(next.resolved(base))
    }

    fn read(&self) -> Result<Stored, PreferencesError> {
        Ok(record::read(&self.vault, APPEARANCE_SECRET)?
            .map(|bytes| decode(bytes.expose()))
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidex_vault::{
        KdfParams, SecretString, VaultServiceConfig,
        testing::{WriteFailure, fail_next_write},
    };
    use serde_json::json;

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

    fn patch(value: Value) -> AppearancePatch {
        serde_json::from_value(value).unwrap()
    }

    fn fail_the_next_vault_write(vault: &VaultService) {
        fail_next_write(vault.dir(), WriteFailure::BeforeRename);
    }

    #[test]
    fn defaults_and_empty_record() {
        for bytes in ["{}", "not json", "[]", "null", "42", ""] {
            let stored = decode(bytes.as_bytes());
            assert_eq!(stored.appearance, Appearance::default());
            assert!(stored.extra.is_empty());
        }
        assert_eq!(
            serde_json::to_value(Appearance::default()).unwrap(),
            json!({
                "ui_scale": 100, "density": "comfortable", "chat_width": "comfortable",
                "chat_font_size": 14, "code_font_size": 13, "sidebar_width": null,
                "sidebar_auto_collapse": true, "message_spacing": "comfortable"
            })
        );
    }

    #[test]
    fn lenient_read_keeps_future_fields_and_clamps() {
        let raw = json!({
            "ui_scale": 300, "density": "dense", "chat_width": "ultra", "chat_font_size": 8,
            "code_font_size": 2, "sidebar_width": 9999, "sidebar_auto_collapse": "yes",
            "message_spacing": "relaxed", "transcript_font_size": 15,
            "panel_layout": {"side": "right"}
        });
        let stored = decode(&serde_json::to_vec(&raw).unwrap());
        assert_eq!(stored.appearance.ui_scale, MAX_UI_SCALE);
        assert_eq!(stored.appearance.density, UiDensity::Comfortable);
        assert_eq!(stored.appearance.chat_width, ChatWidth::Comfortable);
        assert_eq!(stored.appearance.chat_font_size, MIN_CHAT_FONT_SIZE);
        assert_eq!(stored.appearance.code_font_size, MIN_CODE_FONT_SIZE);
        assert_eq!(stored.appearance.sidebar_width, Some(MAX_SIDEBAR_WIDTH));
        assert!(stored.appearance.sidebar_auto_collapse);
        assert_eq!(stored.appearance.message_spacing, MessageSpacing::Relaxed);
        assert_eq!(stored.extra.len(), 2);
        let again: Value = serde_json::from_slice(&encode(&stored).unwrap()).unwrap();
        assert_eq!(again["transcript_font_size"], 15);
        assert_eq!(again["panel_layout"]["side"], "right");
        assert_eq!(again["ui_scale"], 125);

        let raw = json!({
            "ui_scale": 10, "chat_font_size": 40, "code_font_size": 99, "sidebar_width": 1,
            "density": "spacious", "sidebar_auto_collapse": false
        });
        let stored = decode(&serde_json::to_vec(&raw).unwrap());
        assert_eq!(stored.appearance.ui_scale, MIN_UI_SCALE);
        assert_eq!(stored.appearance.chat_font_size, MAX_CHAT_FONT_SIZE);
        assert_eq!(stored.appearance.code_font_size, MAX_CODE_FONT_SIZE);
        assert_eq!(stored.appearance.sidebar_width, Some(MIN_SIDEBAR_WIDTH));
        assert_eq!(stored.appearance.density, UiDensity::Spacious);
        assert!(!stored.appearance.sidebar_auto_collapse);

        // Wrong types, negatives, and fractions read as the defaults.
        let raw = json!({
            "ui_scale": -5, "chat_font_size": 14.5, "code_font_size": "13",
            "sidebar_width": "wide", "density": "compact"
        });
        let stored = decode(&serde_json::to_vec(&raw).unwrap());
        assert_eq!(
            stored.appearance,
            Appearance::default(),
            "a saved compact is only a refinement and reads as comfortable"
        );
    }

    #[test]
    fn extras_are_bounded() {
        let mut raw = Map::new();
        for i in 0..MAX_EXTRA_FIELDS {
            raw.insert(format!("f{i}"), json!(i));
        }
        assert_eq!(
            decode(&serde_json::to_vec(&raw).unwrap()).extra.len(),
            MAX_EXTRA_FIELDS
        );
        raw.insert("one_too_many".into(), json!(true));
        assert!(decode(&serde_json::to_vec(&raw).unwrap()).extra.is_empty());
        let big = json!({"blob": "x".repeat(5000), "ui_scale": 110});
        let stored = decode(&serde_json::to_vec(&big).unwrap());
        assert!(stored.extra.is_empty());
        assert_eq!(stored.appearance.ui_scale, 110);
    }

    #[test]
    fn patch_is_closed_and_strict() {
        for refused in [
            json!({"api_key": "x"}),
            json!({"chat_width": "xwide"}),
            json!({"density": "dense"}),
            json!({"ui_scale": "big"}),
            json!({"ui_scale": 110.5}),
            json!({"ui_scale": 70000}),
            json!({"sidebar_auto_collapse": "yes"}),
        ] {
            assert!(
                serde_json::from_value::<AppearancePatch>(refused.clone()).is_err(),
                "{refused}"
            );
        }
        for invalid in [
            json!({"ui_scale": 74}),
            json!({"ui_scale": 126}),
            json!({"ui_scale": 200}),
            json!({"chat_font_size": 11}),
            json!({"chat_font_size": 21}),
            json!({"code_font_size": 9}),
            json!({"code_font_size": 19}),
            json!({"sidebar_width": 100}),
            json!({"sidebar_width": 481}),
        ] {
            assert!(patch(invalid.clone()).validate().is_err(), "{invalid}");
        }
        for valid in [
            json!({"ui_scale": 75, "chat_font_size": 12, "code_font_size": 10, "sidebar_width": 208}),
            json!({"ui_scale": 125, "chat_font_size": 20, "code_font_size": 18, "sidebar_width": 480}),
        ] {
            assert!(patch(valid.clone()).validate().is_ok(), "{valid}");
        }
        assert_eq!(
            patch(json!({"sidebar_width": null})).sidebar_width,
            Some(None)
        );
        assert_eq!(patch(json!({})), AppearancePatch::default());
    }

    #[test]
    fn density_splits_across_records() {
        let mut stored = decode(b"{}");
        let spacious = AppearancePatch {
            density: Some(UiDensity::Spacious),
            ..Default::default()
        };
        assert_eq!(spacious.apply(&mut stored), Some(Density::Comfortable));
        assert_eq!(
            stored.resolved(Density::Comfortable).density,
            UiDensity::Spacious
        );
        // An older app switches to compact after a rollback: compact wins.
        assert_eq!(
            stored.resolved(Density::Compact).density,
            UiDensity::Compact
        );
        let compact = AppearancePatch {
            density: Some(UiDensity::Compact),
            ..Default::default()
        };
        assert_eq!(compact.apply(&mut stored), Some(Density::Compact));
        assert_eq!(stored.appearance.density, UiDensity::Comfortable);
        // The record never saves compact: it is only the spacious refinement.
        let saved: Value = serde_json::from_slice(&encode(&stored).unwrap()).unwrap();
        assert_eq!(saved["density"], "comfortable");
        assert_eq!(legacy_density(UiDensity::Spacious), Density::Comfortable);
        assert_eq!(legacy_density(UiDensity::Comfortable), Density::Comfortable);
        assert_eq!(legacy_density(UiDensity::Compact), Density::Compact);
    }

    #[test]
    fn set_keeps_fields_a_newer_version_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let vault = unlocked_vault(dir.path());
        let prefs = PreferencesService::new(vault.clone());
        let appearance = AppearanceService::new(vault.clone());
        record::write(
            &vault,
            APPEARANCE_SECRET,
            serde_json::to_vec(&json!({
                "ui_scale": 90, "density": "spacious", "transcript_font_size": 15
            }))
            .unwrap(),
        )
        .unwrap();
        let saved = appearance
            .set(patch(json!({"ui_scale": 110})), &prefs)
            .unwrap();
        assert_eq!(saved.ui_scale, 110);
        assert_eq!(saved.density, UiDensity::Spacious);
        let raw = record::read(&vault, APPEARANCE_SECRET).unwrap().unwrap();
        let raw: Value = serde_json::from_slice(raw.expose()).unwrap();
        assert_eq!(raw["transcript_font_size"], 15);
        assert_eq!(raw["ui_scale"], 110);
        assert_eq!(raw["density"], "spacious");
    }

    #[test]
    fn nothing_is_written_when_nothing_changed() {
        let dir = tempfile::tempdir().unwrap();
        let vault = unlocked_vault(dir.path());
        let prefs = PreferencesService::new(vault.clone());
        let appearance = AppearanceService::new(vault.clone());
        // Defaults on a fresh vault: nothing to save, not even the record.
        let defaults = json!({"ui_scale": 100, "density": "comfortable", "sidebar_width": null});
        assert_eq!(
            appearance.set(patch(defaults.clone()), &prefs).unwrap(),
            Appearance::default()
        );
        assert!(vault.list_secrets().unwrap().is_empty());

        appearance
            .set(
                patch(json!({"ui_scale": 110, "density": "compact"})),
                &prefs,
            )
            .unwrap();
        // An armed write failure survives a patch that changes nothing...
        fail_the_next_vault_write(&vault);
        let same = json!({"ui_scale": 110, "density": "compact", "chat_width": "comfortable"});
        assert_eq!(
            appearance.set(patch(same), &prefs).unwrap().density,
            UiDensity::Compact
        );
        // ...and strikes the next real write.
        assert!(matches!(
            appearance.set(patch(json!({"ui_scale": 125})), &prefs),
            Err(PreferencesError::Vault(_))
        ));
        assert_eq!(appearance.get(&prefs).unwrap().ui_scale, 110);
    }

    #[test]
    fn partial_density_write_stays_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let vault = unlocked_vault(dir.path());
        let prefs = PreferencesService::new(vault.clone());
        let appearance = AppearanceService::new(vault.clone());
        appearance
            .set(patch(json!({"density": "compact"})), &prefs)
            .unwrap();

        // Compact to spacious: the base becomes comfortable, the record write fails.
        BETWEEN_WRITES.set(Some(fail_the_next_vault_write));
        assert!(
            appearance
                .set(patch(json!({"density": "spacious"})), &prefs)
                .is_err()
        );
        assert_eq!(prefs.get().unwrap().density, Density::Comfortable);
        assert_eq!(
            appearance.get(&prefs).unwrap().density,
            UiDensity::Comfortable
        );

        // Spacious to compact: the base becomes compact, the record keeps spacious.
        appearance
            .set(patch(json!({"density": "spacious"})), &prefs)
            .unwrap();
        BETWEEN_WRITES.set(Some(fail_the_next_vault_write));
        assert!(
            appearance
                .set(patch(json!({"density": "compact"})), &prefs)
                .is_err()
        );
        assert_eq!(appearance.get(&prefs).unwrap().density, UiDensity::Compact);

        // A failed base write leaves both records alone.
        appearance
            .set(patch(json!({"density": "spacious"})), &prefs)
            .unwrap();
        fail_the_next_vault_write(&vault);
        assert!(
            appearance
                .set(patch(json!({"density": "compact"})), &prefs)
                .is_err()
        );
        assert_eq!(prefs.get().unwrap().density, Density::Comfortable);
        assert_eq!(appearance.get(&prefs).unwrap().density, UiDensity::Spacious);
    }

    #[test]
    fn an_unreadable_base_reads_as_comfortable() {
        let dir = tempfile::tempdir().unwrap();
        let vault = unlocked_vault(dir.path());
        let prefs = PreferencesService::new(vault.clone());
        let appearance = AppearanceService::new(vault.clone());
        record::write(
            &vault,
            crate::PREFERENCES_SECRET,
            b"{\"density\":\"x\"}".to_vec(),
        )
        .unwrap();
        assert!(matches!(prefs.get(), Err(PreferencesError::Invalid(_))));
        assert_eq!(
            appearance
                .set(
                    patch(json!({"density": "spacious", "ui_scale": 90})),
                    &prefs
                )
                .unwrap()
                .density,
            UiDensity::Spacious
        );
        // Writing compact needs the base, which settings.set must repair first.
        assert!(matches!(
            appearance.set(patch(json!({"density": "compact"})), &prefs),
            Err(PreferencesError::Invalid(_))
        ));
        assert_eq!(appearance.get(&prefs).unwrap().ui_scale, 90);
    }
}
