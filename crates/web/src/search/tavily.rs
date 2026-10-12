//! Tavily: `POST /search` with a JSON body carrying `api_key`.

use serde_json::{Value, json};

use super::{ProviderRequest, RawResult, array, json_body, optional_text, text};
use crate::{SearchProvider, WebError, client::Secret};

pub(super) fn request(
    query: &str,
    count: usize,
    key: &Secret,
) -> Result<ProviderRequest, WebError> {
    let mut body = json!({
        "api_key": key.expose(),
        "query": query,
        "max_results": count,
        "search_depth": "basic",
        "include_answer": false,
        "include_images": false,
    });
    let capacity = 256 + query.len() * 6 + key.expose().len() * 6;
    let encoded = json_body(&body, capacity);
    // The JSON value holds a copy of the key; clear it before it drops.
    clear_strings(&mut body);
    Ok(ProviderRequest {
        post: Some(("application/json", encoded?)),
        headers: Vec::new(),
        accept: "application/json",
    })
}

/// Overwrite every string in `value` (one is the copy of the key).
fn clear_strings(value: &mut Value) {
    match value {
        Value::String(text) => zeroize::Zeroize::zeroize(text),
        Value::Array(items) => items.iter_mut().for_each(clear_strings),
        Value::Object(object) => object.values_mut().for_each(clear_strings),
        _ => {}
    }
}

pub(super) fn parse(body: &Value) -> Result<Vec<RawResult>, WebError> {
    Ok(array(body, &["results"], SearchProvider::Tavily)?
        .iter()
        .map(|item| RawResult {
            title: text(item, "title").to_owned(),
            url: text(item, "url").to_owned(),
            snippet: text(item, "content").to_owned(),
            published: optional_text(item, "published_date"),
        })
        .collect())
}
