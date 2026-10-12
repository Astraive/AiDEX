//! The built-in search provider catalog.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// A supported web search provider.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchProvider {
    Brave,
    Tavily,
    Exa,
    Searxng,
    Duckduckgo,
}

impl SearchProvider {
    pub const ALL: [Self; 5] = [
        Self::Brave,
        Self::Tavily,
        Self::Exa,
        Self::Searxng,
        Self::Duckduckgo,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Brave => "brave",
            Self::Tavily => "tavily",
            Self::Exa => "exa",
            Self::Searxng => "searxng",
            Self::Duckduckgo => "duckduckgo",
        }
    }

    /// Whether searches need an API key from the vault.
    pub const fn requires_key(self) -> bool {
        matches!(self, Self::Brave | Self::Tavily | Self::Exa)
    }

    /// Whether the provider can send a key at all (SearXNG optionally sends one
    /// as a bearer token for instances behind an authenticating proxy).
    pub const fn accepts_key(self) -> bool {
        !matches!(self, Self::Duckduckgo)
    }

    /// Whether the user must configure the endpoint (SearXNG instances).
    pub const fn requires_endpoint(self) -> bool {
        matches!(self, Self::Searxng)
    }

    /// The fixed search URL of a hosted provider (`None` for SearXNG).
    pub const fn default_url(self) -> Option<&'static str> {
        match self {
            Self::Brave => Some("https://api.search.brave.com/res/v1/web/search"),
            Self::Tavily => Some("https://api.tavily.com/search"),
            Self::Exa => Some("https://api.exa.ai/search"),
            Self::Searxng => None,
            Self::Duckduckgo => Some("https://html.duckduckgo.com/html/"),
        }
    }

    /// The catalog entry for this provider.
    pub fn info(self) -> ProviderInfo {
        let (display_name, description, key_url, docs_url) = match self {
            Self::Brave => (
                "Brave Search",
                "Independent web index through the Brave Search API. Needs an API key.",
                Some("https://api-dashboard.search.brave.com/app/keys"),
                "https://api-dashboard.search.brave.com/app/documentation/web-search/get-started",
            ),
            Self::Tavily => (
                "Tavily",
                "Search API built for agents, with extracted page content. Needs an API key.",
                Some("https://app.tavily.com/home"),
                "https://docs.tavily.com/",
            ),
            Self::Exa => (
                "Exa",
                "Neural search API with page highlights. Needs an API key.",
                Some("https://dashboard.exa.ai/api-keys"),
                "https://docs.exa.ai/",
            ),
            Self::Searxng => (
                "SearXNG",
                "Your own SearXNG instance with the JSON format enabled. No key needed.",
                None,
                "https://docs.searxng.org/",
            ),
            Self::Duckduckgo => (
                "DuckDuckGo",
                "DuckDuckGo's HTML results page. No key needed; results may be limited.",
                None,
                "https://duckduckgo.com/",
            ),
        };
        ProviderInfo {
            id: self,
            display_name,
            icon: self.as_str(),
            description,
            requires_key: self.requires_key(),
            accepts_key: self.accepts_key(),
            requires_endpoint: self.requires_endpoint(),
            endpoint: self.default_url(),
            key_url,
            docs_url,
        }
    }
}

impl fmt::Display for SearchProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SearchProvider {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|provider| provider.as_str() == text)
            .ok_or_else(|| {
                "provider must be one of brave, tavily, exa, searxng, duckduckgo".to_owned()
            })
    }
}

/// One catalog entry, as shown by the settings screen.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProviderInfo {
    pub id: SearchProvider,
    pub display_name: &'static str,
    /// Brand icon key (`brave`, `tavily`, `exa`, `searxng`, `duckduckgo`).
    pub icon: &'static str,
    pub description: &'static str,
    pub requires_key: bool,
    pub accepts_key: bool,
    pub requires_endpoint: bool,
    /// The fixed search URL of a hosted provider.
    pub endpoint: Option<&'static str>,
    /// Where the user creates an API key.
    pub key_url: Option<&'static str>,
    pub docs_url: &'static str,
}

/// Every provider in presentation order.
pub fn catalog() -> Vec<ProviderInfo> {
    SearchProvider::ALL
        .into_iter()
        .map(SearchProvider::info)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lists_every_provider_with_https_links_and_icon_keys() {
        let entries = catalog();
        assert_eq!(entries.len(), SearchProvider::ALL.len());
        for entry in &entries {
            assert_eq!(entry.icon, entry.id.as_str());
            assert!(entry.docs_url.starts_with("https://"));
            assert_eq!(entry.key_url.is_some(), entry.requires_key, "{}", entry.id);
            for url in entry.key_url.iter().chain(entry.endpoint.iter()) {
                assert!(url.starts_with("https://"), "{url}");
            }
            assert_eq!(entry.id.as_str().parse::<SearchProvider>(), Ok(entry.id));
        }
        assert!("google".parse::<SearchProvider>().is_err());
        assert_eq!(
            serde_json::to_value(SearchProvider::Duckduckgo).unwrap(),
            "duckduckgo"
        );
    }
}
