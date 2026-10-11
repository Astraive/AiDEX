//! Value-free credential evidence for native recovery; never unverified token claims.
use crate::{
    RuntimeAuth,
    state::{State, error},
};
use aidex_auth::AuthService;
use aidex_native_sessions::RuntimeError;
use aidex_vault::{ConsumerScope, SecretBytes, SecretName, SecretVersion};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CredentialBinding {
    None,
    Vault { version: SecretVersion },
    CodexOauth { account: String },
}
impl CredentialBinding {
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
                        "Shared OAuth account identity is unavailable",
                    )
                }),
        }
    }
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
            .map_err(|_| error("invalid_config", "Invalid runtime credential consumer"))?;
        state
            .vault
            .resolve_versioned(&name, &consumer, version)
            .map_err(|_| {
                error(
                    "account_changed",
                    "Runtime credential changed or is unavailable",
                )
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
