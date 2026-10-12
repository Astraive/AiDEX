//! Web settings: `private_config/web.toml` (closed schema, version 1).
//!
//! The file holds the provider choice, an optional SearXNG endpoint, and the
//! vault secret *name* of the provider key; never a key value. A missing file
//! means the defaults (web tools off, DuckDuckGo, five results, fetch on).

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use aidex_drive::DrivePaths;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{SearchProvider, WebError, target};

/// File name inside `DrivePaths::config()`.
pub const SETTINGS_FILE: &str = "web.toml";
/// Version of the settings file format.
pub const SETTINGS_VERSION: u32 = 1;
/// Most results one search returns.
pub const MAX_RESULTS: u8 = 10;
/// Results returned when the caller does not ask for a number.
pub const DEFAULT_MAX_RESULTS: u8 = 5;
/// Longest endpoint URL accepted.
const MAX_ENDPOINT_BYTES: usize = 2048;
/// Longest vault secret name (as in `aidex-vault`).
const MAX_SECRET_NAME: usize = 128;
/// Largest settings file read.
const MAX_FILE_BYTES: u64 = 16 * 1024;

fn default_provider() -> SearchProvider {
    SearchProvider::Duckduckgo
}

fn default_max_results() -> u8 {
    DEFAULT_MAX_RESULTS
}

fn default_true() -> bool {
    true
}

/// The drive's web search and fetch settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSettings {
    pub version: u32,
    /// Master switch: web tools are offered to agents only when on.
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_provider")]
    pub provider: SearchProvider,
    /// SearXNG base URL (`https://…`, or `http://` to a loopback host).
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Vault secret holding the provider key (consumer `process:web`).
    #[serde(default)]
    pub secret_name: Option<String>,
    /// Results per search, 1 to [`MAX_RESULTS`].
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    /// Whether `web.fetch` is offered alongside search.
    #[serde(default = "default_true")]
    pub fetch_enabled: bool,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            enabled: false,
            provider: default_provider(),
            endpoint: None,
            secret_name: None,
            max_results: DEFAULT_MAX_RESULTS,
            fetch_enabled: true,
        }
    }
}

impl WebSettings {
    /// `private_config/web.toml` on this drive.
    pub fn path(paths: &DrivePaths) -> PathBuf {
        paths.config().join(SETTINGS_FILE)
    }

    /// Check every field; messages never quote the secret name's value
    /// beyond its syntax.
    pub fn validate(&self) -> Result<(), WebError> {
        let invalid = |message: String| Err(WebError::InvalidSettings(message));
        if self.version != SETTINGS_VERSION {
            return invalid(format!(
                "version {} is not supported (expected {SETTINGS_VERSION})",
                self.version
            ));
        }
        if !(1..=MAX_RESULTS).contains(&self.max_results) {
            return invalid(format!("max_results must be 1 to {MAX_RESULTS}"));
        }
        if let Some(endpoint) = &self.endpoint {
            validate_endpoint(endpoint)?;
        }
        if let Some(name) = &self.secret_name {
            validate_secret_name(name)?;
            if !self.provider.accepts_key() {
                return invalid(format!("{} does not use an API key", self.provider));
            }
        }
        Ok(())
    }

    /// The URL a search with these settings contacts (before query parameters):
    /// the provider's built-in URL, or `<endpoint>/search` for a provider without
    /// one (SearXNG). An endpoint kept from an earlier provider choice is ignored,
    /// so policy checks and requests always name the same host.
    pub fn search_url(&self) -> Result<url::Url, WebError> {
        if let Some(url) = self.provider.default_url() {
            return url::Url::parse(url)
                .map_err(|_| WebError::Protocol("the built-in provider URL is invalid".into()));
        }
        let endpoint = self.endpoint.as_deref().ok_or_else(|| {
            WebError::NotConfigured(format!("{} needs an endpoint URL", self.provider))
        })?;
        target::parse_http_url(&format!("{}/search", endpoint.trim_end_matches('/')))
            .map_err(|error| WebError::InvalidSettings(format!("endpoint: {error}")))
    }

    /// Why a search with these settings cannot run yet, if anything is
    /// missing (the vault and network are not consulted).
    pub fn missing(&self) -> Option<WebError> {
        if self.provider.requires_endpoint() && self.endpoint.is_none() {
            return Some(WebError::NotConfigured(format!(
                "{} needs an endpoint URL",
                self.provider
            )));
        }
        if self.provider.requires_key() && self.secret_name.is_none() {
            return Some(WebError::NotConfigured(format!(
                "{} needs an API key: store it in the vault for `process:web` and set secret_name",
                self.provider
            )));
        }
        None
    }

    /// Load from the drive; a missing file yields the defaults.
    pub fn load(paths: &DrivePaths) -> Result<Self, WebError> {
        Self::load_from(&Self::path(paths))
    }

    /// Load from `path`; a missing file yields the defaults.
    pub fn load_from(path: &Path) -> Result<Self, WebError> {
        let display = || path.display().to_string();
        match fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(WebError::InvalidSettings(format!(
                    "{} must be a regular file, not a link or directory",
                    display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(WebError::Io(format!("{}: {error}", display()))),
        }
        let mut text = String::new();
        File::open(path)
            .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_string(&mut text))
            .map_err(|error| WebError::Io(format!("{}: {error}", display())))?;
        if text.len() as u64 > MAX_FILE_BYTES {
            return Err(WebError::InvalidSettings(format!(
                "{} is larger than 16 KiB",
                display()
            )));
        }
        let settings: Self = toml::from_str(&text).map_err(|error| {
            // `toml`'s Display quotes the offending line; report only the
            // position and the message without quoted values.
            let line = error
                .span()
                .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
            let message = strip_quoted(error.message());
            WebError::InvalidSettings(match line {
                Some(line) => format!("{} line {line}: {message}", display()),
                None => format!("{}: {message}", display()),
            })
        })?;
        settings.validate()?;
        Ok(settings)
    }

    /// Validate and atomically replace the drive's settings file.
    pub fn save(&self, paths: &DrivePaths) -> Result<(), WebError> {
        self.validate()?;
        let dir = paths
            .ensure_dir(&paths.config())
            .map_err(|error| WebError::Io(error.to_string()))?;
        self.save_to(&dir.join(SETTINGS_FILE))
    }

    /// Validate and atomically replace `path` (temp file, fsync, rename).
    pub fn save_to(&self, path: &Path) -> Result<(), WebError> {
        self.validate()?;
        let text = toml::to_string(self)
            .map_err(|error| WebError::Io(format!("cannot encode web settings: {error}")))?;
        write_atomic(path, text.as_bytes())
    }
}

fn strip_quoted(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut quoted = false;
    for ch in message.chars() {
        if ch == '"' {
            if quoted {
                out.push_str("\"…\"");
            }
            quoted = !quoted;
        } else if !quoted {
            out.push(ch);
        }
    }
    out.trim_end().to_owned()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), WebError> {
    let io_error = |error: io::Error| WebError::Io(format!("{}: {error}", path.display()));
    if let Ok(metadata) = fs::symlink_metadata(path)
        && !metadata.file_type().is_file()
    {
        return Err(WebError::InvalidSettings(format!(
            "{} must be a regular file, not a link or directory",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| WebError::Io(format!("{} has no parent directory", path.display())))?;
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix)
        .map_err(|error| WebError::Io(format!("randomness unavailable: {error}")))?;
    let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
    let temp = parent.join(format!(".{SETTINGS_FILE}.{suffix}.tmp"));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temp);
        return Err(io_error(error));
    }
    Ok(())
}

/// Validate a SearXNG base URL: http(s) as for fetches, no query or fragment.
pub fn validate_endpoint(endpoint: &str) -> Result<(), WebError> {
    let invalid = |message: &str| Err(WebError::InvalidSettings(format!("endpoint {message}")));
    if endpoint.len() > MAX_ENDPOINT_BYTES {
        return invalid("is longer than 2048 bytes");
    }
    let url = target::parse_http_url(endpoint)
        .map_err(|error| WebError::InvalidSettings(format!("endpoint: {error}")))?;
    if url.query().is_some() || endpoint.contains('#') {
        return invalid("must not have a query or fragment");
    }
    Ok(())
}

/// Validate a vault secret name (`[A-Za-z0-9._-]`, 1-128, starting
/// alphanumeric).
pub fn validate_secret_name(name: &str) -> Result<(), WebError> {
    let bytes = name.as_bytes();
    let valid = !bytes.is_empty()
        && bytes.len() <= MAX_SECRET_NAME
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(WebError::InvalidSettings(
            "secret_name must be 1-128 letters, digits, `.`, `_`, or `-`, starting with a letter or digit"
                .into(),
        ))
    }
}

/// A partial settings change: omitted fields stay; `null` clears `endpoint`
/// or `secret_name`. Unknown fields (such as a key value) are refused.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSettingsPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub provider: Option<SearchProvider>,
    #[serde(default, deserialize_with = "nullable")]
    pub endpoint: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable")]
    pub secret_name: Option<Option<String>>,
    #[serde(default)]
    pub max_results: Option<u8>,
    #[serde(default)]
    pub fetch_enabled: Option<bool>,
}

/// Distinguish an absent field (`None`) from an explicit `null` (`Some(None)`).
fn nullable<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

impl WebSettingsPatch {
    /// The patched settings, validated. Switching to another provider clears
    /// `secret_name` unless the patch names one (a key belongs to the
    /// provider it was issued by, so it never follows a provider change);
    /// empty strings clear optional fields.
    pub fn apply(&self, current: &WebSettings) -> Result<WebSettings, WebError> {
        let mut next = current.clone();
        if let Some(enabled) = self.enabled {
            next.enabled = enabled;
        }
        if let Some(provider) = self.provider {
            if provider != current.provider && self.secret_name.is_none() {
                next.secret_name = None;
            }
            next.provider = provider;
        }
        if let Some(endpoint) = &self.endpoint {
            next.endpoint = endpoint
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(|text| text.trim_end_matches('/').to_owned());
        }
        if let Some(name) = &self.secret_name {
            next.secret_name = name
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned);
        }
        if let Some(max) = self.max_results {
            next.max_results = max;
        }
        if let Some(fetch) = self.fetch_enabled {
            next.fetch_enabled = fetch;
        }
        next.validate()?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_files_load_defaults_and_saves_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        assert_eq!(
            WebSettings::load_from(&path).unwrap(),
            WebSettings::default()
        );
        let settings = WebSettings {
            enabled: true,
            provider: SearchProvider::Searxng,
            endpoint: Some("https://searx.example.org".into()),
            secret_name: Some("web.searx-token".into()),
            max_results: 7,
            fetch_enabled: false,
            ..WebSettings::default()
        };
        settings.save_to(&path).unwrap();
        assert_eq!(WebSettings::load_from(&path).unwrap(), settings);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("provider = \"searxng\""), "{text}");
        // Defaults omit the optional fields entirely.
        WebSettings::default().save_to(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            !text.contains("endpoint") && !text.contains("secret_name"),
            "{text}"
        );
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers.len(), 1, "{leftovers:?}");
    }

    #[test]
    fn the_schema_is_closed_and_errors_do_not_quote_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        fs::write(&path, "version = 1\napi_key = \"sk-pasted-secret\"\n").unwrap();
        let error = WebSettings::load_from(&path).unwrap_err().to_string();
        assert!(error.contains("line 2"), "{error}");
        assert!(!error.contains("sk-pasted-secret"), "{error}");
        for text in [
            "version = 2\n",
            "version = 1\nmax_results = 0\n",
            "version = 1\nmax_results = 11\n",
            "version = 1\nprovider = \"google\"\n",
            "version = 1\nprovider = \"searxng\"\nendpoint = \"http://searx.example\"\n",
            "version = 1\nprovider = \"duckduckgo\"\nsecret_name = \"k\"\n",
            "version = 1\nsecret_name = \"has space\"\n",
        ] {
            fs::write(&path, text).unwrap();
            assert!(
                matches!(
                    WebSettings::load_from(&path),
                    Err(WebError::InvalidSettings(_))
                ),
                "{text}"
            );
        }
    }

    #[test]
    fn patches_keep_omitted_fields_and_null_clears() {
        let current = WebSettings {
            provider: SearchProvider::Brave,
            secret_name: Some("brave-key".into()),
            ..WebSettings::default()
        };
        let patch: WebSettingsPatch =
            serde_json::from_value(serde_json::json!({"enabled": true, "max_results": 3})).unwrap();
        let next = patch.apply(&current).unwrap();
        assert!(next.enabled);
        assert_eq!(next.max_results, 3);
        assert_eq!(next.secret_name.as_deref(), Some("brave-key"));

        let patch: WebSettingsPatch =
            serde_json::from_value(serde_json::json!({"secret_name": null})).unwrap();
        assert_eq!(patch.apply(&current).unwrap().secret_name, None);

        let patch: WebSettingsPatch =
            serde_json::from_value(serde_json::json!({"provider": "duckduckgo"})).unwrap();
        assert_eq!(patch.apply(&current).unwrap().secret_name, None);

        let patch: WebSettingsPatch = serde_json::from_value(
            serde_json::json!({"provider": "searxng", "endpoint": "http://localhost:8888/"}),
        )
        .unwrap();
        let next = patch.apply(&current).unwrap();
        assert_eq!(next.endpoint.as_deref(), Some("http://localhost:8888"));
        assert_eq!(
            next.secret_name, None,
            "a key never follows a provider change"
        );

        // Re-selecting the same provider, or naming a key, keeps or sets it.
        let patch: WebSettingsPatch =
            serde_json::from_value(serde_json::json!({"provider": "brave"})).unwrap();
        assert_eq!(
            patch.apply(&current).unwrap().secret_name.as_deref(),
            Some("brave-key")
        );
        let patch: WebSettingsPatch = serde_json::from_value(
            serde_json::json!({"provider": "exa", "secret_name": "exa-key"}),
        )
        .unwrap();
        assert_eq!(
            patch.apply(&current).unwrap().secret_name.as_deref(),
            Some("exa-key")
        );

        assert!(
            serde_json::from_value::<WebSettingsPatch>(serde_json::json!({"api_key": "sk-123"}))
                .is_err()
        );
        let patch: WebSettingsPatch =
            serde_json::from_value(serde_json::json!({"max_results": 42})).unwrap();
        assert!(patch.apply(&current).is_err());
    }

    #[test]
    fn missing_reports_what_a_search_still_needs() {
        let mut settings = WebSettings {
            provider: SearchProvider::Tavily,
            ..WebSettings::default()
        };
        assert!(matches!(
            settings.missing(),
            Some(WebError::NotConfigured(_))
        ));
        settings.secret_name = Some("tavily".into());
        assert_eq!(settings.missing(), None);
        settings.provider = SearchProvider::Searxng;
        assert!(settings.missing().is_some());
        settings.endpoint = Some("https://searx.example".into());
        assert_eq!(settings.missing(), None);
        assert_eq!(WebSettings::default().missing(), None);
    }

    #[test]
    fn search_urls_ignore_endpoints_of_hosted_providers() {
        let mut settings = WebSettings {
            provider: SearchProvider::Searxng,
            endpoint: Some("http://localhost:8888/".into()),
            ..WebSettings::default()
        };
        assert_eq!(
            settings.search_url().unwrap().as_str(),
            "http://localhost:8888/search"
        );
        // An endpoint left from a SearXNG setup is not where a hosted provider's
        // searches go.
        for provider in SearchProvider::ALL {
            if let Some(built_in) = provider.default_url() {
                settings.provider = provider;
                assert_eq!(settings.search_url().unwrap().as_str(), built_in);
            }
        }
        settings.provider = SearchProvider::Searxng;
        settings.endpoint = None;
        assert!(matches!(
            settings.search_url(),
            Err(WebError::NotConfigured(_))
        ));
    }
}
