//! Guarded page fetch: manual redirects re-validated per hop, bounded bodies,
//! readable content types only, and HTML reduced to readable text.

use aidex_core::CancelToken;
use url::Url;

use crate::{
    FetchedPage, WebClient, WebError,
    client::{EgressPurpose, Payload},
    html::{extract_until, truncate_chars},
    http::{ContentRule, Limits, Overflow, Response, check_cancel},
    target::{self, LocalTargets, Origin},
};

const ACCEPT_PAGE: &str =
    "text/html,application/xhtml+xml,text/plain;q=0.9,application/json;q=0.8,text/*;q=0.7";

pub(crate) fn run(
    client: &WebClient,
    url: &str,
    cancel: &CancelToken,
) -> Result<FetchedPage, WebError> {
    let requested = target::parse_http_url(url)?;
    let options = *client.options();
    let mut origin = Origin::Requested(options.local_targets);
    let mut current = requested.clone();
    let mut hop = 0;
    loop {
        let approved =
            client.approve(EgressPurpose::Fetch, current.clone(), origin, hop, cancel)?;
        if hop == 0 {
            origin = redirect_origin(origin, approved.is_explicitly_local());
        }
        let response = client.exchange(
            approved,
            Payload {
                post: None,
                headers: Vec::new(),
                accept: ACCEPT_PAGE,
                limits: Limits {
                    max_body: options.max_raw_bytes,
                    overflow: Overflow::Truncate,
                    content: ContentRule::Readable,
                },
            },
            cancel,
        )?;
        if !response.is_redirect() {
            return page(requested, current, response, options.max_text_bytes, cancel);
        }
        if hop >= options.max_redirects {
            return Err(WebError::TooManyRedirects(options.max_redirects));
        }
        current = next_hop(&current, response.location.as_deref())?;
        hop += 1;
    }
}

/// The origin redirect hops are checked with. A public page may not redirect
/// into the local network: only a URL that itself names a local address keeps
/// the client's allowance for local destinations.
fn redirect_origin(first: Origin, first_is_explicitly_local: bool) -> Origin {
    if first_is_explicitly_local {
        first
    } else {
        Origin::Requested(LocalTargets::Refuse)
    }
}

/// The validated target of a redirect, relative to the current URL.
fn next_hop(current: &Url, location: Option<&str>) -> Result<Url, WebError> {
    let location = location
        .map(str::trim)
        .filter(|location| !location.is_empty())
        .ok_or_else(|| WebError::Protocol("a redirect had no Location header".into()))?;
    let next = current
        .join(location)
        .map_err(|_| WebError::InvalidUrl("the redirect target is not a valid URL".into()))?;
    target::parse_http_url(next.as_str())
}

fn page(
    requested: Url,
    final_url: Url,
    response: Response,
    max_text: usize,
    cancel: &CancelToken,
) -> Result<FetchedPage, WebError> {
    let media_type = response.media_type.clone().unwrap_or_default();
    let html = matches!(media_type.as_str(), "text/html" | "application/xhtml+xml");
    let charset = response
        .charset
        .clone()
        .or_else(|| html.then(|| sniff_charset(&response.body)).flatten());
    let decoded = decode(&response.body, charset.as_deref());
    let (title, text, cut) = if html {
        let readable = extract_until(&decoded, Some(&final_url), max_text, &|| {
            cancel.is_stopped()
        });
        // Extraction stops early once the fetch is cancelled or times out; that
        // is the fetch's error, not a page cut at the text limit.
        if readable.truncated {
            check_cancel(cancel)?;
        }
        (readable.title, readable.text, readable.truncated)
    } else {
        let text = decoded.replace("\r\n", "\n");
        let cut = text.len() > max_text;
        (None, truncate_bytes(&text, max_text), cut)
    };
    Ok(FetchedPage {
        url: requested.into(),
        final_url: final_url.into(),
        status: response.status,
        content_type: media_type,
        title: title.map(|title| truncate_chars(&title, 512)),
        text,
        truncated: response.truncated || cut,
    })
}

fn truncate_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text[..cut].to_owned()
}

/// A `<meta charset>` or `http-equiv` charset in the first 2 KiB of a page.
fn sniff_charset(body: &[u8]) -> Option<String> {
    let head = &body[..body.len().min(2048)];
    let lower = String::from_utf8_lossy(head).to_ascii_lowercase();
    let at = lower.find("charset=")? + "charset=".len();
    let value: String = lower[at..]
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .collect();
    (!value.is_empty()).then_some(value)
}

/// Decode the body: UTF-8 (lossy, BOM removed) unless the charset is a
/// single-byte Latin one, which is mapped as Windows-1252.
fn decode(body: &[u8], charset: Option<&str>) -> String {
    let latin = matches!(
        charset,
        Some(
            "iso-8859-1" | "latin1" | "latin-1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii"
        )
    );
    if latin {
        return body.iter().map(|&byte| windows_1252(byte)).collect();
    }
    let body = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body);
    String::from_utf8_lossy(body).into_owned()
}

fn windows_1252(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{192}', '\u{201e}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{2c6}', '\u{2030}', '\u{160}', '\u{2039}', '\u{152}', '\u{fffd}', '\u{17d}',
        '\u{fffd}', '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}',
        '\u{2013}', '\u{2014}', '\u{2dc}', '\u{2122}', '\u{161}', '\u{203a}', '\u{153}',
        '\u{fffd}', '\u{17e}', '\u{178}',
    ];
    match byte {
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => char::from(byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_targets_resolve_relative_and_follow_the_url_rules() {
        let current = Url::parse("https://example.com/a/b").unwrap();
        assert_eq!(
            next_hop(&current, Some("../c?d=1#frag")).unwrap().as_str(),
            "https://example.com/c?d=1"
        );
        assert_eq!(
            next_hop(&current, Some("//cdn.example.org/x"))
                .unwrap()
                .as_str(),
            "https://cdn.example.org/x"
        );
        for bad in [
            Some("http://example.com/plain"),
            Some("ftp://example.com/"),
            Some("file:///etc/passwd"),
            Some("https://user:pass@example.com/"),
            Some("javascript:alert(1)"),
        ] {
            assert!(
                matches!(next_hop(&current, bad), Err(WebError::InvalidUrl(_))),
                "{bad:?}"
            );
        }
        assert!(matches!(
            next_hop(&current, None),
            Err(WebError::Protocol(_))
        ));
        assert!(matches!(
            next_hop(&current, Some("  ")),
            Err(WebError::Protocol(_))
        ));
    }

    #[test]
    fn only_explicitly_local_pages_may_redirect_to_local_addresses() {
        let allowance = Origin::Requested(LocalTargets::LoopbackAndPrivate);
        assert_eq!(redirect_origin(allowance, true), allowance);
        assert_eq!(
            redirect_origin(allowance, false),
            Origin::Requested(LocalTargets::Refuse)
        );
    }

    #[test]
    fn bodies_decode_by_charset() {
        assert_eq!(decode(b"\xef\xbb\xbfcaf\xc3\xa9", None), "caf\u{e9}");
        assert_eq!(
            decode(b"caf\xe9 \x80 \x96", Some("iso-8859-1")),
            "caf\u{e9} \u{20ac} \u{2013}"
        );
        assert_eq!(decode(b"bad \xff", Some("utf-8")), "bad \u{fffd}");
        assert_eq!(
            sniff_charset(b"<html><head><meta charset=\"Windows-1252\">").as_deref(),
            Some("windows-1252")
        );
        assert_eq!(
            sniff_charset(
                b"<meta http-equiv=Content-Type content='text/html; charset=ISO-8859-1'>"
            )
            .as_deref(),
            Some("iso-8859-1")
        );
        assert_eq!(sniff_charset(b"<p>no declaration</p>"), None);
    }

    #[test]
    fn a_fetch_stopped_during_extraction_reports_why() {
        let url = Url::parse("https://example.com/").unwrap();
        let response = || Response {
            status: 200,
            location: None,
            media_type: Some("text/html".into()),
            charset: None,
            body: "<p>word</p>".repeat(20_000).into_bytes(),
            truncated: false,
        };
        let cancelled = CancelToken::new();
        cancelled.cancel();
        assert!(matches!(
            page(url.clone(), url.clone(), response(), 1 << 20, &cancelled),
            Err(WebError::Cancelled)
        ));
        let expired = CancelToken::new().with_deadline(std::time::Instant::now());
        assert!(matches!(
            page(url.clone(), url.clone(), response(), 1 << 20, &expired),
            Err(WebError::Timeout)
        ));
        let fetched = page(url.clone(), url, response(), 1 << 20, &CancelToken::new()).unwrap();
        assert!(!fetched.truncated);
        assert!(fetched.text.starts_with("word\n\nword"));
    }

    #[test]
    fn plain_text_is_cut_at_a_character_boundary() {
        assert_eq!(truncate_bytes("\u{e9}\u{e9}\u{e9}", 3), "\u{e9}");
        assert_eq!(truncate_bytes("abc", 10), "abc");
    }
}
