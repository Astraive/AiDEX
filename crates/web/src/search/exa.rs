//! Exa: `POST /search` with the key in the `x-api-key` header, asking for
//! short highlights as snippets.

use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::{ProviderRequest, RawResult, array, json_body, optional_text, text};
use crate::{SearchProvider, WebError, client::Secret};

pub(super) fn request(
    query: &str,
    count: usize,
    key: &Secret,
) -> Result<ProviderRequest, WebError> {
    let body = json!({
        "query": query,
        "numResults": count,
        "type": "auto",
        "contents": {
            "highlights": { "numSentences": 2, "highlightsPerUrl": 1 },
        },
    });
    Ok(ProviderRequest {
        post: Some(("application/json", json_body(&body, 256 + query.len() * 6)?)),
        headers: vec![("x-api-key", Zeroizing::new(key.expose().to_owned()))],
        accept: "application/json",
    })
}

pub(super) fn parse(body: &Value) -> Result<Vec<RawResult>, WebError> {
    Ok(array(body, &["results"], SearchProvider::Exa)?
        .iter()
        .map(|item| RawResult {
            title: text(item, "title").to_owned(),
            url: text(item, "url").to_owned(),
            snippet: snippet(item),
            published: optional_text(item, "publishedDate"),
        })
        .collect())
}

/// Highlights, else the summary, else the start of the page text.
fn snippet(item: &Value) -> String {
    let highlights: Vec<&str> = item
        .get("highlights")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !highlights.is_empty() {
        return highlights.join(" \u{2026} ");
    }
    optional_text(item, "summary")
        .or_else(|| optional_text(item, "text"))
        .unwrap_or_default()
}
