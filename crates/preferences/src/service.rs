use crate::{AppPreferences, record};
use aidex_vault::{VaultError, VaultService};
use serde_json::Value;
use std::{
    fmt,
    sync::{Arc, Mutex},
};

/// Vault secret holding [`AppPreferences`].
pub const PREFERENCES_SECRET: &str = "aidex-preferences";

#[derive(Debug)]
pub enum PreferencesError {
    Vault(VaultError),
    Invalid(&'static str),
    Unavailable,
}
impl fmt::Display for PreferencesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vault(error) => write!(formatter, "{error}"),
            Self::Invalid(message) => formatter.write_str(message),
            Self::Unavailable => formatter.write_str("preference state unavailable"),
        }
    }
}
impl std::error::Error for PreferencesError {}
impl From<VaultError> for PreferencesError {
    fn from(error: VaultError) -> Self {
        Self::Vault(error)
    }
}

pub struct PreferencesService {
    vault: Arc<VaultService>,
    transaction: Mutex<()>,
}
impl PreferencesService {
    pub fn new(vault: Arc<VaultService>) -> Self {
        Self {
            vault,
            transaction: Mutex::new(()),
        }
    }
    pub fn get(&self) -> Result<AppPreferences, PreferencesError> {
        let _lock = self
            .transaction
            .lock()
            .map_err(|_| PreferencesError::Unavailable)?;
        self.read()
    }
    pub fn update(&self, patch: Value) -> Result<AppPreferences, PreferencesError> {
        let _lock = self
            .transaction
            .lock()
            .map_err(|_| PreferencesError::Unavailable)?;
        let patch = patch.as_object().ok_or(PreferencesError::Invalid(
            "preferences patch must be an object",
        ))?;
        let mut value =
            serde_json::to_value(self.read()?).map_err(|_| PreferencesError::Unavailable)?;
        let values = value.as_object_mut().ok_or(PreferencesError::Unavailable)?;
        for (key, value) in patch {
            values.insert(key.clone(), value.clone());
        }
        let updated: AppPreferences = serde_json::from_value(value)
            .map_err(|_| PreferencesError::Invalid("invalid or unknown preference field"))?;
        updated.validate().map_err(PreferencesError::Invalid)?;
        let bytes = serde_json::to_vec(&updated).map_err(|_| PreferencesError::Unavailable)?;
        record::write(&self.vault, PREFERENCES_SECRET, bytes)?;
        Ok(updated)
    }
    fn read(&self) -> Result<AppPreferences, PreferencesError> {
        let Some(bytes) = record::read(&self.vault, PREFERENCES_SECRET)? else {
            return Ok(AppPreferences::default());
        };
        let settings: AppPreferences = serde_json::from_slice(bytes.expose())
            .map_err(|_| PreferencesError::Invalid("saved preferences are invalid"))?;
        settings.validate().map_err(PreferencesError::Invalid)?;
        Ok(settings)
    }
}
