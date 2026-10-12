//! Destination checks: URL rules, address classes, and DNS resolution.
//!
//! Every request (and every redirect hop) is checked here before a socket
//! opens. The resolved addresses are then pinned for the connection (see
//! `http.rs`), so a name cannot resolve to a public address for the check and
//! to a private one for the connection (DNS rebinding).

use std::{
    fmt,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use aidex_core::CancelToken;
use url::{Host, Url};

use crate::{WebError, http::check_cancel};

/// Longest URL accepted.
pub const MAX_URL_BYTES: usize = 8 * 1024;
/// How often a blocking wait checks its cancel token.
const POLL: Duration = Duration::from_millis(20);

/// Which explicitly local destinations a fetch may reach.
///
/// A destination is explicitly local when its URL names a loopback or private
/// IP literal or `localhost`. A DNS name is never treated as local: if it
/// resolves to anything but public addresses the fetch is refused, whatever
/// this setting says.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LocalTargets {
    /// Public addresses only (the default).
    #[default]
    Refuse,
    /// Also loopback literals and `localhost`.
    Loopback,
    /// Also private-range literals (RFC 1918, CGNAT, IPv6 ULA).
    LoopbackAndPrivate,
}

/// The kind of an IP address, for destination checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressClass {
    Public,
    Loopback,
    /// RFC 1918, carrier-grade NAT, IPv6 unique-local and site-local.
    Private,
    /// 169.254.0.0/16 and fe80::/10 (cloud metadata services live here).
    LinkLocal,
    /// Unspecified, multicast, broadcast, documentation, benchmarking, and
    /// other special-purpose ranges; never reachable.
    Reserved,
}

impl fmt::Display for AddressClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Public => "public",
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::LinkLocal => "link-local",
            Self::Reserved => "reserved",
        })
    }
}

/// Classify an address. IPv4-mapped, NAT64, and 6to4 IPv6 addresses are
/// classified by the IPv4 address they embed.
pub fn classify_ip(ip: IpAddr) -> AddressClass {
    match ip {
        IpAddr::V4(v4) => classify_v4(v4),
        IpAddr::V6(v6) => classify_v6(v6),
    }
}

fn classify_v4(ip: Ipv4Addr) -> AddressClass {
    let [a, b, c, _] = ip.octets();
    match (a, b, c) {
        (127, _, _) => AddressClass::Loopback,
        (10, _, _) | (192, 168, _) => AddressClass::Private,
        (172, b, _) if (16..32).contains(&b) => AddressClass::Private,
        (100, b, _) if (64..128).contains(&b) => AddressClass::Private,
        (169, 254, _) => AddressClass::LinkLocal,
        (0, _, _) | (192, 0, 0) | (192, 0, 2) | (198, 51, 100) | (203, 0, 113) => {
            AddressClass::Reserved
        }
        (198, b, _) if b == 18 || b == 19 => AddressClass::Reserved,
        (a, _, _) if a >= 224 => AddressClass::Reserved,
        _ => AddressClass::Public,
    }
}

fn classify_v6(ip: Ipv6Addr) -> AddressClass {
    if ip == Ipv6Addr::LOCALHOST {
        return AddressClass::Loopback;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    let segments = ip.segments();
    let embedded = |high: u16, low: u16| {
        let [h1, h2] = high.to_be_bytes();
        let [l1, l2] = low.to_be_bytes();
        Ipv4Addr::new(h1, h2, l1, l2)
    };
    match segments {
        // ::/96 (unspecified and the deprecated IPv4-compatible range).
        [0, 0, 0, 0, 0, 0, _, _] => AddressClass::Reserved,
        // NAT64 well-known prefix 64:ff9b::/96.
        [0x64, 0xff9b, 0, 0, 0, 0, high, low] => classify_v4(embedded(high, low)),
        // Local-use NAT64 64:ff9b:1::/48.
        [0x64, 0xff9b, 1, ..] => AddressClass::Private,
        // 6to4 2002::/16 embeds an IPv4 address in bits 16-48.
        [0x2002, high, low, ..] => classify_v4(embedded(high, low)),
        // Teredo 2001::/32 tunnels to arbitrary IPv4 hosts; documentation
        // 2001:db8::/32; discard 100::/64.
        [0x2001, 0, ..] | [0x2001, 0xdb8, ..] | [0x100, 0, 0, 0, ..] => AddressClass::Reserved,
        [first, ..] if first & 0xfe00 == 0xfc00 => AddressClass::Private,
        [first, ..] if first & 0xffc0 == 0xfe80 => AddressClass::LinkLocal,
        [first, ..] if first & 0xffc0 == 0xfec0 => AddressClass::Private,
        [first, ..] if first & 0xff00 == 0xff00 => AddressClass::Reserved,
        _ => AddressClass::Public,
    }
}

/// Who chose a destination, which decides how far it may reach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Origin {
    /// A built-in provider host: public addresses only.
    BuiltIn,
    /// A destination the user configured (a SearXNG endpoint, or an endpoint
    /// override): loopback and private addresses are allowed.
    Configured,
    /// A URL from a model, a page, or a redirect, with the client's allowance
    /// for explicitly local destinations.
    Requested(LocalTargets),
}

impl Origin {
    fn allows(self, class: AddressClass, explicit: bool) -> bool {
        match class {
            AddressClass::Public => true,
            AddressClass::LinkLocal | AddressClass::Reserved => false,
            AddressClass::Loopback => match self {
                Self::BuiltIn => false,
                Self::Configured => true,
                Self::Requested(local) => explicit && local != LocalTargets::Refuse,
            },
            AddressClass::Private => match self {
                Self::BuiltIn => false,
                Self::Configured => true,
                Self::Requested(local) => explicit && local == LocalTargets::LoopbackAndPrivate,
            },
        }
    }
}

/// The host part of a validated URL.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum HostKind {
    Ip(IpAddr),
    /// `localhost` or a `*.localhost` name (RFC 6761), pinned to loopback.
    Localhost,
    Name(String),
}

impl HostKind {
    fn of(url: &Url) -> Result<Self, WebError> {
        match url.host() {
            Some(Host::Ipv4(ip)) => Ok(Self::Ip(IpAddr::V4(ip))),
            Some(Host::Ipv6(ip)) => Ok(Self::Ip(IpAddr::V6(ip))),
            Some(Host::Domain("")) | None => {
                Err(WebError::InvalidUrl("the URL has no host".into()))
            }
            Some(Host::Domain(name)) => {
                let name = name.trim_end_matches('.').to_ascii_lowercase();
                if name.is_empty() {
                    Err(WebError::InvalidUrl("the URL has no host".into()))
                } else if name == "localhost" || name.ends_with(".localhost") {
                    Ok(Self::Localhost)
                } else {
                    Ok(Self::Name(name))
                }
            }
        }
    }

    fn is_loopback(&self) -> bool {
        match self {
            Self::Ip(ip) => classify_ip(*ip) == AddressClass::Loopback,
            Self::Localhost => true,
            Self::Name(_) => false,
        }
    }

    /// The host as given to the egress check (no IPv6 brackets).
    pub(crate) fn display(&self) -> String {
        match self {
            Self::Ip(ip) => ip.to_string(),
            Self::Localhost => "localhost".into(),
            Self::Name(name) => name.clone(),
        }
    }
}

/// Parse and validate an absolute http(s) URL: `https` anywhere, plain `http`
/// only to a loopback host, no embedded credentials, at most
/// [`MAX_URL_BYTES`]. The fragment is dropped.
pub(crate) fn parse_http_url(text: &str) -> Result<Url, WebError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(WebError::InvalidUrl("the URL is empty".into()));
    }
    if text.len() > MAX_URL_BYTES {
        return Err(WebError::InvalidUrl(format!(
            "the URL is longer than {MAX_URL_BYTES} bytes"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(WebError::InvalidUrl(
            "the URL contains control characters".into(),
        ));
    }
    let mut url =
        Url::parse(text).map_err(|_| WebError::InvalidUrl("not an absolute http(s) URL".into()))?;
    check_url(&url)?;
    url.set_fragment(None);
    Ok(url)
}

/// The scheme, credential, and host rules of [`parse_http_url`] for an
/// already-parsed URL (redirect targets).
pub(crate) fn check_url(url: &Url) -> Result<HostKind, WebError> {
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        return Err(WebError::InvalidUrl(format!(
            "the `{scheme}` scheme is not allowed; use https"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(WebError::InvalidUrl(
            "URLs with embedded credentials are refused".into(),
        ));
    }
    if url.as_str().len() > MAX_URL_BYTES {
        return Err(WebError::InvalidUrl(format!(
            "the URL is longer than {MAX_URL_BYTES} bytes"
        )));
    }
    let host = HostKind::of(url)?;
    if scheme == "http" && !host.is_loopback() {
        return Err(WebError::InvalidUrl(format!(
            "plain http is allowed only to loopback hosts; use https for {}",
            host.display()
        )));
    }
    if url.port_or_known_default().is_none() {
        return Err(WebError::InvalidUrl("the URL has no port".into()));
    }
    Ok(host)
}

/// Resolves a host name to socket addresses (injectable for tests).
pub type DnsResolver = Arc<dyn Fn(&str, u16) -> std::io::Result<Vec<SocketAddr>> + Send + Sync>;

/// The system resolver.
pub fn system_dns() -> DnsResolver {
    Arc::new(|host, port| (host, port).to_socket_addrs().map(Iterator::collect))
}

/// A validated destination: its URL, host, port, and the addresses the
/// connection is pinned to.
#[derive(Clone, Debug)]
pub(crate) struct Destination {
    pub url: Url,
    pub host: HostKind,
    pub port: u16,
}

impl Destination {
    /// Validate `url` for `origin` without any I/O: scheme and credential
    /// rules, and the class of an IP literal or `localhost`.
    pub(crate) fn check(url: Url, origin: Origin) -> Result<Self, WebError> {
        let host = check_url(&url)?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| WebError::InvalidUrl("the URL has no port".into()))?;
        let explicit_class = match &host {
            HostKind::Ip(ip) => Some((classify_ip(*ip), ip.to_string())),
            HostKind::Localhost => Some((AddressClass::Loopback, "localhost".to_owned())),
            HostKind::Name(_) => None,
        };
        if let Some((class, shown)) = explicit_class
            && !origin.allows(class, true)
        {
            return Err(WebError::TargetRefused(format!(
                "{shown} is a {class} address"
            )));
        }
        Ok(Self { url, host, port })
    }

    /// Whether the URL itself names a non-public address (an IP literal or
    /// `localhost`); DNS names never count as explicitly local.
    pub(crate) fn is_explicitly_local(&self) -> bool {
        match &self.host {
            HostKind::Ip(ip) => classify_ip(*ip) != AddressClass::Public,
            HostKind::Localhost => true,
            HostKind::Name(_) => false,
        }
    }

    /// Resolve the addresses to connect to, refusing any that `origin` may not
    /// reach. Names are resolved through `dns` within `timeout`.
    pub(crate) fn resolve(
        &self,
        origin: Origin,
        dns: &DnsResolver,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Vec<SocketAddr>, WebError> {
        match &self.host {
            HostKind::Ip(ip) => Ok(vec![SocketAddr::new(*ip, self.port)]),
            HostKind::Localhost => Ok(vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port),
                SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), self.port),
            ]),
            HostKind::Name(name) => {
                let addrs = lookup(dns, name, self.port, timeout, cancel)?;
                if addrs.is_empty() {
                    return Err(WebError::Unavailable(format!("{name} has no addresses")));
                }
                if let Some(refused) = addrs
                    .iter()
                    .map(|addr| classify_ip(addr.ip()))
                    .find(|class| !origin.allows(*class, false))
                {
                    return Err(WebError::TargetRefused(format!(
                        "{name} resolves to a {refused} address"
                    )));
                }
                Ok(addrs)
            }
        }
    }
}

fn lookup(
    dns: &DnsResolver,
    name: &str,
    port: u16,
    timeout: Duration,
    cancel: &CancelToken,
) -> Result<Vec<SocketAddr>, WebError> {
    check_cancel(cancel)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let resolver = Arc::clone(dns);
    let host = name.to_owned();
    thread::Builder::new()
        .name("aidex-web-dns".into())
        .spawn(move || {
            let _ = sender.send(resolver(&host, port));
        })
        .map_err(|_| WebError::Unavailable("cannot start a DNS worker thread".into()))?;
    let cancel = cancel.with_timeout(timeout);
    loop {
        check_cancel(&cancel)?;
        let wait = cancel.remaining().map_or(POLL, |left| left.min(POLL));
        match receiver.recv_timeout(wait) {
            Ok(Ok(addrs)) => return Ok(addrs.into_iter().take(16).collect()),
            Ok(Err(_)) => {
                return Err(WebError::Unavailable(format!(
                    "{name} could not be resolved"
                )));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(WebError::Unavailable(format!(
                    "{name} could not be resolved"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(text: &str) -> AddressClass {
        classify_ip(text.parse().unwrap())
    }

    #[test]
    fn addresses_are_classified_including_embedded_ipv4() {
        for (address, expected) in [
            ("8.8.8.8", AddressClass::Public),
            ("93.184.216.34", AddressClass::Public),
            ("127.0.0.1", AddressClass::Loopback),
            ("127.5.6.7", AddressClass::Loopback),
            ("10.0.0.1", AddressClass::Private),
            ("172.16.0.1", AddressClass::Private),
            ("172.31.255.255", AddressClass::Private),
            ("172.32.0.1", AddressClass::Public),
            ("192.168.1.1", AddressClass::Private),
            ("100.64.0.1", AddressClass::Private),
            ("169.254.169.254", AddressClass::LinkLocal),
            ("0.0.0.0", AddressClass::Reserved),
            ("255.255.255.255", AddressClass::Reserved),
            ("224.0.0.1", AddressClass::Reserved),
            ("192.0.2.1", AddressClass::Reserved),
            ("198.18.0.1", AddressClass::Reserved),
            ("::1", AddressClass::Loopback),
            ("::", AddressClass::Reserved),
            ("::ffff:127.0.0.1", AddressClass::Loopback),
            ("::ffff:10.1.2.3", AddressClass::Private),
            ("::ffff:8.8.8.8", AddressClass::Public),
            ("64:ff9b::a9fe:a9fe", AddressClass::LinkLocal),
            ("2002:7f00:1::", AddressClass::Loopback),
            ("2001::1", AddressClass::Reserved),
            ("2001:db8::1", AddressClass::Reserved),
            ("fd00::1", AddressClass::Private),
            ("fe80::1", AddressClass::LinkLocal),
            ("ff02::1", AddressClass::Reserved),
            ("2606:4700:4700::1111", AddressClass::Public),
        ] {
            assert_eq!(class(address), expected, "{address}");
        }
    }

    #[test]
    fn urls_need_https_except_to_loopback_and_never_carry_credentials() {
        for good in [
            "https://example.com/a?b=c#frag",
            "http://127.0.0.1:8080/",
            "http://localhost:3000/x",
            "http://[::1]:9/",
            "https://10.0.0.1/",
        ] {
            let url = parse_http_url(good).unwrap_or_else(|e| panic!("{good}: {e}"));
            assert!(url.fragment().is_none());
        }
        for bad in [
            "",
            "example.com",
            "/relative",
            "ftp://example.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,hi",
            "http://example.com/",
            "http://10.0.0.1/",
            "https://user:pass@example.com/",
            "https://user@example.com/",
            "http://user@127.0.0.1/",
            "https://exa\nmple.com/",
        ] {
            assert!(
                matches!(parse_http_url(bad), Err(WebError::InvalidUrl(_))),
                "{bad:?}"
            );
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_BYTES));
        assert!(matches!(
            parse_http_url(&long),
            Err(WebError::InvalidUrl(_))
        ));
        // Numeric spellings are normalized by the URL parser, then classified.
        let url = parse_http_url("http://0x7f.1:80/").unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
    }

    fn check(url: &str, origin: Origin) -> Result<Destination, WebError> {
        Destination::check(parse_http_url(url).unwrap(), origin)
    }

    #[test]
    fn literal_destinations_follow_the_origin() {
        let refuse = Origin::Requested(LocalTargets::Refuse);
        let loopback = Origin::Requested(LocalTargets::Loopback);
        let private = Origin::Requested(LocalTargets::LoopbackAndPrivate);
        for (url, origin, allowed) in [
            ("https://93.184.216.34/", refuse, true),
            ("http://127.0.0.1/", refuse, false),
            ("http://[::1]/", refuse, false),
            ("http://localhost/", refuse, false),
            ("http://127.0.0.1/", loopback, true),
            ("http://localhost/", loopback, true),
            ("https://10.0.0.1/", loopback, false),
            ("https://10.0.0.1/", private, true),
            ("https://169.254.169.254/", private, false),
            ("https://169.254.169.254/", Origin::Configured, false),
            ("https://[fe80::1]/", private, false),
            ("https://0.0.0.0/", private, false),
            ("https://192.168.1.4/", Origin::Configured, true),
            ("http://127.0.0.1/", Origin::BuiltIn, false),
        ] {
            let result = check(url, origin);
            assert_eq!(result.is_ok(), allowed, "{url} {origin:?}: {result:?}");
            if !allowed {
                assert!(matches!(result, Err(WebError::TargetRefused(_))));
            }
        }
    }

    fn fixed_dns(addrs: &'static [&'static str]) -> DnsResolver {
        Arc::new(move |_, port| {
            Ok(addrs
                .iter()
                .map(|ip| SocketAddr::new(ip.parse().unwrap(), port))
                .collect())
        })
    }

    #[test]
    fn names_must_resolve_only_to_allowed_addresses() {
        let cancel = CancelToken::new();
        let timeout = Duration::from_secs(5);
        let requested = Origin::Requested(LocalTargets::LoopbackAndPrivate);
        let destination = check("https://rebind.example/", requested).unwrap();
        for (addrs, allowed) in [
            (&["93.184.216.34"][..], true),
            (&["93.184.216.34", "127.0.0.1"][..], false),
            (&["10.0.0.5"][..], false),
            (&["169.254.169.254"][..], false),
            (&["::1"][..], false),
        ] {
            let result = destination.resolve(requested, &fixed_dns(addrs), timeout, &cancel);
            assert_eq!(result.is_ok(), allowed, "{addrs:?}: {result:?}");
        }
        // A user-configured endpoint may live on the local network.
        let configured = check("https://searx.lan/", Origin::Configured).unwrap();
        assert!(
            configured
                .resolve(
                    Origin::Configured,
                    &fixed_dns(&["192.168.1.10"]),
                    timeout,
                    &cancel
                )
                .is_ok()
        );
        let failing: DnsResolver = Arc::new(|_, _| Err(std::io::Error::other("nxdomain")));
        assert!(matches!(
            destination.resolve(requested, &failing, timeout, &cancel),
            Err(WebError::Unavailable(_))
        ));
        let stopped = CancelToken::new();
        stopped.cancel();
        assert_eq!(
            destination
                .resolve(requested, &fixed_dns(&["93.184.216.34"]), timeout, &stopped)
                .unwrap_err(),
            WebError::Cancelled
        );
    }
}
