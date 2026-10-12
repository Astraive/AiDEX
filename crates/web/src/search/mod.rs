//! Search with the configured provider.
//!
//! Each provider module builds its request and parses its response; this
//! module validates the query, approves the destination (before any key is
//! released), maps HTTP outcomes, and sanitizes results: plain-text titles and
//! snippets, absolute `http(s)` URLs without credentials, no duplicates, and
//! at most the requested count.

mod brave;
mod duckduckgo;
mod exa;
mod searxng;
mod tavily;

use std::collections::HashSet;

use aidex_core::CancelToken;
use serde_json::Value;
use url::Url;
use zeroize::Zeroizing;

use crate::{
    SearchProvider, SearchResult, WebClient, WebError,
    client::{EgressPurpose, Payload, Secret},
    html::{plain_text, truncate_chars},
    http::{ContentRule, Limits, Overflow, Response},
    settings::MAX_RESULTS,
    target::{MAX_URL_BYTES, Origin},
};

/// Longest query accepted, in characters.
pub const MAX_QUERY_CHARS: usize = 400;
/// Longest result title kept, in characters.
const MAX_TITLE_CHARS: usize = 300;
/// Longest result snippet kept, in characters.
const MAX_SNIPPET_CHARS: usize = 1000;
/// Longest `published` value kept, in characters.
const MAX_PUBLISHED_CHARS: usize = 64;

/// A provider's request body and headers (the URL is built and approved
/// first, before any key is released).
pub(crate) struct ProviderRequest {
    pub post: Option<(&'static str, Zeroizing<Vec<u8>>)>,
    pub headers: Vec<(&'static str, Zeroizing<String>)>,
    pub accept: &'static str,
}

/// One result as a provider reports it, before sanitizing.
#[derive(Debug, Default)]
pub(crate) struct RawResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published: Option<String>,
}

pub(crate) fn run(
    client: &WebClient,
    query: &str,
    max_results: Option<usize>,
    cancel: &CancelToken,
) -> Result<Vec<SearchResult>, WebError> {
    let query = normalize_query(query)?;
    let settings = client.settings();
    let count = match max_results {
        None => usize::from(settings.max_results),
        Some(count) if (1..=usize::from(MAX_RESULTS)).contains(&count) => count,
        Some(_) => {
            return Err(WebError::InvalidRequest(format!(
                "max_results must be 1 to {MAX_RESULTS}"
            )));
        }
    };
    if let Some(problem) = settings.missing() {
        return Err(problem);
    }
    let provider = settings.provider;
    let (base, origin) = search_url(client, provider)?;
    let url = request_url(provider, base, &query, count);
    let approved = client.approve(EgressPurpose::Search(provider), url, origin, 0, cancel)?;
    // The key is released only after the destination was approved.
    let key = match settings.secret_name.as_deref() {
        Some(name) if provider.accepts_key() => Some(client.credential(name)?),
        _ => None,
    };
    let request = build(provider, &query, count, key.as_ref())?;
    drop(key);
    let response = client.exchange(
        approved,
        Payload {
            post: request.post,
            headers: request.headers,
            accept: request.accept,
            limits: Limits {
                max_body: client.options().max_raw_bytes,
                overflow: Overflow::Fail,
                content: ContentRule::Any,
            },
        },
        cancel,
    )?;
    check_status(provider, &response)?;
    let raw = parse(provider, &response.body)?;
    Ok(finish(provider, raw, count))
}

/// Trim and collapse whitespace; refuse empty, overlong, or control-laden
/// queries.
fn normalize_query(query: &str) -> Result<String, WebError> {
    if query
        .chars()
        .any(|ch| ch.is_control() && !ch.is_whitespace())
    {
        return Err(WebError::InvalidRequest(
            "the query contains control characters".into(),
        ));
    }
    let query = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if query.is_empty() {
        return Err(WebError::InvalidRequest("the query is empty".into()));
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(WebError::InvalidRequest(format!(
            "the query is longer than {MAX_QUERY_CHARS} characters"
        )));
    }
    Ok(query)
}

/// The URL a search goes to and who chose it.
fn search_url(client: &WebClient, provider: SearchProvider) -> Result<(Url, Origin), WebError> {
    if let Some(url) = client.search_url() {
        return Ok((url.clone(), Origin::Configured));
    }
    // The same URL the harness policy-checks (`WebSettings::search_url`).
    let url = client.settings().search_url()?;
    let origin = if provider.default_url().is_some() {
        Origin::BuiltIn
    } else {
        Origin::Configured
    };
    Ok((url, origin))
}

/// The search URL with the provider's query parameters.
fn request_url(provider: SearchProvider, base: Url, query: &str, count: usize) -> Url {
    match provider {
        SearchProvider::Brave => brave::url(base, query, count),
        SearchProvider::Searxng => searxng::url(base, query),
        SearchProvider::Duckduckgo => duckduckgo::url(base, query),
        SearchProvider::Tavily | SearchProvider::Exa => base,
    }
}

fn build(
    provider: SearchProvider,
    query: &str,
    count: usize,
    key: Option<&Secret>,
) -> Result<ProviderRequest, WebError> {
    let missing_key = || WebError::NotConfigured(format!("{provider} needs an API key"));
    match provider {
        SearchProvider::Brave => Ok(brave::request(key.ok_or_else(missing_key)?)),
        SearchProvider::Tavily => tavily::request(query, count, key.ok_or_else(missing_key)?),
        SearchProvider::Exa => exa::request(query, count, key.ok_or_else(missing_key)?),
        SearchProvider::Searxng => Ok(searxng::request(key)),
        SearchProvider::Duckduckgo => Ok(duckduckgo::request()),
    }
}

fn parse(provider: SearchProvider, body: &[u8]) -> Result<Vec<RawResult>, WebError> {
    match provider {
        SearchProvider::Duckduckgo => duckduckgo::parse(body),
        SearchProvider::Brave => brave::parse(&json(provider, body)?),
        SearchProvider::Tavily => tavily::parse(&json(provider, body)?),
        SearchProvider::Exa => exa::parse(&json(provider, body)?),
        SearchProvider::Searxng => searxng::parse(&json(provider, body)?),
    }
}

fn json(provider: SearchProvider, body: &[u8]) -> Result<Value, WebError> {
    serde_json::from_slice(body)
        .map_err(|_| WebError::Protocol(format!("{provider} returned a response that is not JSON")))
}

/// Map a non-success HTTP status to a first-class error. Bodies are never
/// quoted (a provider may echo request data).
fn check_status(provider: SearchProvider, response: &Response) -> Result<(), WebError> {
    let status = response.status;
    if provider == SearchProvider::Duckduckgo && status == 202 {
        return Err(WebError::RateLimited(
            "DuckDuckGo asked for a bot check; try again later or use another provider".into(),
        ));
    }
    match status {
        200..=299 => Ok(()),
        _ if response.is_redirect() => Err(WebError::Protocol(format!(
            "{provider} answered with an unexpected redirect (HTTP {status})"
        ))),
        401 | 403 if provider == SearchProvider::Searxng => Err(WebError::Http {
            status,
            message: "the SearXNG instance refused the request; check that its `json` format is \
                      enabled and that it accepts this client"
                .into(),
        }),
        401 | 403 => Err(WebError::Unauthorized(format!(
            "{provider} refused the API key (HTTP {status})"
        ))),
        429 => Err(WebError::RateLimited(format!(
            "{provider} is rate limiting searches (HTTP 429)"
        ))),
        _ => Err(WebError::Http {
            status,
            message: format!("{provider} could not complete the search"),
        }),
    }
}

/// Sanitize, de-duplicate, and cut provider results.
fn finish(provider: SearchProvider, raw: Vec<RawResult>, count: usize) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    let mut results = Vec::new();
    for item in raw {
        if results.len() >= count {
            break;
        }
        let Some(url) = result_url(&item.url) else {
            continue;
        };
        if !seen.insert(url.as_str().to_owned()) {
            continue;
        }
        let mut title = plain_text(&item.title, MAX_TITLE_CHARS);
        if title.is_empty() {
            title = url.host_str().unwrap_or_default().to_owned();
        }
        let published = item
            .published
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|text| !text.is_empty())
            .map(|text| truncate_chars(&text, MAX_PUBLISHED_CHARS));
        results.push(SearchResult {
            title,
            url: url.into(),
            snippet: plain_text(&item.snippet, MAX_SNIPPET_CHARS),
            published,
            source: provider,
        });
    }
    results
}

/// An absolute `http(s)` URL with a host and no credentials, or `None`.
fn result_url(text: &str) -> Option<Url> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_URL_BYTES || text.chars().any(char::is_control) {
        return None;
    }
    let url = Url::parse(text).ok()?;
    let acceptable = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some_and(|host| !host.is_empty())
        && url.username().is_empty()
        && url.password().is_none();
    acceptable.then_some(url)
}

/// A string field of a JSON object, or `""`.
pub(crate) fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// An optional non-empty string field of a JSON object.
pub(crate) fn optional_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

/// The array at `path` (object keys), or a protocol error naming the provider.
pub(crate) fn array<'a>(
    value: &'a Value,
    path: &[&str],
    provider: SearchProvider,
) -> Result<&'a [Value], WebError> {
    let unexpected = || WebError::Protocol(format!("{provider} returned an unexpected response"));
    let mut current = value;
    for key in path {
        let Value::Object(object) = current else {
            return Err(unexpected());
        };
        match object.get(*key) {
            Some(next) => current = next,
            // A missing results container means no results, not an error.
            None => return Ok(&[]),
        }
    }
    match current {
        Value::Array(items) => Ok(items),
        Value::Null => Ok(&[]),
        _ => Err(unexpected()),
    }
}

/// A JSON request body; capacity is reserved up front so a key in the body is
/// never left behind in a reallocated buffer.
pub(crate) fn json_body(value: &Value, capacity: usize) -> Result<Zeroizing<Vec<u8>>, WebError> {
    let mut body = Zeroizing::new(Vec::with_capacity(capacity));
    serde_json::to_writer(&mut *body, value)
        .map_err(|_| WebError::InvalidRequest("the search request cannot be encoded".into()))?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_trimmed_bounded_and_printable() {
        assert_eq!(
            normalize_query("  rust \n  web\tsearch ").unwrap(),
            "rust web search"
        );
        for bad in ["", "   ", "a\u{0}b", "bell\u{7}"] {
            assert!(matches!(
                normalize_query(bad),
                Err(WebError::InvalidRequest(_))
            ));
        }
        assert!(normalize_query(&"q".repeat(MAX_QUERY_CHARS)).is_ok());
        assert!(normalize_query(&"q".repeat(MAX_QUERY_CHARS + 1)).is_err());
    }

    #[test]
    fn results_are_sanitized_deduplicated_and_cut() {
        let raw = vec![
            RawResult {
                title: "<b>Rust</b> &amp; friends".into(),
                url: "https://www.rust-lang.org/".into(),
                snippet: "A <strong>language</strong>\n empowering everyone".into(),
                published: Some("  2 days   ago ".into()),
            },
            RawResult {
                title: "duplicate".into(),
                url: "https://www.rust-lang.org/".into(),
                ..RawResult::default()
            },
            RawResult {
                title: "script".into(),
                url: "javascript:alert(1)".into(),
                ..RawResult::default()
            },
            RawResult {
                title: "creds".into(),
                url: "https://user:pw@example.com/".into(),
                ..RawResult::default()
            },
            RawResult {
                title: String::new(),
                url: "https://docs.rs/serde".into(),
                ..RawResult::default()
            },
            RawResult {
                title: "over the count".into(),
                url: "https://crates.io/".into(),
                ..RawResult::default()
            },
        ];
        let results = finish(SearchProvider::Brave, raw, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust & friends");
        assert_eq!(results[0].snippet, "A language empowering everyone");
        assert_eq!(results[0].published.as_deref(), Some("2 days ago"));
        assert_eq!(results[0].source, SearchProvider::Brave);
        assert_eq!(results[1].title, "docs.rs");
        assert_eq!(results[1].url, "https://docs.rs/serde");
    }

    #[test]
    fn json_arrays_are_found_defensively() {
        let value = serde_json::json!({"web": {"results": [1, 2]}, "other": 3});
        assert_eq!(
            array(&value, &["web", "results"], SearchProvider::Brave)
                .unwrap()
                .len(),
            2
        );
        assert!(
            array(&value, &["missing", "results"], SearchProvider::Brave)
                .unwrap()
                .is_empty()
        );
        assert!(array(&value, &["other"], SearchProvider::Brave).is_err());
        assert!(array(&serde_json::json!([]), &["results"], SearchProvider::Exa).is_err());
    }
}
