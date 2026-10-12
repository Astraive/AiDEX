//! Model picker preferences: favorite and hidden model keys.
//!
//! They live in their own vault secret, apart from [`AppPreferences`], so a
//! daemon that predates them still reads `aidex-preferences` after an update
//! rollback (both records are closed schemas).
//!
//! [`AppPreferences`]: crate::AppPreferences
use crate::{PreferencesError, record};
use aidex_vault::VaultService;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

/// Vault secret holding [`ModelPreferences`].
pub const MODEL_PREFERENCES_SECRET: &str = "aidex-model-preferences";
/// Most favorite model keys kept.
pub const MAX_FAVORITE_MODELS: usize = 64;
/// Most hidden model keys kept.
pub const MAX_HIDDEN_MODELS: usize = 512;
/// Shortest model key in bytes (`a/b` plus one).
pub const MIN_MODEL_KEY_BYTES: usize = 3;
/// Longest model key in bytes.
pub const MAX_MODEL_KEY_BYTES: usize = 512;

/// Favorite and hidden model keys. A key is `provider/model` for a native
/// model or `runtime:<runtime_id>/<model>` for an external runtime's model.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ModelPreferences {
    pub favorite_models: Vec<String>,
    pub hidden_models: Vec<String>,
}

/// `model.preferences.set` params: an omitted (or `null`) list stays as saved.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelPreferencesPatch {
    #[serde(default)]
    pub favorite_models: Option<Vec<String>>,
    #[serde(default)]
    pub hidden_models: Option<Vec<String>>,
}

/// Whether `key` can be stored: 3-512 bytes, contains `/`, no control characters.
pub fn is_model_key(key: &str) -> bool {
    (MIN_MODEL_KEY_BYTES..=MAX_MODEL_KEY_BYTES).contains(&key.len())
        && key.contains('/')
        && !key.chars().any(char::is_control)
}

const INVALID_KEY: &str = "model keys are 3-512 bytes, contain `/`, and have no control characters";

/// Validate keys and drop duplicates, keeping the first occurrence.
fn normalize(
    keys: Vec<String>,
    max: usize,
    too_many: &'static str,
) -> Result<Vec<String>, &'static str> {
    if keys.iter().any(|key| !is_model_key(key)) {
        return Err(INVALID_KEY);
    }
    let mut seen = HashSet::with_capacity(keys.len());
    let unique: Vec<String> = keys
        .into_iter()
        .filter(|key| seen.insert(key.clone()))
        .collect();
    if unique.len() > max {
        return Err(too_many);
    }
    Ok(unique)
}

impl ModelPreferences {
    /// The record with valid keys and no duplicates, or why it is refused.
    pub fn normalized(self) -> Result<Self, &'static str> {
        Ok(Self {
            favorite_models: normalize(
                self.favorite_models,
                MAX_FAVORITE_MODELS,
                "at most 64 favorite models can be saved",
            )?,
            hidden_models: normalize(
                self.hidden_models,
                MAX_HIDDEN_MODELS,
                "at most 512 hidden models can be saved",
            )?,
        })
    }
}

/// Reads and writes [`ModelPreferences`] in the vault; both need it unlocked.
pub struct ModelPreferencesService {
    vault: Arc<VaultService>,
    transaction: Mutex<()>,
}

impl ModelPreferencesService {
    pub fn new(vault: Arc<VaultService>) -> Self {
        Self {
            vault,
            transaction: Mutex::new(()),
        }
    }

    /// The saved record; empty lists when none was saved.
    pub fn get(&self) -> Result<ModelPreferences, PreferencesError> {
        let _lock = self
            .transaction
            .lock()
            .map_err(|_| PreferencesError::Unavailable)?;
        self.read()
    }

    /// Replace the lists the patch names and return the saved record.
    pub fn set(&self, patch: ModelPreferencesPatch) -> Result<ModelPreferences, PreferencesError> {
        let _lock = self
            .transaction
            .lock()
            .map_err(|_| PreferencesError::Unavailable)?;
        let mut next = match (&patch.favorite_models, &patch.hidden_models) {
            // Nothing saved is kept, so an unreadable record can be replaced.
            (Some(_), Some(_)) => ModelPreferences::default(),
            _ => self.read()?,
        };
        if let Some(favorites) = patch.favorite_models {
            next.favorite_models = favorites;
        }
        if let Some(hidden) = patch.hidden_models {
            next.hidden_models = hidden;
        }
        let next = next.normalized().map_err(PreferencesError::Invalid)?;
        let bytes = serde_json::to_vec(&next).map_err(|_| PreferencesError::Unavailable)?;
        record::write(&self.vault, MODEL_PREFERENCES_SECRET, bytes)?;
        Ok(next)
    }

    fn read(&self) -> Result<ModelPreferences, PreferencesError> {
        let Some(bytes) = record::read(&self.vault, MODEL_PREFERENCES_SECRET)? else {
            return Ok(ModelPreferences::default());
        };
        serde_json::from_slice::<ModelPreferences>(bytes.expose())
            .map_err(|_| PreferencesError::Invalid("saved model preferences are invalid"))?
            .normalized()
            .map_err(|_| PreferencesError::Invalid("saved model preferences are invalid"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(prefix: &str, count: usize) -> Vec<String> {
        (0..count).map(|i| format!("{prefix}/model-{i}")).collect()
    }

    #[test]
    fn keys_need_a_slash_bounded_length_and_no_control_characters() {
        assert!(is_model_key("openai/gpt-5"));
        assert!(is_model_key("runtime:codex/default"));
        assert!(is_model_key("a/b"));
        assert!(!is_model_key("/b"));
        assert!(!is_model_key("no-slash"));
        assert!(!is_model_key("openai/gpt\n5"));
        assert!(!is_model_key(&format!(
            "p/{}",
            "m".repeat(MAX_MODEL_KEY_BYTES)
        )));
        assert!(is_model_key(&format!(
            "p/{}",
            "m".repeat(MAX_MODEL_KEY_BYTES - 2)
        )));
    }

    #[test]
    fn duplicates_are_dropped_in_order_before_the_limit_applies() {
        let record = ModelPreferences {
            favorite_models: vec![
                "b/two".into(),
                "a/one".into(),
                "b/two".into(),
                "c/three".into(),
                "a/one".into(),
            ],
            hidden_models: vec![],
        }
        .normalized()
        .unwrap();
        assert_eq!(record.favorite_models, ["b/two", "a/one", "c/three"]);

        let mut favorites = keys("p", MAX_FAVORITE_MODELS);
        favorites.extend(keys("p", MAX_FAVORITE_MODELS));
        let record = ModelPreferences {
            favorite_models: favorites,
            hidden_models: keys("h", MAX_HIDDEN_MODELS),
        }
        .normalized()
        .unwrap();
        assert_eq!(record.favorite_models.len(), MAX_FAVORITE_MODELS);
        assert_eq!(record.hidden_models.len(), MAX_HIDDEN_MODELS);

        assert!(
            ModelPreferences {
                favorite_models: keys("p", MAX_FAVORITE_MODELS + 1),
                hidden_models: vec![],
            }
            .normalized()
            .is_err()
        );
        assert!(
            ModelPreferences {
                favorite_models: vec![],
                hidden_models: keys("h", MAX_HIDDEN_MODELS + 1),
            }
            .normalized()
            .is_err()
        );
    }

    #[test]
    fn records_and_patches_are_closed_schemas() {
        assert!(serde_json::from_str::<ModelPreferences>(r#"{"recent_models":[]}"#).is_err());
        assert!(serde_json::from_str::<ModelPreferencesPatch>(r#"{"favorites":["a/b"]}"#).is_err());
        assert_eq!(
            serde_json::from_str::<ModelPreferences>("{}").unwrap(),
            ModelPreferences::default()
        );
        assert_eq!(
            serde_json::from_str::<ModelPreferencesPatch>(r#"{"hidden_models":null}"#).unwrap(),
            ModelPreferencesPatch::default()
        );
    }
}
