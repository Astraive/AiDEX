//! [`WebClient`]: configured search and guarded fetch.
//!
//! The client owns no policy. It is built from [`WebSettings`], a credential
//! resolver (the daemon releases a vault secret for the consumer
//! `process:web`), and an egress check (the daemon consults the offline switch
//! and the broker's network policy). The egress check runs before every
//! connection, including each redirect hop, and before any DNS lookup, so a
//! refusal opens no socket.

use std::{fmt, sync::Arc, time::Duration};

use aidex_core::CancelToken;
use serde::{Deserialize, Serialize};
use url::Url;
use zeroize::Zeroizing;

use crate::{
    SearchProvider, WebError, WebSettings, fetch,
    http::{self, Limits, Response, Timeouts},
    search,
    target::{self, Destination, DnsResolver, LocalTargets, Origin},
};

/// A provider key released for one request. Never printed.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    /// A secret from raw vault bytes; it must be UTF-8 without control
    /// characters (it travels in an HTTP header or JSON body).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, WebError> {
        let text = std::str::from_utf8(bytes).map_err(|_| {
            WebError::credential(
                crate::CredentialProblem::Invalid,
                "the stored key is not UTF-8 text",
            )
        })?;
        let text = text.trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(WebError::credential(
                crate::CredentialProblem::Invalid,
                "the stored key is empty or contains control characters",
            ));
        }
        Ok(Self::new(text.to_owned()))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// What the credential resolver is asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CredentialRequest<'a> {
    pub provider: SearchProvider,
    /// The vault secret name from the settings.
    pub secret_name: &'a str,
}

/// Why a connection is about to be made.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EgressPurpose {
    Search(SearchProvider),
    Fetch,
}

/// A connection the egress check must approve before any I/O.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EgressRequest<'a> {
    pub purpose: EgressPurpose,
    pub scheme: &'a str,
    /// Lowercase host name or IP literal (IPv6 without brackets).
    pub host: &'a str,
    pub port: u16,
    /// 0 for the first request, then the redirect hop number.
    pub hop: usize,
}

/// Releases a provider key (the daemon resolves a vault handle).
pub type CredentialResolver =
    Arc<dyn Fn(&CredentialRequest<'_>) -> Result<Secret, WebError> + Send + Sync>;
/// Approves or refuses a connection (offline switch, network policy).
pub type EgressCheck = Arc<dyn Fn(&EgressRequest<'_>) -> Result<(), WebError> + Send + Sync>;

/// Timeouts and size limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebOptions {
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub body_timeout: Duration,
    pub dns_timeout: Duration,
    /// Deadline for a whole search or fetch, redirects included.
    pub request_timeout: Duration,
    /// Raw response bytes read (fetch bodies beyond it are truncated; API
    /// responses beyond it fail).
    pub max_raw_bytes: usize,
    /// Extracted text kept per page.
    pub max_text_bytes: usize,
    pub max_redirects: usize,
    /// Explicitly local destinations a fetch may reach.
    pub local_targets: LocalTargets,
}

/// Default raw response cap (2 MiB).
pub const MAX_RAW_BYTES: usize = 2 * 1024 * 1024;
/// Default extracted text cap (256 KiB).
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// Default redirect limit.
pub const MAX_REDIRECTS: usize = 5;

impl Default for WebOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            response_timeout: Duration::from_secs(20),
            body_timeout: Duration::from_secs(30),
            dns_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
            max_raw_bytes: MAX_RAW_BYTES,
            max_text_bytes: MAX_TEXT_BYTES,
            max_redirects: MAX_REDIRECTS,
            local_targets: LocalTargets::Refuse,
        }
    }
}

/// One search result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    pub source: SearchProvider,
}

/// A fetched page reduced to readable text.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FetchedPage {
    /// The URL as requested (normalized, without fragment).
    pub url: String,
    /// The URL after redirects.
    pub final_url: String,
    pub status: u16,
    /// Media type without parameters.
    pub content_type: String,
    pub title: Option<String>,
    pub text: String,
    /// The body or the text reached its size limit.
    pub truncated: bool,
}

/// Web search and fetch for one settings snapshot.
pub struct WebClient {
    settings: WebSettings,
    credentials: CredentialResolver,
    egress: EgressCheck,
    options: WebOptions,
    dns: DnsResolver,
    search_url: Option<Url>,
}

impl fmt::Debug for WebClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebClient")
            .field("provider", &self.settings.provider)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

/// A destination approved by the URL rules and the egress check, not yet
/// resolved or connected.
pub(crate) struct Approved {
    destination: Destination,
    origin: Origin,
}

impl Approved {
    /// Whether the URL names an explicitly local destination (an IP literal
    /// that is not public, or `localhost`).
    pub(crate) fn is_explicitly_local(&self) -> bool {
        self.destination.is_explicitly_local()
    }
}

/// What to send to an approved destination.
pub(crate) struct Payload {
    /// `(content type, body)` for a POST; `None` sends a GET. The body may
    /// hold a credential and is zeroized once sent.
    pub post: Option<(&'static str, Zeroizing<Vec<u8>>)>,
    /// Extra headers; values may be credentials and are zeroized once sent.
    pub headers: Vec<(&'static str, Zeroizing<String>)>,
    pub accept: &'static str,
    pub limits: Limits,
}

impl WebClient {
    /// A client for `settings` (validated), releasing keys through
    /// `credentials` and asking `egress` before every connection.
    pub fn new<C, E>(settings: WebSettings, credentials: C, egress: E) -> Result<Self, WebError>
    where
        C: Fn(&CredentialRequest<'_>) -> Result<Secret, WebError> + Send + Sync + 'static,
        E: Fn(&EgressRequest<'_>) -> Result<(), WebError> + Send + Sync + 'static,
    {
        settings.validate()?;
        Ok(Self {
            settings,
            credentials: Arc::new(credentials),
            egress: Arc::new(egress),
            options: WebOptions::default(),
            dns: target::system_dns(),
            search_url: None,
        })
    }

    #[must_use]
    pub fn with_options(mut self, options: WebOptions) -> Self {
        self.options = options;
        self
    }

    /// Resolve host names through `dns` instead of the system resolver.
    #[must_use]
    pub fn with_dns_resolver(mut self, dns: DnsResolver) -> Self {
        self.dns = dns;
        self
    }

    /// Send searches to `url` instead of the provider's built-in URL (for a
    /// self-hosted mirror or a test server). The URL follows the fetch URL
    /// rules and is treated as user-configured, so it may be local.
    pub fn with_search_url(mut self, url: &str) -> Result<Self, WebError> {
        self.search_url = Some(target::parse_http_url(url)?);
        Ok(self)
    }

    pub fn settings(&self) -> &WebSettings {
        &self.settings
    }

    pub fn options(&self) -> &WebOptions {
        &self.options
    }

    /// Search with the configured provider: at most `max_results` (1-10;
    /// default from the settings) results. The `enabled` switch is not
    /// consulted here (it decides whether tools are offered), so a settings
    /// screen can test a provider before turning web search on.
    pub fn search(
        &self,
        query: &str,
        max_results: Option<usize>,
        cancel: &CancelToken,
    ) -> Result<Vec<SearchResult>, WebError> {
        let cancel = cancel.with_timeout(self.options.request_timeout);
        search::run(self, query, max_results, &cancel)
    }

    /// Fetch `url` and reduce it to readable text, following at most
    /// [`WebOptions::max_redirects`] redirects, each re-validated and
    /// re-approved by the egress check.
    pub fn fetch(&self, url: &str, cancel: &CancelToken) -> Result<FetchedPage, WebError> {
        let cancel = cancel.with_timeout(self.options.request_timeout);
        fetch::run(self, url, &cancel)
    }

    pub(crate) fn search_url(&self) -> Option<&Url> {
        self.search_url.as_ref()
    }

    pub(crate) fn credential(&self, secret_name: &str) -> Result<Secret, WebError> {
        (self.credentials)(&CredentialRequest {
            provider: self.settings.provider,
            secret_name,
        })
    }

    pub(crate) fn timeouts(&self) -> Timeouts {
        Timeouts {
            connect: self.options.connect_timeout,
            response: self.options.response_timeout,
            body: self.options.body_timeout,
        }
    }

    /// Validate `url` for `origin` and ask the egress check, before any DNS
    /// lookup or connection (a refusal opens no socket).
    pub(crate) fn approve(
        &self,
        purpose: EgressPurpose,
        url: Url,
        origin: Origin,
        hop: usize,
        cancel: &CancelToken,
    ) -> Result<Approved, WebError> {
        http::check_cancel(cancel)?;
        let destination = Destination::check(url, origin)?;
        let host = destination.host.display();
        (self.egress)(&EgressRequest {
            purpose,
            scheme: destination.url.scheme(),
            host: &host,
            port: destination.port,
            hop,
        })?;
        Ok(Approved {
            destination,
            origin,
        })
    }

    /// Resolve and pin the approved destination's addresses, then run one
    /// exchange.
    pub(crate) fn exchange(
        &self,
        approved: Approved,
        payload: Payload,
        cancel: &CancelToken,
    ) -> Result<Response, WebError> {
        let Approved {
            destination,
            origin,
        } = approved;
        let addrs = destination.resolve(origin, &self.dns, self.options.dns_timeout, cancel)?;
        http::exchange(
            http::Request {
                url: destination.url,
                addrs,
                post: payload.post,
                headers: payload.headers,
                accept: payload.accept,
            },
            payload.limits,
            self.timeouts(),
            cancel,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_never_print_and_must_be_header_safe() {
        let secret = Secret::from_bytes(b"  sk-test-value \n").unwrap();
        assert_eq!(secret.expose(), "sk-test-value");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        for bad in [&b"\xff\xfe"[..], b"", b"a\x00b", b"line\nbreak"] {
            assert!(matches!(
                Secret::from_bytes(bad),
                Err(WebError::Credential { .. })
            ));
        }
    }

    #[test]
    fn clients_validate_their_settings() {
        let invalid = WebSettings {
            max_results: 0,
            ..WebSettings::default()
        };
        assert!(
            WebClient::new(
                invalid,
                |_: &CredentialRequest<'_>| Err(WebError::Cancelled),
                |_: &EgressRequest<'_>| Ok(())
            )
            .is_err()
        );
        let client = WebClient::new(
            WebSettings::default(),
            |_: &CredentialRequest<'_>| Err(WebError::Cancelled),
            |_: &EgressRequest<'_>| Ok(()),
        )
        .unwrap();
        let shown = format!("{client:?}");
        assert!(shown.contains("Duckduckgo"), "{shown}");
        assert!(client.with_search_url("ftp://x/").is_err());
    }
}
