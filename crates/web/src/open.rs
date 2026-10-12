//! Validation of links a user asks to open in the system browser.

use url::Url;

use crate::WebError;

/// Longest URL handed to the system browser.
const MAX_OPEN_URL_BYTES: usize = 4096;

/// Validate a link from a search result or page before it is handed to the
/// system browser: an absolute `http` or `https` URL with a host, at most
/// 4096 bytes, without embedded credentials, whitespace, or control
/// characters. The returned URL is normalized (the fragment is kept).
pub fn validate_open_url(text: &str) -> Result<Url, WebError> {
    let invalid = |message: &str| Err(WebError::InvalidUrl(message.to_owned()));
    if text.is_empty() {
        return invalid("the URL is empty");
    }
    if text.len() > MAX_OPEN_URL_BYTES {
        return invalid("the URL is longer than 4096 bytes");
    }
    if text.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
        return invalid("the URL contains whitespace or control characters");
    }
    let url = Url::parse(text).map_err(|_| WebError::InvalidUrl("not an absolute URL".into()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(WebError::InvalidUrl(format!(
            "the `{}` scheme cannot be opened; only http and https links can",
            url.scheme()
        )));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return invalid("the URL has no host");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return invalid("URLs with embedded credentials are refused");
    }
    if url.as_str().len() > MAX_OPEN_URL_BYTES {
        return invalid("the URL is longer than 4096 bytes");
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_http_and_https_links_open() {
        for good in [
            "https://www.rust-lang.org/learn#book",
            "http://example.com/a?b=c",
            "https://[2606:4700::1111]/",
        ] {
            assert!(validate_open_url(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "example.com",
            "//example.com/x",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "data:text/html,<script>1</script>",
            "ms-settings:privacy",
            "mailto:a@example.com",
            "https://user:pass@example.com/",
            "https://user@example.com/",
            "https://example.com/a b",
            "https://example.com/\n",
            "https://",
        ] {
            assert!(
                matches!(validate_open_url(bad), Err(WebError::InvalidUrl(_))),
                "{bad:?}"
            );
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_OPEN_URL_BYTES));
        assert!(validate_open_url(&long).is_err());
    }
}
