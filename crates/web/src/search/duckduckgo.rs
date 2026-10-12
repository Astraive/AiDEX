//! DuckDuckGo's HTML results page (`html.duckduckgo.com/html/?q=`), no key.
//!
//! The page is parsed defensively with the crate's tokenizer: a result is a
//! `result__a` anchor (the title and link) followed by an element with the
//! `result__snippet` class. Ads (`result--ad` containers and `y.js` links) are
//! skipped, and DuckDuckGo's `/l/?uddg=` redirect links are unwrapped to the
//! target URL. Markup changes degrade to fewer results, never to a panic.

use url::Url;

use super::{ProviderRequest, RawResult};
use crate::{
    WebError,
    html::tokenizer::{Token, Tokenizer},
};

/// Most results read from one page (more than any caller keeps).
const MAX_PARSED: usize = 30;
/// Longest text gathered for one title or snippet, in bytes.
const MAX_FIELD_BYTES: usize = 4096;

pub(super) fn url(mut url: Url, query: &str) -> Url {
    url.query_pairs_mut().append_pair("q", query);
    url
}

pub(super) fn request() -> ProviderRequest {
    ProviderRequest {
        post: None,
        headers: Vec::new(),
        accept: "text/html",
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Field {
    Title,
    Snippet,
}

struct Capture {
    field: Field,
    /// The element whose end tag closes the capture.
    element: String,
    /// Nested elements of the same name inside it.
    depth: usize,
    href: Option<String>,
    text: String,
}

/// Where the parser is: the current result container and the last result
/// taken from it.
#[derive(Default)]
struct Position {
    /// Result containers seen so far (`class="result ..."`).
    container: usize,
    in_ad: bool,
    /// The container the last pushed result came from.
    last_result_container: Option<usize>,
}

pub(super) fn parse(body: &[u8]) -> Result<Vec<RawResult>, WebError> {
    let html = String::from_utf8_lossy(body);
    let mut results: Vec<RawResult> = Vec::new();
    let mut position = Position::default();
    let mut capture: Option<Capture> = None;
    let mut found_markup = false;
    for token in Tokenizer::new(&html) {
        if results.len() >= MAX_PARSED {
            break;
        }
        match &token {
            Token::Start {
                name, self_closing, ..
            } => {
                if let Some(open) = capture.as_mut() {
                    if open.element == *name && !self_closing {
                        open.depth += 1;
                    }
                    continue;
                }
                let classes: Vec<&str> = token
                    .attribute("class")
                    .map(|value| value.split_whitespace().collect())
                    .unwrap_or_default();
                if classes.contains(&"result") {
                    position.container += 1;
                    position.in_ad = classes.contains(&"result--ad");
                    found_markup = true;
                }
                let field = if name == "a" && classes.contains(&"result__a") {
                    Some(Field::Title)
                } else if classes.contains(&"result__snippet") {
                    Some(Field::Snippet)
                } else {
                    None
                };
                if let Some(field) = field
                    && !self_closing
                {
                    found_markup = true;
                    capture = Some(Capture {
                        field,
                        element: name.clone(),
                        depth: 0,
                        href: token.attribute("href").map(str::to_owned),
                        text: String::new(),
                    });
                }
            }
            Token::Text(text) => {
                if let Some(open) = capture.as_mut()
                    && open.text.len() < MAX_FIELD_BYTES
                {
                    // Raw text: entities are decoded once, when results are
                    // sanitized.
                    open.text.push_str(text);
                }
            }
            Token::End { name } => {
                let Some(open) = capture.as_mut() else {
                    continue;
                };
                if open.element != *name {
                    continue;
                }
                if open.depth > 0 {
                    open.depth -= 1;
                    continue;
                }
                if let Some(done) = capture.take() {
                    finish(done, &mut position, &mut results);
                }
            }
        }
    }
    if let Some(done) = capture.take() {
        finish(done, &mut position, &mut results);
    }
    if results.is_empty() && !found_markup && looks_like_bot_check(&html) {
        return Err(WebError::RateLimited(
            "DuckDuckGo asked for a bot check; try again later or use another provider".into(),
        ));
    }
    Ok(results)
}

fn finish(capture: Capture, position: &mut Position, results: &mut Vec<RawResult>) {
    if position.in_ad {
        return;
    }
    let text = capture
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    match capture.field {
        Field::Title => {
            let Some(url) = capture.href.as_deref().and_then(target_url) else {
                return;
            };
            results.push(RawResult {
                title: text,
                url,
                snippet: String::new(),
                published: None,
            });
            position.last_result_container = Some(position.container);
        }
        Field::Snippet => {
            // A snippet belongs to the title from its own container.
            if position.last_result_container != Some(position.container) {
                return;
            }
            if let Some(last) = results.last_mut()
                && last.snippet.is_empty()
            {
                last.snippet = text;
            }
        }
    }
}

/// The result's real URL: DuckDuckGo redirect links are unwrapped; ad links
/// and other DuckDuckGo-internal links are dropped.
fn target_url(href: &str) -> Option<String> {
    let base = Url::parse("https://duckduckgo.com/").ok()?;
    let url = base.join(href.trim()).ok()?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let internal = host == "duckduckgo.com" || host.ends_with(".duckduckgo.com");
    if !internal {
        return Some(url.into());
    }
    if url.path() == "/l/" {
        return url
            .query_pairs()
            .find(|(key, _)| key == "uddg")
            .map(|(_, value)| value.into_owned())
            .filter(|target| target.starts_with("https://") || target.starts_with("http://"));
    }
    None
}

fn looks_like_bot_check(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("anomaly-modal")
        || lower.contains("challenge-form")
        || lower.contains("bots use duckduckgo")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_links_unwrap_and_internal_links_drop() {
        assert_eq!(
            target_url(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2Flearn%3Fa%3D1&rut=abc"
            )
            .as_deref(),
            Some("https://www.rust-lang.org/learn?a=1")
        );
        assert_eq!(
            target_url("https://example.org/page").as_deref(),
            Some("https://example.org/page")
        );
        assert_eq!(target_url("https://duckduckgo.com/y.js?ad_domain=x"), None);
        assert_eq!(target_url("/l/?uddg=javascript%3Aalert(1)"), None);
        assert_eq!(target_url("/settings"), None);
    }

    #[test]
    fn garbage_and_challenges_never_panic() {
        assert!(parse(b"").unwrap().is_empty());
        assert!(
            parse(b"<a class=\"result__a\" href=\"https://x.example/\"")
                .unwrap()
                .len()
                <= 1
        );
        assert!(parse(&[0xff, 0xfe, b'<', b'a']).unwrap().is_empty());
        assert!(matches!(
            parse(b"<html><div class=\"anomaly-modal__title\">Unfortunately, bots use DuckDuckGo too.</div></html>"),
            Err(WebError::RateLimited(_))
        ));
    }
}
