//! Value-free credential evidence for native recovery; never unverified token claims.
use crate::{
    RuntimeAuth,
    state::{State, error},
};
use aidex_auth::AuthService;
use aidex_native_sessions::{RuntimeError, digest};
use aidex_vault::{
    ConsumerScope, HandleDenial, SecretBytes, SecretName, SecretVersion, VaultError,
};
use serde_json::{Value, json};

/// Which credential a launch used: the vault record and its revision, or the
/// verified shared OAuth account (a hash of issuer, subject and client). Its
/// [`evidence`](Self::evidence) is part of the resume reference, so a session
/// or an approval never continues under a different credential than the one
/// that started it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CredentialBinding {
    None,
    Vault { version: SecretVersion },
    CodexOauth { account: String },
}
impl CredentialBinding {
    /// The binding as it appears in a resume reference: a vault record only as
    /// a digest of its id, with its revision, and the OAuth account hash. No key
    /// names a credential (`secret`, `token`, `key`, `auth`, `credential`), so
    /// `aidex_events::redact_text` leaves the reference unchanged and the
    /// harness accepts it as resume metadata.
    pub fn evidence(&self) -> Value {
        match self {
            Self::None => json!({"kind":"none"}),
            Self::Vault { version } => json!({
                "kind": "vault",
                "record": digest(
                    format!(
                        "{}:{}",
                        version.secret_ref.vault_id, version.secret_ref.secret_id
                    )
                    .as_bytes()
                ),
                "revision": version.revision,
            }),
            Self::CodexOauth { account } => json!({"kind":"codex_oauth","account":account}),
        }
    }
    pub fn capture(
        config: &RuntimeAuth,
        state: &State,
        auth: &AuthService,
    ) -> Result<Self, RuntimeError> {
        match config {
            RuntimeAuth::None => Ok(Self::None),
            RuntimeAuth::Vault { secret_name, .. } => {
                let name = SecretName::new(secret_name)
                    .map_err(|_| error("invalid_config", "Invalid secret reference"))?;
                state
                    .vault
                    .secret_version(&name)
                    .map(|version| Self::Vault { version })
                    .map_err(|_| error("auth_required", "Runtime credential is unavailable"))
            }
            RuntimeAuth::CodexOauth { provider_id } => auth
                .account_binding(provider_id)
                .map(|account| Self::CodexOauth { account })
                .map_err(|_| {
                    error(
                        "auth_required",
                        "Connect the shared ChatGPT account before using Codex",
                    )
                }),
        }
    }
    /// Resolve the bound vault credential, refusing once its revision changed.
    pub fn vault_secret(
        &self,
        state: &State,
        runtime_id: &str,
        name: &str,
    ) -> Result<SecretBytes, RuntimeError> {
        let Self::Vault { version } = self else {
            return Err(error(
                "account_changed",
                "Runtime credential binding changed",
            ));
        };
        let name = SecretName::new(name)
            .map_err(|_| error("invalid_config", "Invalid secret reference"))?;
        let consumer = ConsumerScope::process(format!("runtime-{runtime_id}"))
            .map_err(|_| error("invalid_config", "Invalid runtime secret consumer"))?;
        state
            .vault
            .resolve_versioned(&name, &consumer, version)
            .map_err(|e| match e {
                VaultError::HandleDenied(HandleDenial::RevisionChanged) => error(
                    "account_changed",
                    "Runtime credential changed since this launch captured it",
                ),
                _ => error(
                    "auth_required",
                    "Runtime credential is unavailable or consumer denied",
                ),
            })
    }
    pub fn verify(
        &self,
        config: &RuntimeAuth,
        state: &State,
        auth: &AuthService,
    ) -> Result<(), RuntimeError> {
        if *self != Self::capture(config, state, auth)? {
            return Err(error(
                "account_changed",
                "Runtime account changed; begin a new native session",
            ));
        }
        Ok(())
    }
}
