//! Brave Search API: `GET /res/v1/web/search?q=&count=` with the key in the
//! `X-Subscription-Token` header.

use serde_json::Value;
use url::Url;
use zeroize::Zeroizing;

use super::{ProviderRequest, RawResult, array, optional_text, text};
use crate::{SearchProvider, WebError, client::Secret};

pub(super) fn url(mut url: Url, query: &str, count: usize) -> Url {
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("count", &count.to_string());
    url
}

pub(super) fn request(key: &Secret) -> ProviderRequest {
    ProviderRequest {
        post: None,
        headers: vec![(
            "X-Subscription-Token",
            Zeroizing::new(key.expose().to_owned()),
        )],
        accept: "application/json",
    }
}

pub(super) fn parse(body: &Value) -> Result<Vec<RawResult>, WebError> {
    Ok(array(body, &["web", "results"], SearchProvider::Brave)?
        .iter()
        .map(|item| RawResult {
            title: text(item, "title").to_owned(),
            url: text(item, "url").to_owned(),
            snippet: text(item, "description").to_owned(),
            published: optional_text(item, "page_age").or_else(|| optional_text(item, "age")),
        })
        .collect())
}
