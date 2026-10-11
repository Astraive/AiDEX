use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchProvider {
    #[default]
    Brave,
    Tavily,
}
impl SearchProvider {
    pub fn secret_name(self) -> &'static str {
        match self {
            Self::Brave => "aidex-web-search-brave-key",
            Self::Tavily => "aidex-web-search-tavily-key",
        }
    }
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Brave => "https://api.search.brave.com/res/v1/web/search",
            Self::Tavily => "https://api.tavily.com/search",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SearchSettings {
    pub enabled: bool,
    pub provider: SearchProvider,
    pub revision: u64,
}
impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: SearchProvider::Brave,
            revision: 0,
        }
    }
}
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSnapshot {
    pub settings: SearchSettings,
    pub credential_version: aidex_vault::SecretVersion,
}
#[derive(Clone, Debug, Serialize)]
pub struct WebStatus {
    pub settings: SearchSettings,
    pub credential_present: bool,
    pub network_allowed: bool,
    pub fetch_available: bool,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchInput {
    pub query: String,
    #[serde(default = "default_count")]
    pub count: u32,
    #[serde(default)]
    pub freshness: Option<String>,
}
fn default_count() -> u32 {
    8
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FetchInput {
    pub url: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchOutput {
    pub query: String,
    pub results: Vec<SearchHit>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FetchOutput {
    pub url: String,
    pub content: String,
    pub truncated: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebError {
    Invalid,
    Offline,
    NotConfigured,
    CredentialUnavailable,
    Cancelled,
    Transport,
    TooLarge,
    Response,
    Http(u16),
    Conflict,
}
impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid public web request",
            Self::Offline => "offline mode blocks web access",
            Self::NotConfigured => "configure a web search connection in AiDEX settings",
            Self::CredentialUnavailable => "unlock the vault and reconnect web search",
            Self::Cancelled => "web request cancelled",
            Self::Transport => "web endpoint could not be reached",
            Self::TooLarge => "web response exceeds the configured limit",
            Self::Response => "web endpoint returned an unsupported response",
            Self::Http(401 | 403) => "web search credentials were rejected",
            Self::Http(429) => "web service rate limit reached",
            Self::Http(_) => "web endpoint rejected the request",
            Self::Conflict => "web search configuration changed after approval",
        })
    }
}
impl std::error::Error for WebError {}
pub type Result<T> = std::result::Result<T, WebError>;
