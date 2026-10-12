//! Portable, encrypted application preferences.
mod model;
mod model_preferences;
mod record;
mod service;
pub use model::{AppPreferences, Density, Theme};
pub use model_preferences::{
    MAX_FAVORITE_MODELS, MAX_HIDDEN_MODELS, MAX_MODEL_KEY_BYTES, MIN_MODEL_KEY_BYTES,
    MODEL_PREFERENCES_SECRET, ModelPreferences, ModelPreferencesPatch, ModelPreferencesService,
    is_model_key,
};
pub use service::{PREFERENCES_SECRET, PreferencesError, PreferencesService};
