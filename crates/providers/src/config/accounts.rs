//! `private_config/provider-accounts.toml`: per-account presentation metadata
//! (display name, preset, enabled flag) kept beside `providers.toml`.
//!
//! `providers.toml` stays byte-compatible with older daemons (an
//! `update.rollback` may start one on the same drive), so metadata that older
//! builds do not understand lives here; older builds simply ignore this file.

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

use super::{CONFIG_VERSION, ConfigError, read_toml, yes};
use crate::ProviderId;

/// Account metadata file name inside `DrivePaths::config()`.
pub const PROVIDER_ACCOUNTS_FILE: &str = "provider-accounts.toml";

/// Longest account display name, in characters.
pub const MAX_ACCOUNT_NAME_CHARS: usize = 64;

/// `provider-accounts.toml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAccountsConfig {
    pub version: u32,
    #[serde(default, rename = "account", skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<ProviderAccountMeta>,
}

impl Default for ProviderAccountsConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            accounts: Vec::new(),
        }
    }
}

/// Metadata of one configured account.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderAccountMeta {
    pub id: ProviderId,
    /// What the UI calls the account (1-64 characters, no control characters).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The preset the account was created from (`[a-z0-9-]{1,64}`); unknown
    /// ids are kept, so a newer build's presets survive a rollback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<String>,
    /// Disabled accounts stay configured but are not listed or used.
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Account metadata as the registry API takes and reports it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountMetadata {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub preset_id: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

impl Default for AccountMetadata {
    /// No name, no preset, enabled.
    fn default() -> Self {
        Self {
            display_name: None,
            preset_id: None,
            enabled: true,
        }
    }
}

impl AccountMetadata {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(name) = &self.display_name
            && !is_account_name(name)
        {
            return Err(format!(
                "account display names are 1-{MAX_ACCOUNT_NAME_CHARS} characters without control characters"
            ));
        }
        if let Some(preset) = &self.preset_id
            && !is_preset_id(preset)
        {
            return Err("preset ids are 1-64 lowercase ASCII letters, digits, or `-`".into());
        }
        Ok(())
    }

    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A usable account display name.
pub fn is_account_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= MAX_ACCOUNT_NAME_CHARS
        && !name.chars().any(char::is_control)
}

/// `[a-z0-9-]{1,64}`.
pub fn is_preset_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl ProviderAccountsConfig {
    /// Read the sidecar; a missing file means no metadata.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let Some(config) = read_toml::<Self>(path)? else {
            return Ok(Self::default());
        };
        if config.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: config.version,
                supported: CONFIG_VERSION,
            });
        }
        config.validate().map_err(|message| ConfigError::Invalid {
            path: path.to_path_buf(),
            message,
        })?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for account in &self.accounts {
            account
                .metadata()
                .validate()
                .map_err(|error| format!("account `{}`: {error}", account.id))?;
            if !ids.insert(&account.id) {
                return Err(format!("account `{}` is listed twice", account.id));
            }
        }
        Ok(())
    }

    /// Metadata keyed by account id.
    pub(crate) fn into_map(self) -> BTreeMap<ProviderId, AccountMetadata> {
        self.accounts
            .into_iter()
            .map(|account| {
                let metadata = account.metadata();
                (account.id, metadata)
            })
            .collect()
    }

    /// The file contents for `accounts`, skipping entries that carry only
    /// defaults.
    pub(crate) fn from_map(accounts: &BTreeMap<ProviderId, AccountMetadata>) -> Self {
        Self {
            version: CONFIG_VERSION,
            accounts: accounts
                .iter()
                .filter(|(_, metadata)| !metadata.is_default())
                .map(|(id, metadata)| ProviderAccountMeta {
                    id: id.clone(),
                    display_name: metadata.display_name.clone(),
                    preset_id: metadata.preset_id.clone(),
                    enabled: metadata.enabled,
                })
                .collect(),
        }
    }
}

impl ProviderAccountMeta {
    pub fn metadata(&self) -> AccountMetadata {
        AccountMetadata {
            display_name: self.display_name.clone(),
            preset_id: self.preset_id.clone(),
            enabled: self.enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_schema_is_closed_and_validated() {
        let text = "version = 1\n[[account]]\nid = \"anthropic\"\ndisplay_name = \"Work Claude\"\npreset_id = \"anthropic\"\n";
        let parsed: ProviderAccountsConfig = toml::from_str(text).unwrap();
        assert!(parsed.validate().is_ok());
        assert!(parsed.accounts[0].enabled, "enabled defaults to true");
        assert!(
            toml::from_str::<ProviderAccountsConfig>(&format!("{text}api_key = \"x\"\n")).is_err()
        );
        for bad in [
            "display_name = \"\"",
            "display_name = \"   \"",
            "display_name = \"a\\u0007b\"",
            "preset_id = \"Upper\"",
            "preset_id = \"has_underscore\"",
        ] {
            let text = format!("version = 1\n[[account]]\nid = \"x\"\n{bad}\n");
            let parsed: ProviderAccountsConfig = toml::from_str(&text).unwrap();
            assert!(parsed.validate().is_err(), "{bad}");
        }
        let long = format!(
            "version = 1\n[[account]]\nid = \"x\"\ndisplay_name = \"{}\"\n",
            "n".repeat(65)
        );
        let parsed: ProviderAccountsConfig = toml::from_str(&long).unwrap();
        assert!(parsed.validate().is_err());
        let twice = "version = 1\n[[account]]\nid = \"x\"\n[[account]]\nid = \"x\"\n";
        let parsed: ProviderAccountsConfig = toml::from_str(twice).unwrap();
        assert!(parsed.validate().is_err());
    }

    #[test]
    fn default_entries_are_not_written() {
        let mut accounts = BTreeMap::new();
        accounts.insert(
            ProviderId::new("plain").unwrap(),
            AccountMetadata::default(),
        );
        accounts.insert(
            ProviderId::new("named").unwrap(),
            AccountMetadata {
                display_name: Some("Named".into()),
                ..AccountMetadata::default()
            },
        );
        let file = ProviderAccountsConfig::from_map(&accounts);
        assert_eq!(file.accounts.len(), 1);
        assert_eq!(file.accounts[0].id.as_str(), "named");
        let text = toml::to_string(&file).unwrap();
        let parsed: ProviderAccountsConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed, file);
    }
}
