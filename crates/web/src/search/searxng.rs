//! SearXNG: `GET <endpoint>/search?q=&format=json` on the user's instance; an
//! optional key is sent as a bearer token (for instances behind an
//! authenticating proxy).

use serde_json::Value;
use url::Url;
use zeroize::Zeroizing;

use super::{ProviderRequest, RawResult, array, optional_text, text};
use crate::{SearchProvider, WebError, client::Secret};

pub(super) fn url(mut url: Url, query: &str) -> Url {
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json")
        .append_pair("pageno", "1");
    url
}

pub(super) fn request(key: Option<&Secret>) -> ProviderRequest {
    let headers = key
        .map(|key| {
            vec![(
                "Authorization",
                Zeroizing::new(format!("Bearer {}", key.expose())),
            )]
        })
        .unwrap_or_default();
    ProviderRequest {
        post: None,
        headers,
        accept: "application/json",
    }
}

pub(super) fn parse(body: &Value) -> Result<Vec<RawResult>, WebError> {
    Ok(array(body, &["results"], SearchProvider::Searxng)?
        .iter()
        .map(|item| RawResult {
            title: text(item, "title").to_owned(),
            url: text(item, "url").to_owned(),
            snippet: text(item, "content").to_owned(),
            published: optional_text(item, "publishedDate"),
        })
        .collect())
}
