//! One HTTP exchange on a worker thread, pinned to pre-checked addresses.
//!
//! `ureq` is blocking, so each exchange runs on its own thread and hands the
//! bounded result back over a channel; the caller polls its [`CancelToken`]
//! and abandons the worker on cancellation (the worker is bounded by the
//! agent's timeouts). Every exchange gets a fresh agent whose resolver returns
//! only the addresses the destination check approved, with redirects,
//! environment proxies, and cookies disabled.

use std::{
    io::Read,
    net::SocketAddr,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use aidex_core::{CancelReason, CancelToken};
use ureq::{
    config::Config,
    http::Uri,
    unversioned::{
        resolver::{ResolvedSocketAddrs, Resolver},
        transport::{DefaultConnector, NextTimeout},
    },
};
use url::Url;
use zeroize::Zeroizing;

use crate::WebError;

/// Longest wait between cancellation checks.
const POLL: Duration = Duration::from_millis(20);
const USER_AGENT: &str = concat!("aidex/", env!("CARGO_PKG_VERSION"));

pub(crate) fn check_cancel(cancel: &CancelToken) -> Result<(), WebError> {
    match cancel.state() {
        Some(CancelReason::Cancelled) => Err(WebError::Cancelled),
        Some(CancelReason::DeadlineExceeded) => Err(WebError::Timeout),
        None => Ok(()),
    }
}

/// Connection deadlines for one exchange.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timeouts {
    pub connect: Duration,
    pub response: Duration,
    pub body: Duration,
}

/// Which response content types are read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentRule {
    /// Any type (provider APIs; the caller parses the body).
    Any,
    /// Only `text/*`, `application/json`, and `application/xhtml+xml`.
    Readable,
}

/// What happens when the body is larger than the limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Overflow {
    /// Keep the first `max_body` bytes and mark the response truncated.
    Truncate,
    /// Fail with a protocol error (a partial API response is useless).
    Fail,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub max_body: usize,
    pub overflow: Overflow,
    pub content: ContentRule,
}

pub(crate) struct Request {
    pub url: Url,
    /// The approved addresses; the connection uses only these.
    pub addrs: Vec<SocketAddr>,
    /// `(content type, body)` for a POST; `None` sends a GET. The body may
    /// carry a credential and is zeroized when dropped.
    pub post: Option<(&'static str, Zeroizing<Vec<u8>>)>,
    /// Extra headers; values may be credentials and are zeroized once sent.
    pub headers: Vec<(&'static str, Zeroizing<String>)>,
    pub accept: &'static str,
}

#[derive(Debug)]
pub(crate) struct Response {
    pub status: u16,
    /// The `Location` header of a redirect.
    pub location: Option<String>,
    /// The media type, lowercase, without parameters.
    pub media_type: Option<String>,
    /// The `charset` parameter, lowercase.
    pub charset: Option<String>,
    pub body: Vec<u8>,
    pub truncated: bool,
}

impl Response {
    pub fn is_redirect(&self) -> bool {
        matches!(self.status, 301 | 302 | 303 | 307 | 308)
    }
}

/// Whether a fetch may read a body of this media type.
pub(crate) fn is_readable(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || media_type == "application/json"
        || media_type == "application/xhtml+xml"
}

/// Run one exchange, returning as soon as `cancel` stops.
pub(crate) fn exchange(
    request: Request,
    limits: Limits,
    timeouts: Timeouts,
    cancel: &CancelToken,
) -> Result<Response, WebError> {
    check_cancel(cancel)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker_cancel = cancel.clone();
    thread::Builder::new()
        .name("aidex-web-http".into())
        .spawn(move || {
            let result = run(request, limits, timeouts, &worker_cancel);
            let _ = sender.send(result);
        })
        .map_err(|_| WebError::Unavailable("cannot start an HTTP worker thread".into()))?;
    loop {
        check_cancel(cancel)?;
        let wait = cancel.remaining().map_or(POLL, |left| left.min(POLL));
        match receiver.recv_timeout(wait) {
            Ok(result) => return result,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(WebError::Unavailable(
                    "the HTTP exchange ended unexpectedly".into(),
                ));
            }
        }
    }
}

/// Resolves only the destination's approved addresses, for its own host.
#[derive(Debug)]
struct PinnedResolver {
    host: String,
    addrs: Vec<SocketAddr>,
}

impl Resolver for PinnedResolver {
    fn resolve(
        &self,
        uri: &Uri,
        _config: &Config,
        _timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let host = uri
            .host()
            .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
            .unwrap_or_default();
        if !host.eq_ignore_ascii_case(&self.host) {
            return Err(ureq::Error::HostNotFound);
        }
        let mut resolved = self.empty();
        for addr in self.addrs.iter().take(16) {
            resolved.push(*addr);
        }
        if resolved.is_empty() {
            Err(ureq::Error::HostNotFound)
        } else {
            Ok(resolved)
        }
    }
}

fn agent(url: &Url, addrs: Vec<SocketAddr>, timeouts: Timeouts) -> ureq::Agent {
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .max_redirects_will_error(false)
        .save_redirect_history(false)
        .proxy(None)
        .timeout_connect(Some(timeouts.connect))
        .timeout_recv_response(Some(timeouts.response))
        .timeout_recv_body(Some(timeouts.body))
        .user_agent(USER_AGENT)
        .build();
    ureq::Agent::with_parts(
        config,
        DefaultConnector::new(),
        PinnedResolver { host, addrs },
    )
}

fn run(
    request: Request,
    limits: Limits,
    timeouts: Timeouts,
    cancel: &CancelToken,
) -> Result<Response, WebError> {
    let Request {
        url,
        addrs,
        post,
        headers,
        accept,
    } = request;
    let host = url.host_str().unwrap_or_default().to_owned();
    let agent = agent(&url, addrs, timeouts);
    check_cancel(cancel)?;
    let result = match post {
        None => {
            let mut builder = agent.get(url.as_str()).header("Accept", accept);
            for (name, value) in &headers {
                builder = builder.header(*name, value.as_str());
            }
            builder.call()
        }
        Some((content_type, body)) => {
            let mut builder = agent
                .post(url.as_str())
                .header("Accept", accept)
                .content_type(content_type);
            for (name, value) in &headers {
                builder = builder.header(*name, value.as_str());
            }
            let sent = builder.send(&body[..]);
            drop(body);
            sent
        }
    };
    drop(headers);
    let mut response = result.map_err(|error| map_error(&error, &host))?;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let location = header("location");
    let (media_type, charset) = header("content-type")
        .as_deref()
        .map(parse_content_type)
        .unwrap_or_default();
    let mut parsed = Response {
        status,
        location,
        media_type,
        charset,
        body: Vec::new(),
        truncated: false,
    };
    if parsed.is_redirect() {
        return Ok(parsed);
    }
    if limits.content == ContentRule::Readable {
        let media = parsed.media_type.as_deref().unwrap_or("");
        if !is_readable(media) {
            let shown = if media.is_empty() { "(none)" } else { media };
            return Err(WebError::UnsupportedContentType(shown.to_owned()));
        }
    }
    let limit = u64::try_from(limits.max_body).unwrap_or(u64::MAX);
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit.saturating_add(1))
        .read_to_end(&mut body)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::TimedOut => WebError::Timeout,
            _ => WebError::Unavailable(format!(
                "the connection to {host} was lost while reading the response"
            )),
        })?;
    if body.len() > limits.max_body {
        if limits.overflow == Overflow::Fail {
            return Err(WebError::Protocol(format!(
                "the response from {host} is larger than {} bytes",
                limits.max_body
            )));
        }
        body.truncate(limits.max_body);
        parsed.truncated = true;
    }
    parsed.body = body;
    Ok(parsed)
}

/// `(media type, charset)` from a `Content-Type` value, both lowercase.
fn parse_content_type(value: &str) -> (Option<String>, Option<String>) {
    let mut parts = value.split(';');
    let media = parts
        .next()
        .map(|media| media.trim().to_ascii_lowercase())
        .filter(|media| !media.is_empty());
    let charset = parts.find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_ascii_lowercase())
    });
    (media, charset)
}

fn map_error(error: &ureq::Error, host: &str) -> WebError {
    match error {
        ureq::Error::Timeout(_) => WebError::Timeout,
        ureq::Error::HostNotFound => WebError::Unavailable(format!("{host} could not be reached")),
        ureq::Error::ConnectionFailed => {
            WebError::Unavailable(format!("the connection to {host} failed"))
        }
        ureq::Error::Io(io) => match io.kind() {
            std::io::ErrorKind::TimedOut => WebError::Timeout,
            std::io::ErrorKind::ConnectionRefused => {
                WebError::Unavailable(format!("{host} refused the connection"))
            }
            kind => WebError::Unavailable(format!("connection error with {host} ({kind})")),
        },
        ureq::Error::Protocol(_) | ureq::Error::LargeResponseHeader(..) => {
            WebError::Protocol(format!("{host} sent a malformed HTTP response"))
        }
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::TlsRequired => {
            WebError::Unavailable(format!("the TLS connection to {host} failed"))
        }
        _ => WebError::Unavailable(format!("the request to {host} failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_are_split_and_lowercased() {
        assert_eq!(
            parse_content_type("Text/HTML; Charset=\"ISO-8859-1\""),
            (Some("text/html".into()), Some("iso-8859-1".into()))
        );
        assert_eq!(
            parse_content_type("application/json"),
            (Some("application/json".into()), None)
        );
        assert_eq!(parse_content_type(""), (None, None));
        for readable in [
            "text/html",
            "text/plain",
            "application/json",
            "application/xhtml+xml",
        ] {
            assert!(is_readable(readable));
        }
        for binary in [
            "image/png",
            "application/pdf",
            "application/octet-stream",
            "",
        ] {
            assert!(!is_readable(binary));
        }
    }
}
