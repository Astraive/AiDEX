//! Closed-schema provider and network configuration on the drive.
//!
//! `private_config/providers.toml` names providers, their endpoints, the vault
//! secret each one authenticates with, optional model declarations, and the
//! default model. `private_config/network.toml` holds the offline switch.
//! `private_config/provider-accounts.toml` holds account presentation
//! metadata that older builds do not read. No file ever contains a
//! credential value.

mod accounts;
mod file;

use std::path::PathBuf;

use aidex_drive::DrivePaths;
use serde::{Deserialize, Serialize};

pub use accounts::{
    AccountMetadata, MAX_ACCOUNT_NAME_CHARS, PROVIDER_ACCOUNTS_FILE, ProviderAccountMeta,
    ProviderAccountsConfig, is_account_name, is_preset_id,
};
pub use file::ConfigError;
pub(crate) use file::{read_toml, write_toml};

use crate::{
    CapabilitySource, CostHint, Locality, ModelCapabilities, ModelDescriptor, ModelId, ModelRef,
    ModelRole, ProviderId, ToolCallSupport, credentials::is_secret_name,
};

/// Provider configuration file name inside `DrivePaths::config()`.
pub const PROVIDERS_FILE: &str = "providers.toml";
/// Network configuration file name inside `DrivePaths::config()`.
pub const NETWORK_FILE: &str = "network.toml";
/// Schema version written by this build.
pub const CONFIG_VERSION: u32 = 1;

/// Absolute path of `name` inside the drive's private configuration directory.
pub fn config_file(paths: &DrivePaths, name: &str) -> PathBuf {
    paths.config().join(name)
}

/// Create the configuration directory (rejecting links) and return the file path.
pub(crate) fn prepare_config_file(paths: &DrivePaths, name: &str) -> Result<PathBuf, ConfigError> {
    let dir = paths
        .ensure_dir(&paths.config())
        .map_err(|error| ConfigError::Io {
            path: paths.config(),
            message: error.to_string(),
        })?;
    Ok(dir.join(name))
}

/// How a provider is implemented.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    /// Any server speaking the OpenAI `/v1/models` + `/v1/chat/completions` API.
    OpenaiCompatible,
    OpenaiResponses,
    Anthropic,
    Gemini,
    AzureOpenai,
    Bedrock,
    /// A managed local llama.cpp runtime (registered at runtime, never configured).
    LlamaCpp,
    /// The deterministic in-process [`crate::FakeProvider`].
    Fake,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiCompatible => "openai-compatible",
            Self::OpenaiResponses => "openai-responses",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
            Self::AzureOpenai => "azure-openai",
            Self::Bedrock => "bedrock",
            Self::LlamaCpp => "llama-cpp",
            Self::Fake => "fake",
        }
    }
}

impl std::str::FromStr for ProviderKind {
    type Err = String;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "openai-compatible" => Ok(Self::OpenaiCompatible),
            "openai-responses" => Ok(Self::OpenaiResponses),
            "anthropic" => Ok(Self::Anthropic),
            "gemini" => Ok(Self::Gemini),
            "azure-openai" => Ok(Self::AzureOpenai),
            "bedrock" => Ok(Self::Bedrock),
            "llama-cpp" => Ok(Self::LlamaCpp),
            "fake" => Ok(Self::Fake),
            _ => Err("unknown provider kind (use openai-compatible)".into()),
        }
    }
}

/// How a provider authenticates. Only a secret *name* is ever stored.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthConfig {
    #[default]
    None,
    /// A bearer token held in the vault and released only to the consumer
    /// `provider:<id>`.
    VaultSecret { secret_name: String },
}

/// One model declared for a provider; declarations override discovered metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    pub id: ModelId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub context_window: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub tool_calls: ToolCallSupport,
    #[serde(default)]
    pub json_schema_output: bool,
    /// Accepts chat requests (set false for embedding-only models).
    #[serde(default = "yes")]
    pub chat: bool,
    #[serde(default = "yes")]
    pub streaming: bool,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub embeddings: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<ModelRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostHint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_ram_bytes: Option<u64>,
}

fn yes() -> bool {
    true
}

impl ModelConfig {
    /// A declared chat model with default capabilities.
    pub fn new(id: ModelId, context_window: u32) -> Self {
        Self {
            id,
            display_name: None,
            context_window,
            max_output_tokens: None,
            tool_calls: ToolCallSupport::None,
            json_schema_output: false,
            chat: true,
            streaming: true,
            vision: false,
            embeddings: false,
            roles: Vec::new(),
            cost: None,
            min_ram_bytes: None,
        }
    }

    /// The descriptor this declaration produces for `provider`.
    pub fn descriptor(&self, provider: &ProviderId, locality: Locality) -> ModelDescriptor {
        ModelDescriptor {
            model: ModelRef::new(provider.clone(), self.id.clone()),
            display_name: self
                .display_name
                .clone()
                .unwrap_or_else(|| self.id.to_string()),
            locality,
            context_window: self.context_window,
            max_output_tokens: self.max_output_tokens,
            capabilities: ModelCapabilities {
                chat: self.chat,
                streaming: self.streaming,
                tool_calls: self.tool_calls,
                json_schema_output: self.json_schema_output,
                vision: self.vision,
                embeddings: self.embeddings,
            },
            roles: if self.roles.is_empty() {
                vec![if self.embeddings {
                    ModelRole::Embed
                } else {
                    ModelRole::General
                }]
            } else {
                self.roles.clone()
            },
            cost: self.cost,
            min_ram_bytes: self.min_ram_bytes,
            source: CapabilitySource::Configured,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.context_window == 0 {
            return Err(format!(
                "model `{}` needs a positive context_window",
                self.id
            ));
        }
        if self.max_output_tokens == Some(0) {
            return Err(format!("model `{}` has max_output_tokens = 0", self.id));
        }
        if self.display_name.as_deref().is_some_and(|name| {
            name.is_empty() || name.len() > 128 || name.chars().any(char::is_control)
        }) {
            return Err(format!("model `{}` has an invalid display_name", self.id));
        }
        Ok(())
    }
}

/// One configured provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: ProviderId,
    pub kind: ProviderKind,
    /// API root including its version segment, e.g. `https://api.openai.com/v1`
    /// or `http://127.0.0.1:8080/v1` (an empty path means `/v1`).
    pub base_url: String,
    /// Override the locality derived from `base_url`. Only `remote` may be
    /// forced (for example a loopback tunnel to a cloud service); a
    /// non-loopback endpoint can never be declared `local`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locality: Option<Locality>,
    #[serde(default)]
    pub auth: AuthConfig,
    /// Query `/models` for the served model list (default true). When false,
    /// only declared models are offered and listing needs no network.
    #[serde(default = "yes")]
    pub discover_models: bool,
    #[serde(default, rename = "model", skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelConfig>,
}

impl ProviderConfig {
    pub fn new(id: ProviderId, kind: ProviderKind, base_url: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            base_url: base_url.into(),
            locality: None,
            auth: AuthConfig::None,
            discover_models: true,
            models: Vec::new(),
        }
    }

    /// Structural checks that need no network. Endpoint parsing and locality
    /// rules are checked when the provider is built.
    pub fn validate(&self) -> Result<(), String> {
        if matches!(self.kind, ProviderKind::Fake | ProviderKind::LlamaCpp) {
            return Err(format!(
                "provider `{}`: only kind `openai-compatible` can be configured",
                self.id
            ));
        }
        if let AuthConfig::VaultSecret { secret_name } = &self.auth {
            if !is_secret_name(secret_name) {
                return Err(format!(
                    "provider `{}`: secret names are 1-128 ASCII letters, digits, `.`, `_`, or `-`, starting with a letter or digit",
                    self.id
                ));
            }
            if secret_name.starts_with("oauth.")
                && (secret_name != &format!("oauth.{}.access", self.id)
                    || self.kind != ProviderKind::OpenaiResponses
                    || self.base_url.trim_end_matches('/') != "https://api.openai.com/v1")
            {
                return Err("OAuth credentials are bound to their registered account and the trusted OpenAI Responses endpoint".into());
            }
        }
        let mut seen = std::collections::HashSet::new();
        for model in &self.models {
            model
                .validate()
                .map_err(|error| format!("provider `{}`: {error}", self.id))?;
            if !seen.insert(&model.id) {
                return Err(format!(
                    "provider `{}` declares model `{}` twice",
                    self.id, model.id
                ));
            }
        }
        if !self.discover_models && self.models.is_empty() {
            return Err(format!(
                "provider `{}` disables model discovery but declares no models",
                self.id
            ));
        }
        Ok(())
    }
}

/// `providers.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersConfig {
    pub version: u32,
    /// Default model as `provider/model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, rename = "provider", skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<ProviderConfig>,
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            default_model: None,
            providers: Vec::new(),
        }
    }
}

impl ProvidersConfig {
    /// Read `providers.toml`; a missing file is an empty configuration.
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
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
        for provider in &self.providers {
            provider.validate()?;
            if !ids.insert(&provider.id) {
                return Err(format!("provider `{}` is configured twice", provider.id));
            }
        }
        if let Some(model) = &self.default_model {
            ModelRef::parse(model).map_err(|error| format!("default_model: {error}"))?;
        }
        Ok(())
    }

    pub fn default_model(&self) -> Option<ModelRef> {
        self.default_model
            .as_deref()
            .and_then(|text| ModelRef::parse(text).ok())
    }
}

/// `network.toml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    pub version: u32,
    /// Refuse all remote provider traffic.
    #[serde(default)]
    pub offline: bool,
}

impl NetworkConfig {
    pub fn load(path: &std::path::Path) -> Result<Self, ConfigError> {
        let config = read_toml::<Self>(path)?.unwrap_or(Self {
            version: CONFIG_VERSION,
            offline: false,
        });
        if config.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion {
                path: path.to_path_buf(),
                found: config.version,
                supported: CONFIG_VERSION,
            });
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_schema_round_trips_and_rejects_unknown_fields() {
        let mut provider = ProviderConfig::new(
            ProviderId::new("openai").unwrap(),
            ProviderKind::OpenaiCompatible,
            "https://api.openai.com/v1",
        );
        provider.auth = AuthConfig::VaultSecret {
            secret_name: "openai-key".into(),
        };
        let mut model = ModelConfig::new(ModelId::new("gpt-4o-mini").unwrap(), 128_000);
        model.tool_calls = ToolCallSupport::Native;
        model.roles = vec![ModelRole::Code];
        provider.models.push(model);
        let config = ProvidersConfig {
            version: 1,
            default_model: Some("openai/gpt-4o-mini".into()),
            providers: vec![provider],
        };
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("secret_name = \"openai-key\""), "{text}");
        let parsed: ProvidersConfig = toml::from_str(&text).unwrap();
        assert_eq!(parsed, config);
        assert!(parsed.validate().is_ok());

        let unknown = format!("{text}\napi_key = \"x\"\n");
        assert!(toml::from_str::<ProvidersConfig>(&unknown).is_err());
        let inline_key = text.replace("secret_name = \"openai-key\"", "value = \"sk-1\"");
        assert!(toml::from_str::<ProvidersConfig>(&inline_key).is_err());
    }

    #[test]
    fn validation_rejects_unsupported_kinds_duplicates_and_bad_names() {
        let id = ProviderId::new("p").unwrap();
        let mut fake = ProviderConfig::new(id.clone(), ProviderKind::Fake, "http://127.0.0.1:1");
        assert!(fake.validate().is_err());
        fake.kind = ProviderKind::OpenaiCompatible;
        assert!(fake.validate().is_ok());
        fake.auth = AuthConfig::VaultSecret {
            secret_name: "bad name".into(),
        };
        assert!(fake.validate().is_err());
        fake.auth = AuthConfig::None;
        fake.discover_models = false;
        assert!(fake.validate().is_err());
        let config = ProvidersConfig {
            version: 1,
            default_model: None,
            providers: vec![
                ProviderConfig::new(
                    id.clone(),
                    ProviderKind::OpenaiCompatible,
                    "http://127.0.0.1:1",
                ),
                ProviderConfig::new(id, ProviderKind::OpenaiCompatible, "http://127.0.0.1:2"),
            ],
        };
        assert!(config.validate().is_err());
    }
}
