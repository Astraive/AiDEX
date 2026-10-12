//! Resolve a git remote name to its push destination, so `git.push` intents carry a
//! classified destination (and a credential consumer) instead of an unknown one.
//!
//! Only the repository's own `.git/config` is read (through the confined root). The
//! broker's launcher runs git with `HOME` in a scratch directory and, when it injects
//! a credential, with `GIT_CONFIG_NOSYSTEM=1`, so no other configuration can redirect
//! the push. Configurations that could still rewrite the URL (`insteadOf`,
//! `pushInsteadOf`, includes, several URLs, worktree `.git` files) are reported as
//! inexact: the destination stays unknown and no credential is injected.

use std::{fs, io::Read};

use aidex_core::WorkspacePath;
use aidex_policy::{NetworkKind, NetworkTarget, classify_network};
use aidex_vault::ConsumerScope;
use aidex_workspace::{Access, ConfinedRoot};

/// Largest repository config read.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// How a remote is reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Transport {
    Https,
    Http,
    Ssh,
    Git,
    Local,
}

impl Transport {
    fn default_port(self) -> u16 {
        match self {
            Self::Https => 443,
            Self::Http => 80,
            Self::Ssh => 22,
            Self::Git => 9418,
            Self::Local => 0,
        }
    }

    fn scheme(self) -> &'static str {
        match self {
            Self::Https => "https",
            Self::Http => "http",
            Self::Ssh => "ssh",
            Self::Git => "git",
            Self::Local => "file",
        }
    }
}

/// A remote's push destination as far as the repository config states it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GitRemote {
    pub name: String,
    /// The URL without user info, for cards and audit.
    pub display: String,
    pub transport: Transport,
    /// Lowercase host for network transports.
    pub host: Option<String>,
    pub port: Option<u16>,
    /// The configured URL carries user info (possibly a credential).
    pub embeds_credentials: bool,
    /// `false` when other configuration could change where git actually pushes.
    pub exact: bool,
}

impl GitRemote {
    fn effective_port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.transport.default_port())
    }

    /// The classified destination, when the remote is exact and networked.
    pub fn network_target(&self) -> Option<NetworkTarget> {
        if !self.exact || self.transport == Transport::Local {
            return None;
        }
        classify_network(self.host.as_deref()?, self.effective_port()).ok()
    }

    /// `host` or `host:port` (non-default port), as used in `git:` consumer scopes.
    fn authority(&self) -> Option<String> {
        let host = self.host.as_deref()?;
        let shown = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_owned()
        };
        Some(match self.port {
            Some(port) if port != self.transport.default_port() => format!("{shown}:{port}"),
            _ => shown,
        })
    }

    /// The vault consumer a stored HTTP credential must allow (`git:<host[:port]>`).
    /// Only HTTPS remotes qualify, plus plain HTTP to a loopback address (local test
    /// servers); credentials are never sent in clear text over a network.
    pub fn credential_consumer(&self) -> Option<ConsumerScope> {
        if !self.exact || self.embeds_credentials {
            return None;
        }
        let eligible = match self.transport {
            Transport::Https => true,
            Transport::Http => self
                .network_target()
                .is_some_and(|target| target.kind() == NetworkKind::Loopback),
            _ => false,
        };
        if !eligible {
            return None;
        }
        let host = self.host.as_deref()?;
        let target = match self.port {
            Some(port) if port != self.transport.default_port() => format!("{host}:{port}"),
            _ => host.to_owned(),
        };
        ConsumerScope::git(target).ok()
    }

    /// `scheme://authority/`, the prefix an injected `http.<url>.extraHeader` is
    /// scoped to.
    pub fn url_prefix(&self) -> Option<String> {
        Some(format!(
            "{}://{}/",
            self.transport.scheme(),
            self.authority()?
        ))
    }
}

/// Look up `remote` in the workspace repository's `.git/config`. `None` when there
/// is no readable config or no such remote.
pub(crate) fn resolve(root: &ConfinedRoot, remote: &str) -> Option<GitRemote> {
    let path = WorkspacePath::new(".git/config").ok()?;
    let absolute = root.resolve(&path, Access::Read).ok()?;
    let metadata = fs::symlink_metadata(&absolute).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return None;
    }
    let mut text = String::new();
    fs::File::open(&absolute)
        .ok()?
        .take(MAX_CONFIG_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    from_config(&text, remote)
}

/// Parse git config text for `remote`'s push URL.
pub(crate) fn from_config(text: &str, remote: &str) -> Option<GitRemote> {
    let mut exact = true;
    let mut in_remote = false;
    let mut urls = Vec::new();
    let mut push_urls = Vec::new();
    for raw in text.lines() {
        let line = strip_comment(raw.trim());
        if line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header.trim_end_matches(']').trim();
            let lower = header.to_ascii_lowercase();
            if lower.starts_with("include") {
                exact = false;
            }
            in_remote = section_is_remote(header, remote);
            continue;
        }
        let (key, value) = match line.split_once('=') {
            Some((key, value)) => (key.trim().to_ascii_lowercase(), unquote(value.trim())),
            None => (line.trim().to_ascii_lowercase(), String::new()),
        };
        if key == "insteadof" || key == "pushinsteadof" {
            exact = false;
        }
        if in_remote {
            match key.as_str() {
                "url" => urls.push(value),
                "pushurl" => push_urls.push(value),
                _ => {}
            }
        }
    }
    let chosen = if push_urls.is_empty() {
        urls
    } else {
        push_urls
    };
    if chosen.len() > 1 {
        exact = false;
    }
    let url = chosen.into_iter().next()?;
    let mut parsed = parse_url(&url)?;
    parsed.name = remote.to_owned();
    parsed.exact &= exact;
    Some(parsed)
}

fn strip_comment(line: &str) -> &str {
    // Comments start with `#` or `;` outside quotes.
    let mut quoted = false;
    for (index, ch) in line.char_indices() {
        match ch {
            '"' => quoted = !quoted,
            '#' | ';' if !quoted => return line[..index].trim_end(),
            _ => {}
        }
    }
    line
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(trimmed)
        .to_owned()
}

/// `remote "name"` (subsection names are case-sensitive).
fn section_is_remote(header: &str, remote: &str) -> bool {
    let Some((section, rest)) = header.split_once(char::is_whitespace) else {
        return false;
    };
    section.eq_ignore_ascii_case("remote") && unquote(rest) == remote
}

/// Parse a git URL (`scheme://[user@]host[:port]/path`, scp-like `user@host:path`,
/// or a local path).
pub(crate) fn parse_url(url: &str) -> Option<GitRemote> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = url.split_once("://") {
        let transport = match scheme.to_ascii_lowercase().as_str() {
            "https" => Transport::Https,
            "http" => Transport::Http,
            "ssh" | "git+ssh" | "ssh+git" => Transport::Ssh,
            "git" => Transport::Git,
            "file" => return Some(local(url)),
            _ => return None,
        };
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        let (userinfo, hostport) = match authority.rsplit_once('@') {
            Some((userinfo, hostport)) => (Some(userinfo), hostport),
            None => (None, authority),
        };
        let (host, port) = split_host_port(hostport)?;
        let display = format!(
            "{}://{}{}/{path}",
            transport.scheme(),
            if host.contains(':') {
                format!("[{host}]")
            } else {
                host.clone()
            },
            port.map(|port| format!(":{port}")).unwrap_or_default()
        );
        return Some(GitRemote {
            name: String::new(),
            display,
            transport,
            host: Some(host),
            port,
            // HTTP user info may be a credential; an ssh user name is not.
            embeds_credentials: userinfo.is_some()
                && matches!(transport, Transport::Https | Transport::Http),
            exact: true,
        });
    }
    // scp-like syntax: `[user@]host:path`, with no `/` before the colon and not a
    // Windows drive letter.
    if let Some((before, path)) = url.split_once(':') {
        let is_drive = before.len() == 1 && before.chars().all(|ch| ch.is_ascii_alphabetic());
        if !is_drive && !before.contains('/') && !before.contains('\\') {
            let host = before.rsplit_once('@').map_or(before, |(_, host)| host);
            if host.is_empty() {
                return None;
            }
            return Some(GitRemote {
                name: String::new(),
                display: format!("{}:{path}", host.to_ascii_lowercase()),
                transport: Transport::Ssh,
                host: Some(host.to_ascii_lowercase()),
                port: None,
                embeds_credentials: false,
                exact: true,
            });
        }
    }
    Some(local(url))
}

fn local(url: &str) -> GitRemote {
    GitRemote {
        name: String::new(),
        display: url.to_owned(),
        transport: Transport::Local,
        host: None,
        port: None,
        embeds_credentials: false,
        exact: true,
    }
}

fn split_host_port(hostport: &str) -> Option<(String, Option<u16>)> {
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(port) if !port.is_empty() => Some(port.parse().ok()?),
            _ => None,
        };
        return Some((host.to_ascii_lowercase(), port));
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() => (host, Some(port.parse().ok()?)),
        Some((host, _)) => (host, None),
        None => (hostport, None),
    };
    if host.is_empty() {
        return None;
    }
    Some((host.to_ascii_lowercase(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_remote_yields_destination_and_consumer() {
        let config = "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://GitHub.com/acme/app.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n";
        let remote = from_config(config, "origin").unwrap();
        assert_eq!(remote.transport, Transport::Https);
        assert_eq!(remote.host.as_deref(), Some("github.com"));
        assert!(remote.exact);
        assert_eq!(
            remote.credential_consumer().unwrap().to_string(),
            "git:github.com"
        );
        assert_eq!(remote.url_prefix().unwrap(), "https://github.com/");
        assert_eq!(remote.network_target().unwrap().port(), 443);
    }

    #[test]
    fn pushurl_wins_and_rewrites_make_it_inexact() {
        let config = "[remote \"origin\"]\n url = https://a.example/x\n pushurl = https://b.example:8443/x\n";
        let remote = from_config(config, "origin").unwrap();
        assert_eq!(remote.host.as_deref(), Some("b.example"));
        assert_eq!(
            remote.credential_consumer().unwrap().to_string(),
            "git:b.example:8443"
        );
        let rewritten = format!(
            "{config}[url \"https://evil.example/\"]\n insteadOf = https://b.example:8443/\n"
        );
        let remote = from_config(&rewritten, "origin").unwrap();
        assert!(!remote.exact);
        assert!(remote.network_target().is_none());
        assert!(remote.credential_consumer().is_none());
        assert!(from_config(config, "upstream").is_none());
    }

    #[test]
    fn credentials_are_only_offered_over_tls_or_loopback() {
        let plain = parse_url("http://example.com/repo.git").unwrap();
        assert!(plain.credential_consumer().is_none());
        let loopback = parse_url("http://127.0.0.1:8080/repo.git").unwrap();
        assert_eq!(
            loopback.credential_consumer().unwrap().to_string(),
            "git:127.0.0.1:8080"
        );
        let embedded = parse_url("https://user:pw@example.com/repo.git").unwrap();
        assert!(embedded.embeds_credentials);
        assert!(!embedded.display.contains("pw"));
        assert!(embedded.credential_consumer().is_none());
        let scp = parse_url("git@github.com:acme/app.git").unwrap();
        assert_eq!(scp.transport, Transport::Ssh);
        assert!(scp.credential_consumer().is_none());
        assert_eq!(scp.network_target().unwrap().port(), 22);
        assert_eq!(
            parse_url("C:\\repos\\app").unwrap().transport,
            Transport::Local
        );
        assert_eq!(
            parse_url("/srv/app.git").unwrap().transport,
            Transport::Local
        );
    }
}
