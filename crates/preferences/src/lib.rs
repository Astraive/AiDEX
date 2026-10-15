//! Portable, encrypted application preferences.
mod appearance;
mod model;
mod model_preferences;
mod record;
mod service;
pub use appearance::{
    APPEARANCE_SECRET, Appearance, AppearancePatch, AppearanceService, ChatWidth,
    DEFAULT_CHAT_FONT_SIZE, DEFAULT_CODE_FONT_SIZE, DEFAULT_UI_SCALE, MAX_CHAT_FONT_SIZE,
    MAX_CODE_FONT_SIZE, MAX_SIDEBAR_WIDTH, MAX_UI_SCALE, MIN_CHAT_FONT_SIZE, MIN_CODE_FONT_SIZE,
    MIN_SIDEBAR_WIDTH, MIN_UI_SCALE, MessageSpacing, UiDensity, legacy_density,
};
pub use model::{AppPreferences, Density, Theme};
pub use model_preferences::{
    MAX_FAVORITE_MODELS, MAX_HIDDEN_MODELS, MAX_MODEL_KEY_BYTES, MIN_MODEL_KEY_BYTES,
    MODEL_PREFERENCES_SECRET, ModelPreferences, ModelPreferencesPatch, ModelPreferencesService,
    is_model_key,
};
pub use service::{PREFERENCES_SECRET, PreferencesError, PreferencesService};
