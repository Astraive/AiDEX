use crate::{model::*, network::*};
use aidex_drive::DrivePaths;
use aidex_tools::CancelToken;
use aidex_vault::{ConsumerScope, SecretBytes, SecretKind, SecretName, SecretPolicy, VaultService};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use zeroize::Zeroizing;

pub const SEARCH_CONSUMER: &str = "aidex-web-search";
pub struct WebService {
    path: PathBuf,
    vault: Arc<VaultService>,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
    transport: Arc<dyn HttpTransport>,
    changes: Mutex<()>,
}
impl WebService {
    pub fn open(
        paths: &DrivePaths,
        vault: Arc<VaultService>,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Arc<Self>> {
        Self::with_transport(paths, vault, gate, default_transport())
    }
    pub fn with_transport(
        paths: &DrivePaths,
        vault: Arc<VaultService>,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
        transport: Arc<dyn HttpTransport>,
    ) -> Result<Arc<Self>> {
        let dir = paths
            .ensure_dir(&paths.config())
            .map_err(|_| WebError::Transport)?;
        let path = dir.join("web-search.json");
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err(WebError::Invalid);
        }
        Ok(Arc::new(Self {
            path,
            vault,
            gate,
            transport,
            changes: Mutex::new(()),
        }))
    }
    pub fn settings(&self) -> Result<SearchSettings> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes.len() <= 4096 => {
                serde_json::from_slice(&bytes).map_err(|_| WebError::Response)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(SearchSettings::default())
            }
            _ => Err(WebError::Response),
        }
    }
    pub fn status(&self) -> Result<WebStatus> {
        let settings = self.settings()?;
        let network_allowed = (self.gate)();
        let name =
            SecretName::new(settings.provider.secret_name()).map_err(|_| WebError::Invalid)?;
        Ok(WebStatus {
            settings,
            credential_present: self.vault.secret_meta(&name).is_ok(),
            network_allowed,
            fetch_available: network_allowed,
        })
    }
    pub fn configure(
        &self,
        mut settings: SearchSettings,
        credential: Option<SecretBytes>,
    ) -> Result<WebStatus> {
        let _guard = self.changes.lock().map_err(|_| WebError::Conflict)?;
        let current = self.settings()?;
        settings.revision = current.revision.checked_add(1).ok_or(WebError::Conflict)?;
        if current.provider != settings.provider && settings.enabled && credential.is_none() {
            return Err(WebError::CredentialUnavailable);
        }
        let name =
            SecretName::new(settings.provider.secret_name()).map_err(|_| WebError::Invalid)?;
        if let Some(credential) = credential {
            if credential.is_empty() || credential.len() > 8192 {
                return Err(WebError::Invalid);
            }
            let text = std::str::from_utf8(credential.expose()).map_err(|_| WebError::Invalid)?;
            if text.trim().is_empty() || text.chars().any(char::is_control) {
                return Err(WebError::Invalid);
            }
            let consumer =
                ConsumerScope::process(SEARCH_CONSUMER).map_err(|_| WebError::Invalid)?;
            self.vault
                .put_secret(
                    &name,
                    SecretKind::ApiKey,
                    credential,
                    SecretPolicy::new([consumer]).map_err(|_| WebError::Invalid)?,
                )
                .map_err(|_| WebError::CredentialUnavailable)?;
        }
        if settings.enabled && self.vault.secret_meta(&name).is_err() {
            return Err(WebError::CredentialUnavailable);
        }
        let temp = self.path.with_extension(format!(
            "{}.tmp",
            aidex_core::EntityId::generate("web").map_err(|_| WebError::Transport)?
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(|_| WebError::Transport)?;
        let result = (|| {
            file.write_all(&serde_json::to_vec(&settings).map_err(|_| WebError::Response)?)
                .map_err(|_| WebError::Transport)?;
            file.sync_all().map_err(|_| WebError::Transport)?;
            drop(file);
            std::fs::rename(&temp, &self.path).map_err(|_| WebError::Transport)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result?;
        self.status()
    }
    pub fn disconnect(&self) -> Result<WebStatus> {
        let mut settings = self.settings()?;
        settings.enabled = false;
        let name =
            SecretName::new(settings.provider.secret_name()).map_err(|_| WebError::Invalid)?;
        self.configure(settings, None)?;
        if self.vault.secret_meta(&name).is_ok() {
            self.vault
                .delete_secret(&name)
                .map_err(|_| WebError::CredentialUnavailable)?;
        }
        self.status()
    }
    fn admit(&self, cancel: &CancelToken) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(WebError::Cancelled);
        }
        if !(self.gate)() {
            return Err(WebError::Offline);
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Result<SearchSnapshot> {
        let _guard = self.changes.lock().map_err(|_| WebError::Conflict)?;
        let settings = self.settings()?;
        if !settings.enabled {
            return Err(WebError::NotConfigured);
        }
        let name =
            SecretName::new(settings.provider.secret_name()).map_err(|_| WebError::Invalid)?;
        let credential_version = self
            .vault
            .secret_version(&name)
            .map_err(|_| WebError::CredentialUnavailable)?;
        Ok(SearchSnapshot {
            settings,
            credential_version,
        })
    }
    fn credential(
        &self,
        provider: SearchProvider,
        expected: &aidex_vault::SecretVersion,
    ) -> Result<Zeroizing<String>> {
        let consumer = ConsumerScope::process(SEARCH_CONSUMER).map_err(|_| WebError::Invalid)?;
        let name = SecretName::new(provider.secret_name()).map_err(|_| WebError::Invalid)?;
        let bytes = self
            .vault
            .resolve_versioned(&name, &consumer, expected)
            .map_err(|error| match error {
                aidex_vault::VaultError::HandleDenied(
                    aidex_vault::HandleDenial::RevisionChanged,
                ) => WebError::Conflict,
                _ => WebError::CredentialUnavailable,
            })?;
        let text =
            std::str::from_utf8(bytes.expose()).map_err(|_| WebError::CredentialUnavailable)?;
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(WebError::CredentialUnavailable);
        }
        Ok(Zeroizing::new(text.to_owned()))
    }
    pub fn search(
        self: &Arc<Self>,
        input: &SearchInput,
        expected: &SearchSnapshot,
        cancel: &CancelToken,
    ) -> Result<SearchOutput> {
        self.admit(cancel)?;
        validate_search(input)?;
        let guard = self.changes.lock().map_err(|_| WebError::Conflict)?;
        let settings = self.settings()?;
        if settings != expected.settings {
            return Err(WebError::Conflict);
        }
        if !settings.enabled {
            return Err(WebError::NotConfigured);
        }
        let credential = self.credential(settings.provider, &expected.credential_version)?;
        drop(guard);
        let sensitive = credential.clone();
        self.admit(cancel)?;
        let weak = Arc::downgrade(self);
        let bound = expected.clone();
        let request_gate: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            weak.upgrade().is_some_and(|service| {
                (service.gate)() && service.snapshot().is_ok_and(|current| current == bound)
            })
        });
        let request = match settings.provider {
            SearchProvider::Brave => {
                let mut url = validate_url(settings.provider.endpoint())?;
                url.query_pairs_mut()
                    .append_pair("q", &input.query)
                    .append_pair("count", &input.count.to_string());
                if let Some(freshness) = &input.freshness {
                    url.query_pairs_mut().append_pair("freshness", freshness);
                }
                HttpRequest {
                    url: url.to_string(),
                    json: None,
                    authorization: Some(("X-Subscription-Token".into(), credential)),
                    network_allowed: request_gate.clone(),
                }
            }
            SearchProvider::Tavily => {
                let body = serde_json::json!({"query":input.query,"max_results":input.count,"search_depth":"basic","include_answer":false});
                if input.freshness.is_some() {
                    return Err(WebError::Invalid);
                }
                HttpRequest {
                    url: settings.provider.endpoint().into(),
                    json: Some(body),
                    authorization: Some((
                        "Authorization".into(),
                        Zeroizing::new(format!("Bearer {}", credential.as_str())),
                    )),
                    network_allowed: request_gate,
                }
            }
        };
        let reply = self.transport.send(request, cancel).map_err(|error| {
            if (self.gate)() && !self.snapshot().is_ok_and(|current| &current == expected) {
                WebError::Conflict
            } else {
                error
            }
        })?;
        self.admit(cancel)?;
        if !self.snapshot().is_ok_and(|current| &current == expected) {
            return Err(WebError::Conflict);
        }
        if !(200..300).contains(&reply.status) {
            return Err(WebError::Http(reply.status));
        }
        if reply.body.len() > 1024 * 1024 {
            return Err(WebError::TooLarge);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&reply.body).map_err(|_| WebError::Response)?;
        let raw = match settings.provider {
            SearchProvider::Brave => value.pointer("/web/results"),
            SearchProvider::Tavily => value.get("results"),
        };
        let raw = raw.and_then(|v| v.as_array()).ok_or(WebError::Response)?;
        let mut results = vec![];
        for item in raw.iter().take(input.count as usize) {
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or(WebError::Response)?;
            if validate_url(url).is_err() {
                continue;
            }
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .ok_or(WebError::Response)?;
            let snippet = item
                .get(if settings.provider == SearchProvider::Brave {
                    "description"
                } else {
                    "content"
                })
                .and_then(|v| v.as_str())
                .unwrap_or("");
            results.push(SearchHit {
                title: bounded(&title.replace(sensitive.as_str(), "[REDACTED]"), 1024),
                url: bounded(&url.replace(sensitive.as_str(), "[REDACTED]"), 8192),
                snippet: bounded(&snippet.replace(sensitive.as_str(), "[REDACTED]"), 4096),
            });
        }
        Ok(SearchOutput {
            query: input.query.replace(sensitive.as_str(), "[REDACTED]"),
            results,
        })
    }
    pub fn fetch(&self, input: &FetchInput, cancel: &CancelToken) -> Result<FetchOutput> {
        self.admit(cancel)?;
        validate_url(&input.url)?;
        let reply = self.transport.send(
            HttpRequest {
                url: input.url.clone(),
                json: None,
                authorization: None,
                network_allowed: self.gate.clone(),
            },
            cancel,
        )?;
        self.admit(cancel)?;
        if !(200..300).contains(&reply.status) {
            return Err(WebError::Http(reply.status));
        }
        if reply.body.len() > 1024 * 1024 {
            return Err(WebError::TooLarge);
        }
        let media = reply.content_type.split(';').next().unwrap_or("").trim();
        if ![
            "text/plain",
            "text/html",
            "application/json",
            "application/xhtml+xml",
        ]
        .contains(&media)
        {
            return Err(WebError::Response);
        }
        let text = std::str::from_utf8(&reply.body).map_err(|_| WebError::Response)?;
        let text = if media == "text/html" || media == "application/xhtml+xml" {
            plain_html(text)
        } else {
            text.to_owned()
        };
        Ok(FetchOutput {
            url: reply.final_url,
            truncated: text.len() > 64 * 1024,
            content: bounded(&text, 64 * 1024),
        })
    }
}
pub(crate) fn validate_search(input: &SearchInput) -> Result<()> {
    if input.query.trim().is_empty()
        || input.query.len() > 2048
        || input.query.chars().any(char::is_control)
        || !(1..=20).contains(&input.count)
        || input
            .freshness
            .as_deref()
            .is_some_and(|f| !["pd", "pw", "pm", "py"].contains(&f))
    {
        return Err(WebError::Invalid);
    }
    Ok(())
}
pub(crate) fn bounded(text: &str, bytes: usize) -> String {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
fn plain_html(html: &str) -> String {
    let mut output = String::new();
    let mut tag = String::new();
    let mut inside = false;
    let mut hidden: Option<String> = None;
    for ch in html.chars() {
        if ch == '<' {
            inside = true;
            tag.clear();
            continue;
        }
        if inside {
            if ch == '>' {
                inside = false;
                let name = tag
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if hidden.is_none() && ["script", "style", "noscript"].contains(&name.as_str()) {
                    hidden = Some(name.clone());
                } else if hidden.as_ref().is_some_and(|h| name == format!("/{h}")) {
                    hidden = None;
                }
                if hidden.is_none() {
                    output.push('\n');
                }
            } else if tag.len() < 128 {
                tag.push(ch);
            }
        } else if hidden.is_none() {
            output.push(ch);
        }
    }
    output
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
}
