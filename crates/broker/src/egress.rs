//! Policy-backed egress and credentials for model providers (spec §12.5, §82,
//! §92).
//!
//! [`ModelEgress`] implements `aidex_providers::EgressGate` and
//! `aidex_providers::CredentialResolver`, so every remote provider request passes
//! the broker: the offline switch first, then an emergency stop, then a
//! `model.invoke` intent (remote locality, classified destination) evaluated under
//! the active profile, privacy mode, network mode, and policy documents. Approval
//! verdicts cannot be shown for a synchronous provider call, so they refuse the call
//! (fail closed) with a hint; allow rules for `model.invoke` lift that.
//!
//! Provider credentials are evaluated as `secret.use` for `provider:<id>`. The
//! user's own binding (a provider configured with a vault secret whose policy names
//! `provider:<id>`) is the standing approval for that consumer, so an approval
//! verdict does not block it, but any deny (explicit rules, untrusted hosts,
//! emergency stop) does. Each use is a 30-second handle recorded as
//! `secret.granted`/`secret.revoked`; the value goes only into the request header.

use std::{fmt, sync::Arc, time::Duration};

use aidex_core::EntityId;
use aidex_events::{EventKind, EventScope};
use aidex_policy::{Action, Intent, ModelLocality, SecretUse, Verdict, classify_network};
use aidex_providers::{
    CredentialError, CredentialResolver, EgressGate, ProviderError, ProviderId, SecretBytes,
};
use aidex_vault::{ConsumerScope, SecretName, VaultError, VaultErrorKind};

use crate::{Broker, BrokerError, audit::events_verdict};

/// Lifetime of the single-use handle issued for one provider request.
const CREDENTIAL_HANDLE_TTL: Duration = Duration::from_secs(30);
/// Port assumed for a remote provider host (providers report the host only).
const HTTPS_PORT: u16 = 443;

/// Model-provider egress through the broker. See the module documentation.
pub struct ModelEgress {
    broker: Arc<Broker>,
    offline: Option<Arc<dyn EgressGate>>,
    /// Provider calls are not tied to a session in the provider API; their intents
    /// use this fixed session id (session-layer rules do not apply to them).
    session: EntityId,
}

impl fmt::Debug for ModelEgress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelEgress")
            .field("offline_switch", &self.offline.is_some())
            .finish_non_exhaustive()
    }
}

impl ModelEgress {
    pub fn new(broker: Arc<Broker>) -> Result<Self, BrokerError> {
        Ok(Self {
            broker,
            offline: None,
            session: EntityId::new("ses_model_egress")
                .map_err(|error| BrokerError::internal("egress session id", error))?,
        })
    }

    /// Consult `gate` (for example `aidex_providers::OfflineMode`) before policy.
    pub fn with_offline_switch(mut self, gate: Arc<dyn EgressGate>) -> Self {
        self.offline = Some(gate);
        self
    }

    fn record(&self, intent_id: EntityId, verdict: &Verdict, rule: String) {
        self.broker.audit().note(
            &EventScope::global(),
            EventKind::PolicyDecision {
                intent_id,
                decision: events_verdict(verdict),
                rule,
            },
        );
    }
}

fn intent_id() -> Option<EntityId> {
    EntityId::generate("int").ok()
}

impl EgressGate for ModelEgress {
    fn check_remote(&self, provider: &ProviderId, host: &str) -> Result<(), ProviderError> {
        if let Some(offline) = &self.offline {
            offline.check_remote(provider, host)?;
        }
        if let Err(stopped) = self.broker.emergency().check() {
            return Err(ProviderError::PolicyDenied {
                reason: stopped.to_string(),
            });
        }
        let target =
            classify_network(host, HTTPS_PORT).map_err(|error| ProviderError::PolicyDenied {
                reason: format!("the provider host cannot be classified: {error}"),
            })?;
        let intent = Intent::new(
            Action::ModelInvoke,
            self.session.clone(),
            format!("call remote model provider `{provider}` at {host}"),
        )
        .with_model_locality(ModelLocality::Remote)
        .with_network(target);
        let decision = self.broker.evaluate_intent(&intent);
        if let Some(id) = intent_id() {
            self.record(id, &decision.verdict, decision.rule.to_string());
        }
        match &decision.verdict {
            Verdict::Allow | Verdict::Sandbox { .. } => Ok(()),
            Verdict::RateLimit { max, per } => {
                let key = format!("model.invoke|{provider}|{}", decision.rule);
                let limit = aidex_policy::RateLimit {
                    max: *max,
                    per: *per,
                };
                if self.broker.rate().try_acquire(&key, limit) {
                    Ok(())
                } else {
                    Err(ProviderError::PolicyDenied {
                        reason: format!("rate limit reached for provider `{provider}`"),
                    })
                }
            }
            Verdict::RequireApproval { .. } => Err(ProviderError::PolicyDenied {
                reason: format!(
                    "remote model calls need approval under the current policy ({}); add a \
                     policy rule allowing `model.invoke` for {host}, or choose another profile",
                    decision.rule
                ),
            }),
            Verdict::Deny => Err(ProviderError::PolicyDenied {
                reason: decision.reason,
            }),
        }
    }
}

fn credential_error(error: &VaultError, secret_name: &str) -> CredentialError {
    let name = || secret_name.to_owned();
    match error.kind() {
        VaultErrorKind::Locked => CredentialError::Locked,
        VaultErrorKind::NotFound => CredentialError::Unavailable {
            reason: "no vault exists on this drive; create one with `aidex vault init`".into(),
        },
        VaultErrorKind::SecretNotFound => CredentialError::NotFound {
            secret_name: name(),
        },
        VaultErrorKind::ConsumerNotAllowed | VaultErrorKind::ForbiddenConsumer => {
            CredentialError::Denied {
                secret_name: name(),
            }
        }
        kind => CredentialError::Unavailable {
            reason: format!("vault error ({})", kind.as_str()),
        },
    }
}

impl CredentialResolver for ModelEgress {
    fn resolve(
        &self,
        provider: &ProviderId,
        secret_name: &str,
    ) -> Result<SecretBytes, CredentialError> {
        if let Err(stopped) = self.broker.emergency().check() {
            return Err(CredentialError::Unavailable {
                reason: stopped.to_string(),
            });
        }
        let vault = self
            .broker
            .secrets()
            .vault()
            .ok_or_else(|| CredentialError::Unavailable {
                reason: "no vault is available to the broker".into(),
            })?;
        let name = SecretName::new(secret_name).map_err(|_| CredentialError::NotFound {
            secret_name: secret_name.to_owned(),
        })?;
        let consumer = ConsumerScope::provider(provider.as_str())
            .map_err(|error| credential_error(&error, secret_name))?;
        let intent = Intent::new(
            Action::SecretUse,
            self.session.clone(),
            format!("use secret `{secret_name}` for {consumer}"),
        )
        .with_secret(SecretUse::new(secret_name, consumer.to_string()));
        let decision = self.broker.evaluate_intent(&intent);
        if let Some(id) = intent_id() {
            self.record(id, &decision.verdict, decision.rule.to_string());
        }
        if decision.verdict.is_deny() {
            return Err(CredentialError::Denied {
                secret_name: secret_name.to_owned(),
            });
        }
        let handle = vault
            .issue_handle(&name, &consumer, CREDENTIAL_HANDLE_TTL)
            .map_err(|error| credential_error(&error, secret_name))?;
        let scope = EventScope::global();
        if let Ok(consumer_id) = EntityId::new(consumer.to_string()) {
            self.broker.audit().note(
                &scope,
                EventKind::SecretGranted {
                    handle_id: handle.handle_id.clone(),
                    secret_name: secret_name.to_owned(),
                    consumer: consumer_id,
                },
            );
        }
        let resolved = vault.resolve(&handle, &consumer);
        vault.revoke_handle(&handle.handle_id);
        self.broker.audit().note(
            &scope,
            EventKind::SecretRevoked {
                handle_id: handle.handle_id.clone(),
                reason: Some("the provider request was prepared".into()),
            },
        );
        let secret = resolved.map_err(|error| credential_error(&error, secret_name))?;
        Ok(SecretBytes::new(secret.expose().to_vec()))
    }
}
