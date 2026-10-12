use crate::{
    oauth::{self, Attempt, Discovery},
    storage::{self, Profile},
};
use aidex_vault::{SecretBytes, VaultService};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Pending,
    Connected,
    Failed,
    Cancelled,
    Expired,
    Disconnected,
}
/// Safe metadata only. Neither token values nor upstream error bodies belong here.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AuthStatus {
    pub state: AuthState,
    pub provider_id: String,
    pub account_label: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StartResult {
    pub flow_id: String,
    pub provider_id: String,
    pub authorization_url: String,
    /// Unix milliseconds; expiry is monotonic.
    pub expires_at: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthError {
    NetworkOffline,
    Locked,
    Invalid,
    Unavailable,
    TokenTransport,
    TokenRejected(u16),
    TokenResponseInvalid,
    IdentityKeysUnavailable,
    Reauthorize,
    NotFound,
    Conflict,
    ConsentDenied,
    PlanPermission,
    IdentityInvalid,
    CallbackInvalid,
    RemoteRevocationUnconfirmed,
}
impl AuthError {
    pub fn code(self) -> &'static str {
        match self {
            Self::NetworkOffline => "offline_refused",
            Self::Locked => "vault_locked",
            Self::Invalid => "invalid_auth_request",
            Self::Unavailable => "auth_unavailable",
            Self::TokenTransport => "token_exchange_network_failed",
            Self::TokenRejected(_) => "token_exchange_rejected",
            Self::TokenResponseInvalid => "invalid_token_response",
            Self::IdentityKeysUnavailable => "identity_keys_unavailable",
            Self::Reauthorize => "reauthorization_required",
            Self::NotFound => "auth_not_found",
            Self::Conflict => "auth_pending",
            Self::ConsentDenied => "consent_denied",
            Self::PlanPermission => "chatgpt_plan_permission_missing",
            Self::IdentityInvalid => "invalid_identity_token",
            Self::CallbackInvalid => "invalid_auth_callback",
            Self::RemoteRevocationUnconfirmed => "remote_revocation_unconfirmed",
        }
    }
}
impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::NetworkOffline => "Offline mode blocks ChatGPT network access.",
            Self::Locked => "Unlock the vault before using ChatGPT sign-in.",
            Self::Invalid => "The ChatGPT sign-in request is invalid.",
            Self::Unavailable => "ChatGPT sign-in is unavailable. Try again.",
            Self::TokenTransport => {
                "The ChatGPT token exchange could not reach the server. Check the network and sign in again."
            }
            Self::TokenRejected(status) => {
                return write!(
                    f,
                    "ChatGPT rejected the token exchange (HTTP {status}). Start a fresh sign-in."
                );
            }
            Self::TokenResponseInvalid => {
                "ChatGPT returned an unsupported token response. The sign-in credentials were not activated."
            }
            Self::IdentityKeysUnavailable => {
                "ChatGPT identity verification keys could not be loaded. Check the network and sign in again."
            }
            Self::Reauthorize => "The ChatGPT connection needs a new sign-in.",
            Self::NotFound => "The ChatGPT sign-in flow was not found.",
            Self::Conflict => "A ChatGPT sign-in is already pending for this account.",
            Self::ConsentDenied => "ChatGPT authorization was declined.",
            Self::PlanPermission => {
                "ChatGPT plan usage permission was not granted. Sign in again and approve plan usage."
            }
            Self::IdentityInvalid => "The ChatGPT account identity could not be verified.",
            Self::CallbackInvalid => "The ChatGPT sign-in callback could not be verified.",
            Self::RemoteRevocationUnconfirmed => {
                "Signed out locally. Remote revocation was not confirmed; disconnect the app in ChatGPT Settings."
            }
        };
        f.write_str(message)
    }
}
impl std::error::Error for AuthError {}
struct Flow {
    status: AuthStatus,
    url: String,
    deadline: Instant,
}
/// RPC and credential resolution share one daemon-owned service. Startup does not
/// load secrets, so a locked portable vault can open normally.
pub struct AuthService {
    network_allowed: Arc<dyn Fn() -> bool + Send + Sync>,
    vault: Arc<VaultService>,
    discovery: String,
    host_key: Result<String, AuthError>,
    ttl: Duration,
    http: reqwest::blocking::Client,
    flows: Mutex<HashMap<String, Arc<Mutex<Flow>>>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    host_lock: Mutex<()>,
}
impl fmt::Debug for AuthService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthService").finish_non_exhaustive()
    }
}
impl AuthService {
    /// Construct with network denied. Daemon composition uses with_network_gate
    /// to supply explicit admission from its shared offline switch.
    pub fn new(vault: Arc<VaultService>) -> Result<Self, AuthError> {
        Ok(Self {
            network_allowed: Arc::new(|| false),
            vault,
            discovery: oauth::DISCOVERY.into(),
            host_key: storage::native_host_key(),
            ttl: Duration::from_secs(600),
            http: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| AuthError::Unavailable)?,
            flows: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            host_lock: Mutex::new(()),
        })
    }
    /// Explicit network admission, normally the daemon's shared OfflineMode.
    pub fn with_network_gate(
        vault: Arc<VaultService>,
        network_allowed: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self, AuthError> {
        let mut service = Self::new(vault)?;
        service.network_allowed = network_allowed;
        Ok(service)
    }
    fn provider_lock(&self, provider: &str) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(provider.into())
            .or_default()
            .clone()
    }
    fn profile(&self, provider: &str) -> Result<Option<Profile>, AuthError> {
        storage::read_profile(&self.vault, provider)
    }
    fn flow(&self, id: &str) -> Result<Arc<Mutex<Flow>>, AuthError> {
        self.flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or(AuthError::NotFound)
    }
    /// Callback composes the native registry only after token validation/storage.
    pub fn start(
        self: &Arc<Self>,
        provider: &str,
        connected: impl Fn(&str, &str) -> Result<(), AuthError> + Send + Sync + 'static,
    ) -> Result<StartResult, AuthError> {
        storage::validate_provider(provider)?;
        let lock = self.provider_lock(provider);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let profile = self.profile(provider)?;
        if !(self.network_allowed)() {
            return Err(AuthError::NetworkOffline);
        }
        // Keep vault/offline policy errors primary; unsupported host identity
        // still denies sign-in before discovery or any network effects.
        let host_key = self.host_key.as_ref().map_err(|error| *error)?;
        {
            let mut flows = self.flows.lock().unwrap_or_else(|e| e.into_inner());
            if flows.values().any(|f| {
                let f = f.lock().unwrap_or_else(|e| e.into_inner());
                f.status.provider_id == provider && f.status.state == AuthState::Pending
            }) {
                return Err(AuthError::Conflict);
            }
            if flows.len() >= 64 {
                flows.retain(|_, f| {
                    f.lock().unwrap_or_else(|e| e.into_inner()).status.state == AuthState::Pending
                });
                if flows.len() >= 64 {
                    return Err(AuthError::Conflict);
                }
            }
        }
        let endpoints = oauth::discover(&self.http, &self.discovery, &*self.network_allowed)?;
        let host_id = {
            let _host = self.host_lock.lock().unwrap_or_else(|e| e.into_inner());
            storage::host_id(&self.vault, host_key)?
        };
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|_| AuthError::Unavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| AuthError::Unavailable)?;
        let callback = format!(
            "http://127.0.0.1:{}/auth/callback",
            listener
                .local_addr()
                .map_err(|_| AuthError::Unavailable)?
                .port()
        );
        let attempt = Attempt::new(callback, profile.as_ref(), &host_id, &endpoints)?;
        let flow_id = oauth::random_string()?;
        let result = StartResult {
            flow_id: flow_id.clone(),
            provider_id: provider.into(),
            authorization_url: attempt.url.clone(),
            expires_at: oauth::now()
                .saturating_mul(1000)
                .saturating_add(self.ttl.as_millis() as u64),
        };
        let flow = Arc::new(Mutex::new(Flow {
            status: AuthStatus {
                state: AuthState::Pending,
                provider_id: provider.into(),
                account_label: None,
                error: None,
            },
            url: attempt.url.clone(),
            deadline: Instant::now() + self.ttl,
        }));
        self.flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(flow_id, flow.clone());
        let weak = Arc::downgrade(self);
        let provider = provider.to_owned();
        let worker_flow = flow.clone();
        let spawn = thread::Builder::new()
            .name("aidex-oauth-loopback".into())
            .spawn(move || {
                let Some(callback) = receive_callback(listener, &worker_flow, &weak) else {
                    return;
                };
                let Some(service) = weak.upgrade() else {
                    return;
                };
                let outcome = callback.and_then(|url| {
                    service.complete(
                        &provider,
                        &worker_flow,
                        &attempt,
                        &endpoints,
                        &url,
                        &connected,
                    )
                });
                let mut flow = worker_flow.lock().unwrap_or_else(|e| e.into_inner());
                if flow.status.state == AuthState::Pending {
                    if Instant::now() >= flow.deadline {
                        flow.status.state = AuthState::Expired;
                        return;
                    }
                    match outcome {
                        Ok(label) => {
                            flow.status.state = AuthState::Connected;
                            flow.status.account_label = label;
                        }
                        Err(error) => {
                            flow.status.state = AuthState::Failed;
                            flow.status.error = Some(error.to_string());
                        }
                    }
                }
            });
        if spawn.is_err() {
            let mut flow = flow.lock().unwrap_or_else(|e| e.into_inner());
            flow.status.state = AuthState::Failed;
            flow.status.error = Some(AuthError::Unavailable.to_string());
            return Err(AuthError::Unavailable);
        }
        Ok(result)
    }
    fn complete(
        &self,
        provider: &str,
        flow: &Arc<Mutex<Flow>>,
        attempt: &Attempt,
        endpoints: &Discovery,
        url: &url::Url,
        connected: &impl Fn(&str, &str) -> Result<(), AuthError>,
    ) -> Result<Option<String>, AuthError> {
        let lock = self.provider_lock(provider);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (client, code) = attempt.callback(url)?;
        if !pending(flow) {
            return Err(AuthError::Invalid);
        }
        let previous = self.profile(provider)?;
        // Issued registration survives failed exchange (for example invalid_grant).
        let mut profile = previous
            .clone()
            .unwrap_or_else(|| Profile::registration(client.clone()));
        if profile.client_id != client {
            return Err(AuthError::CallbackInvalid);
        }
        storage::write_profile(&self.vault, provider, &profile)?;
        let response = oauth::exchange(
            &self.http,
            endpoints,
            &client,
            &code,
            &attempt.verifier,
            &attempt.callback,
            &*self.network_allowed,
        )?;
        let identity = oauth::validate_identity(
            &self.http,
            endpoints,
            &response,
            &client,
            &attempt.nonce,
            &*self.network_allowed,
        )?;
        if profile
            .subject
            .as_ref()
            .is_some_and(|subject| subject != &identity.sub)
        {
            return Err(AuthError::IdentityInvalid);
        }
        // Hold flow lock through persistence; cancel cannot race a successful commit.
        let flow_guard = flow.lock().unwrap_or_else(|e| e.into_inner());
        if flow_guard.status.state != AuthState::Pending || Instant::now() >= flow_guard.deadline {
            return Err(AuthError::Invalid);
        }
        profile.subject = Some(identity.sub);
        profile.email = identity.email;
        profile.issuer = Some(endpoints.issuer.clone());
        profile.tokens = Some(response.into_tokens(None)?);
        profile.needs_reauth = false;
        storage::write_profile(&self.vault, provider, &profile)?;
        storage::write_access(
            &self.vault,
            provider,
            profile.tokens.as_ref().unwrap().access_token.as_bytes(),
        )?;
        if let Err(error) = connected(provider, &Self::secret_name(provider)) {
            if let Some(previous) = previous.filter(|p| p.tokens.is_some()) {
                storage::write_profile(&self.vault, provider, &previous)?;
                storage::write_access(
                    &self.vault,
                    provider,
                    previous.tokens.as_ref().unwrap().access_token.as_bytes(),
                )?;
            } else {
                storage::clear_tokens(&self.vault, provider, &mut profile)?;
            }
            return Err(error);
        }
        drop(flow_guard);
        Ok(profile.email.clone().or(profile.subject.clone()))
    }
    pub fn status(
        &self,
        flow_id: Option<&str>,
        provider: Option<&str>,
    ) -> Result<AuthStatus, AuthError> {
        if let Some(id) = flow_id {
            let flow = self.flow(id)?;
            pending(&flow);
            let status = flow
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .status
                .clone();
            if status.state == AuthState::Connected {
                return self.status(None, Some(&status.provider_id));
            }
            return Ok(status);
        }
        let provider = provider.ok_or(AuthError::Invalid)?;
        storage::validate_provider(provider)?;
        let latest = self
            .flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter_map(|f| {
                let flow = f.lock().unwrap_or_else(|e| e.into_inner());
                (flow.status.provider_id == provider).then_some((flow.deadline, f.clone()))
            })
            .max_by_key(|(deadline, _)| *deadline)
            .map(|(_, flow)| flow);
        if let Some(flow) = latest {
            pending(&flow);
            let status = flow
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .status
                .clone();
            if status.state != AuthState::Connected {
                return Ok(status);
            }
        }
        Ok(match self.profile(provider)? {
            Some(profile) => AuthStatus {
                state: if profile.tokens.is_some() && !profile.needs_reauth {
                    AuthState::Connected
                } else {
                    AuthState::Disconnected
                },
                provider_id: provider.into(),
                account_label: profile.email.clone().or(profile.subject.clone()),
                error: profile
                    .needs_reauth
                    .then(|| AuthError::Reauthorize.to_string()),
            },
            None => AuthStatus {
                state: AuthState::Disconnected,
                provider_id: provider.into(),
                account_label: None,
                error: None,
            },
        })
    }
    pub fn cancel(&self, id: &str) -> Result<AuthStatus, AuthError> {
        let flow = self.flow(id)?;
        let mut flow = flow.lock().unwrap_or_else(|e| e.into_inner());
        if flow.status.state == AuthState::Pending {
            flow.status.state = AuthState::Cancelled;
        }
        Ok(flow.status.clone())
    }
    pub fn disconnect(&self, provider: &str) -> Result<AuthStatus, AuthError> {
        storage::validate_provider(provider)?;
        let lock = self.provider_lock(provider);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut profile = self.profile(provider)?.ok_or(AuthError::NotFound)?;
        for flow in self
            .flows
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            let mut flow = flow.lock().unwrap_or_else(|e| e.into_inner());
            if flow.status.provider_id == provider {
                flow.status.state = AuthState::Disconnected;
            }
        }
        let revocation = if let Some(tokens) = profile
            .tokens
            .as_ref()
            .filter(|t| t.refresh_token.is_some())
        {
            oauth::discover(&self.http, &self.discovery, &*self.network_allowed).and_then(
                |endpoints| {
                    oauth::revoke(
                        &self.http,
                        &endpoints,
                        &profile.client_id,
                        tokens.refresh_token.as_deref().unwrap(),
                        &*self.network_allowed,
                    )
                },
            )
        } else {
            Ok(())
        };
        storage::clear_tokens(&self.vault, provider, &mut profile)?;
        let mut status = self.status(None, Some(provider))?;
        if revocation.is_err() {
            status.error = Some(AuthError::RemoteRevocationUnconfirmed.to_string());
        }
        Ok(status)
    }
    pub fn secret_name(provider: &str) -> String {
        format!("oauth.{provider}.access")
    }
    /// Backend-only, value-free identity of the connected account: SHA-256 over
    /// the provider id and the verified issuer, subject and issued client id of
    /// its encrypted profile. Token refresh does not change it; another account
    /// or registration does. Fails closed: `NotFound` without a profile,
    /// `Reauthorize` when signed out or awaiting sign-in, `IdentityInvalid` when
    /// the verified identity is incomplete. Never returned over RPC.
    pub fn account_binding(&self, provider: &str) -> Result<String, AuthError> {
        storage::validate_provider(provider)?;
        let lock = self.provider_lock(provider);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let profile = self.profile(provider)?.ok_or(AuthError::NotFound)?;
        if profile.needs_reauth
            || profile
                .tokens
                .as_ref()
                .is_none_or(|tokens| tokens.access_token.is_empty())
        {
            return Err(AuthError::Reauthorize);
        }
        let (Some(issuer), Some(subject)) = (
            profile.issuer.as_deref().filter(|v| !v.is_empty()),
            profile.subject.as_deref().filter(|v| !v.is_empty()),
        ) else {
            return Err(AuthError::IdentityInvalid);
        };
        if profile.client_id.is_empty() {
            return Err(AuthError::IdentityInvalid);
        }
        let identity = serde_json::to_vec(&(provider, issuer, subject, &profile.client_id))
            .map_err(|_| AuthError::Unavailable)?;
        Ok(format!("{:x}", Sha256::digest(identity)))
    }
    /// Designated names never fall back to a stale raw vault token.
    pub fn owns_secret(provider: &str, name: &str) -> bool {
        name == Self::secret_name(provider)
    }
    pub fn resolve(&self, provider: &str, name: &str) -> Result<SecretBytes, AuthError> {
        storage::validate_provider(provider)?;
        if !Self::owns_secret(provider, name) {
            return Err(AuthError::Invalid);
        }
        let lock = self.provider_lock(provider);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut profile = self.profile(provider)?.ok_or(AuthError::NotFound)?;
        if profile.needs_reauth {
            return Err(AuthError::Reauthorize);
        }
        let tokens = profile.tokens.as_ref().ok_or(AuthError::Reauthorize)?;
        let now = oauth::now();
        let refresh_due = now.saturating_add(60) >= tokens.expires_at;
        let permitted = tokens
            .earliest_refresh_at
            .is_none_or(|earliest| now >= earliest);
        if refresh_due && permitted {
            let refresh = tokens
                .refresh_token
                .as_deref()
                .ok_or(AuthError::Reauthorize)?;
            let endpoints = oauth::discover(&self.http, &self.discovery, &*self.network_allowed)?;
            match oauth::refresh(
                &self.http,
                &endpoints,
                &profile.client_id,
                refresh,
                &*self.network_allowed,
            ) {
                Ok(response) => {
                    let next = response.into_tokens(Some(tokens))?;
                    profile.tokens = Some(next);
                    storage::write_profile(&self.vault, provider, &profile)?;
                }
                Err(error @ (AuthError::Reauthorize | AuthError::PlanPermission)) => {
                    profile.needs_reauth = true;
                    storage::clear_tokens(&self.vault, provider, &mut profile)?;
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        } else if now >= tokens.expires_at {
            return Err(AuthError::Unavailable);
        }
        let tokens = profile.tokens.as_ref().unwrap();
        storage::write_access_if_changed(&self.vault, provider, tokens.access_token.as_bytes())?;
        storage::read_access(&self.vault, provider)
    }
    pub fn validate_open_url(&self, url: &str) -> Result<(), AuthError> {
        let flows = self.flows.lock().unwrap_or_else(|e| e.into_inner());
        if flows.values().any(|flow| {
            pending(flow);
            let flow = flow.lock().unwrap_or_else(|e| e.into_inner());
            flow.status.state == AuthState::Pending && flow.url == url
        }) {
            Ok(())
        } else {
            Err(AuthError::Invalid)
        }
    }
    /// Open the exact URL of a pending sign-in flow through
    /// [`open_in_browser`](crate::open_in_browser): fixed command text, the URL
    /// as environment data, and this service's network gate.
    pub fn open(&self, url: &str) -> Result<(), AuthError> {
        self.validate_open_url(url)?;
        crate::browser::open_in_browser(url, &*self.network_allowed)
    }
}
fn receive_callback(
    listener: TcpListener,
    flow: &Arc<Mutex<Flow>>,
    service: &std::sync::Weak<AuthService>,
) -> Option<Result<url::Url, AuthError>> {
    loop {
        if !pending(flow) || service.strong_count() == 0 {
            return None;
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let parsed = read_callback(&mut stream, flow);
                let valid_path = parsed
                    .as_ref()
                    .is_ok_and(|url| url.path() == "/auth/callback");
                let body = if valid_path {
                    "AiDEX received the sign-in response. You may close this window."
                } else {
                    "Invalid callback."
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                    if valid_path {
                        "200 OK"
                    } else {
                        "400 Bad Request"
                    },
                    body.len(),
                    body
                );
                if valid_path {
                    return Some(parsed);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(_) => return Some(Err(AuthError::Unavailable)),
        }
    }
}
fn pending(flow: &Arc<Mutex<Flow>>) -> bool {
    let mut flow = flow.lock().unwrap_or_else(|e| e.into_inner());
    if flow.status.state == AuthState::Pending && Instant::now() >= flow.deadline {
        flow.status.state = AuthState::Expired;
    }
    flow.status.state == AuthState::Pending
}
fn read_callback(stream: &mut TcpStream, flow: &Arc<Mutex<Flow>>) -> Result<url::Url, AuthError> {
    stream
        .set_nonblocking(false)
        .map_err(|_| AuthError::CallbackInvalid)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .map_err(|_| AuthError::CallbackInvalid)?;
    let mut input = Vec::new();
    let mut chunk = [0; 1024];
    let deadline = Instant::now() + Duration::from_secs(2);
    while input.len() < 8192 && Instant::now() < deadline && pending(flow) {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                input.extend_from_slice(&chunk[..n]);
                if input.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(_) => return Err(AuthError::CallbackInvalid),
        }
    }
    if input.len() >= 8192 {
        return Err(AuthError::CallbackInvalid);
    }
    let text = std::str::from_utf8(&input).map_err(|_| AuthError::CallbackInvalid)?;
    let mut parts = text
        .lines()
        .next()
        .ok_or(AuthError::CallbackInvalid)?
        .split_whitespace();
    if parts.next() != Some("GET") {
        return Err(AuthError::CallbackInvalid);
    }
    let target = parts.next().ok_or(AuthError::CallbackInvalid)?;
    if !target.starts_with('/') || target.starts_with("//") {
        return Err(AuthError::CallbackInvalid);
    }
    url::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|_| AuthError::CallbackInvalid)
}
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
