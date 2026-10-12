//! Web search and fetch composition: the drive's `private_config/web.toml`,
//! provider keys released through vault handles for `process:web`, and an
//! egress gate consulted before every connection (redirect hops included).
//!
//! The gate refuses when the offline switch is on or an emergency stop is in
//! effect, then evaluates a `network.request` intent for the classified
//! destination under the broker's active settings (profile, privacy mode,
//! network mode) and the drive's policy documents. Approval verdicts cannot
//! be shown for a synchronous request, so they refuse it (fail closed) with a
//! hint; allow rules for `network.request` lift that. Every evaluation is
//! recorded as `policy.decision`, and every released key as
//! `secret.granted`/`secret.revoked`.
//!
//! The documents are read from the broker's policies directory for each
//! decision (they are small), so a document that fails to load refuses web
//! requests just as it denies broker tool calls.

use std::{
    collections::{HashMap, VecDeque},
    fmt,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use aidex_broker::Broker;
use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use aidex_events::{EventKind, EventScope, EventSink};
use aidex_policy::{
    Action, Decision, Intent, PolicySet, RateLimit, SecretUse, Verdict, classify_network,
};
use aidex_providers::OfflineMode;
use aidex_vault::{ConsumerScope, SecretName, VaultError, VaultErrorKind, VaultService};
use aidex_web::{
    CREDENTIAL_TOOL_ID, CredentialProblem, CredentialRequest, EgressPurpose, EgressRequest, Secret,
    WebClient, WebError, WebSettings, WebSettingsPatch,
};

use super::ServicesError;

/// Lifetime of the single-use handle issued for one provider request.
const CREDENTIAL_HANDLE_TTL: Duration = Duration::from_secs(30);

/// The daemon's web services.
pub struct WebServices {
    paths: DrivePaths,
    gate: Arc<WebGate>,
    /// Serializes read-modify-write settings changes.
    settings_lock: Mutex<()>,
}

impl WebServices {
    pub fn open(
        paths: &DrivePaths,
        broker: Arc<Broker>,
        offline: Arc<OfflineMode>,
        vault: Arc<VaultService>,
        events: Arc<dyn EventSink>,
    ) -> Result<Self, ServicesError> {
        let session = EntityId::new("ses_web_egress").map_err(|error| {
            ServicesError::new(
                "web",
                aidex_core::ServiceErrorCode::Internal,
                error.to_string(),
            )
        })?;
        Ok(Self {
            paths: paths.clone(),
            gate: Arc::new(WebGate {
                broker,
                offline,
                vault,
                events,
                session,
                rate: Mutex::new(HashMap::new()),
            }),
            settings_lock: Mutex::new(()),
        })
    }

    /// The drive's settings (defaults when the file does not exist).
    pub fn settings(&self) -> Result<WebSettings, WebError> {
        WebSettings::load(&self.paths)
    }

    /// Apply `patch` to the stored settings and save them atomically.
    pub fn update(&self, patch: &WebSettingsPatch) -> Result<WebSettings, WebError> {
        let _guard = self
            .settings_lock
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let next = patch.apply(&WebSettings::load(&self.paths)?)?;
        next.save(&self.paths)?;
        Ok(next)
    }

    /// A client for the stored settings, gated by this drive's policy.
    pub fn client(&self) -> Result<WebClient, WebError> {
        self.client_for(self.settings()?)
    }

    /// A client for `settings` (for example unsaved ones under test), gated by
    /// this drive's policy and releasing keys from its vault.
    pub fn client_for(&self, settings: WebSettings) -> Result<WebClient, WebError> {
        let credentials = Arc::clone(&self.gate);
        let egress = Arc::clone(&self.gate);
        WebClient::new(
            settings,
            move |request: &CredentialRequest<'_>| credentials.credential(request),
            move |request: &EgressRequest<'_>| egress.check(request),
        )
    }

    /// Whether the offline switch currently refuses web requests.
    pub fn is_offline(&self) -> bool {
        self.gate.offline.is_offline()
    }
}

impl fmt::Debug for WebServices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebServices").finish_non_exhaustive()
    }
}

/// Policy, offline switch, and vault access shared by every client.
struct WebGate {
    broker: Arc<Broker>,
    offline: Arc<OfflineMode>,
    vault: Arc<VaultService>,
    events: Arc<dyn EventSink>,
    /// Web requests are not tied to a session yet; their intents use this
    /// fixed session id (session-layer rules do not apply to them).
    session: EntityId,
    rate: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl WebGate {
    fn check(&self, request: &EgressRequest<'_>) -> Result<(), WebError> {
        if self.offline.is_offline() {
            return Err(WebError::Offline);
        }
        if let Some(reason) = self.broker.emergency_reason() {
            return Err(WebError::EgressDenied(format!(
                "an emergency stop is in effect: {reason}"
            )));
        }
        let host = request.host;
        let target = classify_network(host, request.port).map_err(|error| {
            WebError::EgressDenied(format!("the destination cannot be classified: {error}"))
        })?;
        let what = match request.purpose {
            EgressPurpose::Search(provider) => format!("search the web with {provider}"),
            EgressPurpose::Fetch => "fetch a web page".to_owned(),
        };
        let intent = Intent::new(
            Action::NetworkRequest,
            self.session.clone(),
            format!("{what} at {host}"),
        )
        .with_network(target);
        let decision = self.evaluate(&intent)?;
        match decision.verdict {
            Verdict::Allow | Verdict::Sandbox { .. } => Ok(()),
            Verdict::RateLimit { max, per } => {
                let key = format!("network.request|web|{}", decision.rule);
                if self.try_acquire(&key, RateLimit { max, per }) {
                    Ok(())
                } else {
                    Err(WebError::EgressDenied(format!(
                        "the policy rate limit for web requests was reached ({})",
                        decision.rule
                    )))
                }
            }
            Verdict::RequireApproval { .. } => Err(WebError::EgressDenied(format!(
                "web requests to {host} need approval under the current policy ({}); add a \
                 policy rule allowing `network.request` for {host}, or change the network mode",
                decision.rule
            ))),
            Verdict::Deny => Err(WebError::EgressDenied(decision.reason)),
        }
    }

    fn credential(&self, request: &CredentialRequest<'_>) -> Result<Secret, WebError> {
        let secret_name = request.secret_name;
        let problem =
            |problem: CredentialProblem, message: String| WebError::Credential { problem, message };
        if let Some(reason) = self.broker.emergency_reason() {
            return Err(problem(
                CredentialProblem::Unavailable,
                format!("an emergency stop is in effect: {reason}"),
            ));
        }
        let name = SecretName::new(secret_name).map_err(|_| {
            problem(
                CredentialProblem::NotFound,
                format!("`{secret_name}` is not a valid secret name"),
            )
        })?;
        let consumer = ConsumerScope::process(CREDENTIAL_TOOL_ID)
            .map_err(|error| vault_problem(&error, secret_name))?;
        let intent = Intent::new(
            Action::SecretUse,
            self.session.clone(),
            format!("use secret `{secret_name}` for {consumer}"),
        )
        .with_secret(SecretUse::new(secret_name, consumer.to_string()));
        // The user's own binding (a key stored for `process:web` and named in
        // the web settings) is the standing approval; only a deny blocks.
        let decision = self.evaluate(&intent)?;
        if decision.verdict.is_deny() {
            return Err(problem(
                CredentialProblem::Denied,
                format!(
                    "the policy denied using `{secret_name}`: {}",
                    decision.reason
                ),
            ));
        }
        let handle = self
            .vault
            .issue_handle(&name, &consumer, CREDENTIAL_HANDLE_TTL)
            .map_err(|error| vault_problem(&error, secret_name))?;
        let scope = EventScope::global();
        if let Ok(consumer_id) = EntityId::new(consumer.to_string()) {
            let _ = self.events.emit(
                scope.clone(),
                EventKind::SecretGranted {
                    handle_id: handle.handle_id.clone(),
                    secret_name: secret_name.to_owned(),
                    consumer: consumer_id,
                },
            );
        }
        let resolved = self.vault.resolve(&handle, &consumer);
        self.vault.revoke_handle(&handle.handle_id);
        let _ = self.events.emit(
            scope,
            EventKind::SecretRevoked {
                handle_id: handle.handle_id.clone(),
                reason: Some("the web search request was prepared".into()),
            },
        );
        let secret = resolved.map_err(|error| vault_problem(&error, secret_name))?;
        Secret::from_bytes(secret.expose())
    }

    /// Evaluate `intent` under the broker's settings and the drive's policy
    /// documents, recording the decision.
    fn evaluate(&self, intent: &Intent) -> Result<Decision, WebError> {
        let documents = match self.broker.config().policies_dir.as_deref() {
            Some(dir) => PolicySet::load(dir),
            None => Ok(PolicySet::new()),
        }
        .map_err(|error| {
            WebError::EgressDenied(format!(
                "policy documents failed to load, so web requests are refused: {error}"
            ))
        })?;
        let decision =
            aidex_policy::evaluate(&documents, &self.broker.settings().context(), intent);
        if let Ok(intent_id) = EntityId::generate("int") {
            // Best effort, as for model egress: a lost audit event never
            // turns a decision around.
            let _ = self.events.emit(
                EventScope::global(),
                EventKind::PolicyDecision {
                    intent_id,
                    decision: events_verdict(&decision.verdict),
                    rule: decision.rule.to_string(),
                },
            );
        }
        Ok(decision)
    }

    /// Record one request under `key` if `limit` allows it.
    fn try_acquire(&self, key: &str, limit: RateLimit) -> bool {
        let now = Instant::now();
        let mut windows = self.rate.lock().unwrap_or_else(PoisonError::into_inner);
        let window = windows.entry(key.to_owned()).or_default();
        while window
            .front()
            .is_some_and(|started| now.duration_since(*started) >= limit.per)
        {
            window.pop_front();
        }
        if window.len() >= limit.max as usize {
            return false;
        }
        window.push_back(now);
        true
    }
}

fn events_verdict(verdict: &Verdict) -> aidex_events::Verdict {
    match verdict {
        Verdict::Allow => aidex_events::Verdict::Allow,
        Verdict::Deny => aidex_events::Verdict::Deny,
        Verdict::Sandbox { .. } => aidex_events::Verdict::Sandbox,
        Verdict::RequireApproval { .. } => aidex_events::Verdict::RequireApproval,
        Verdict::RateLimit { .. } => aidex_events::Verdict::RateLimit,
    }
}

fn vault_problem(error: &VaultError, secret_name: &str) -> WebError {
    let (problem, message) = match error.kind() {
        VaultErrorKind::Locked => (
            CredentialProblem::Locked,
            "the vault is locked; unlock it to use the search provider key".to_owned(),
        ),
        VaultErrorKind::NotFound => (
            CredentialProblem::Unavailable,
            "no vault exists on this drive; create one with `aidex vault init`".to_owned(),
        ),
        VaultErrorKind::SecretNotFound => (
            CredentialProblem::NotFound,
            format!("no secret named `{secret_name}` exists in the vault"),
        ),
        VaultErrorKind::ConsumerNotAllowed | VaultErrorKind::ForbiddenConsumer => (
            CredentialProblem::Denied,
            format!(
                "the secret `{secret_name}` is not available to `process:{CREDENTIAL_TOOL_ID}`; \
                 store it with that consumer"
            ),
        ),
        kind => (
            CredentialProblem::Unavailable,
            format!("vault error ({})", kind.as_str()),
        ),
    };
    WebError::Credential { problem, message }
}
