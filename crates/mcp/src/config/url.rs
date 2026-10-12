//! Validation of remote MCP endpoint URLs.

use std::net::IpAddr;

const MAX_URL: usize = 2048;

/// A validated Streamable HTTP endpoint such as `https://mcp.example.com/mcp`.
///
/// `https` is required, except that plain `http` is accepted for loopback hosts.
/// User info (credentials embedded in the URL), queries, fragments, whitespace,
/// and non-ASCII text are refused: credentials belong in the vault and reach the
/// server only as headers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EndpointUrl {
    https: bool,
    /// Lowercase host name or IP literal (IPv6 without brackets).
    host: String,
    port: u16,
    explicit_port: bool,
    /// Path beginning with `/` (at least `/`).
    path: String,
}

impl EndpointUrl {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_URL {
            return Err("the URL is longer than 2048 bytes".into());
        }
        if !text.is_ascii() || text.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
            return Err("the URL must be ASCII without spaces or control characters".into());
        }
        let lower = text.to_ascii_lowercase();
        let (https, rest) = if lower.starts_with("https://") {
            (true, &text["https://".len()..])
        } else if lower.starts_with("http://") {
            (false, &text["http://".len()..])
        } else {
            return Err("the URL scheme must be https:// (or http:// for loopback)".into());
        };
        if rest.contains(['?', '#']) {
            return Err(
                "queries and fragments are not allowed; map credentials to headers from the vault"
                    .into(),
            );
        }
        let (authority, path) = rest.find('/').map_or((rest, "/"), |at| rest.split_at(at));
        if authority.contains('@') {
            return Err(
                "credentials must not be embedded in the URL; store them in the vault".into(),
            );
        }
        let (host, port) = split_host_port(authority)?;
        let default_port = if https { 443 } else { 80 };
        let url = Self {
            https,
            host: host.to_ascii_lowercase(),
            port: port.unwrap_or(default_port),
            explicit_port: port.is_some(),
            path: path.to_owned(),
        };
        if !url.https && !url.is_loopback() {
            return Err("plain http:// is allowed only for loopback hosts; use https://".into());
        }
        Ok(url)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn is_https(&self) -> bool {
        self.https
    }

    pub fn is_loopback(&self) -> bool {
        self.host == "localhost"
            || self.host.ends_with(".localhost")
            || self.host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
    }

    /// `scheme://host[:port]` without the path (the path may carry identifiers).
    pub fn origin(&self) -> String {
        let scheme = if self.https { "https" } else { "http" };
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.explicit_port {
            format!("{scheme}://{host}:{}", self.port)
        } else {
            format!("{scheme}://{host}")
        }
    }

    /// The full URL.
    pub fn as_url(&self) -> String {
        format!("{}{}", self.origin(), self.path)
    }
}

fn split_host_port(authority: &str) -> Result<(&str, Option<u16>), String> {
    if authority.is_empty() {
        return Err("the URL has no host".into());
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| "unterminated IPv6 literal".to_owned())?;
        let host = &rest[..end];
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("invalid IPv6 literal".into());
        }
        let tail = &rest[end + 1..];
        let port = match tail.strip_prefix(':') {
            Some(port) => Some(port),
            None if tail.is_empty() => None,
            None => return Err("unexpected text after the IPv6 literal".into()),
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err("the URL has no host".into());
    }
    if !authority.starts_with('[')
        && !host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_'))
    {
        return Err("the host contains invalid characters".into());
    }
    let port = match port {
        None => None,
        Some(text) => match text.parse::<u16>() {
            Ok(0) | Err(_) => return Err("the port must be 1-65535".into()),
            Ok(port) => Some(port),
        },
    };
    Ok((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_and_loopback_http_are_accepted() {
        let url = EndpointUrl::parse("https://MCP.example.com/v1/mcp").unwrap();
        assert_eq!(url.host(), "mcp.example.com");
        assert_eq!(url.port(), 443);
        assert_eq!(url.origin(), "https://mcp.example.com");
        assert_eq!(url.as_url(), "https://mcp.example.com/v1/mcp");
        let local = EndpointUrl::parse("http://127.0.0.1:8123").unwrap();
        assert!(local.is_loopback());
        assert_eq!(local.as_url(), "http://127.0.0.1:8123/");
        let v6 = EndpointUrl::parse("http://[::1]:9/mcp").unwrap();
        assert_eq!(v6.origin(), "http://[::1]:9");
    }

    #[test]
    fn unsafe_urls_are_refused() {
        for bad in [
            "http://example.com/mcp",
            "ftp://example.com",
            "https://user:pw@example.com",
            "https://example.com/mcp?token=x",
            "https://example.com/#x",
            "https:// example.com",
            "https://",
            "https://example.com:0",
            "https://exa mple.com",
            "https://[zz]/",
        ] {
            assert!(EndpointUrl::parse(bad).is_err(), "{bad}");
        }
    }
}
