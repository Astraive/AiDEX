//! Secret handles for tool calls (spec §15.3, §92).
//!
//! The broker finds the vault secret a plan needs (today: an HTTPS git credential
//! whose policy allows `git:<host>`), evaluates `secret.use` like any other intent,
//! and only after a permit issues a short-lived handle scoped to that consumer. The
//! launcher resolves the handle immediately before spawning and places the value in
//! the child's environment (never in argv, events, plans, cards, or tool output).
//! Handles are revoked as soon as the call ends.

use std::{fmt, sync::Arc, time::Duration};

use aidex_vault::{ConsumerScope, SecretBytes, SecretHandle, SecretName, VaultService};

use crate::{BrokerError, digest::base64, git_remote::GitRemote};

/// How a resolved secret reaches the process that uses it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Injection {
    /// An HTTP `Authorization: Basic` header for git, scoped to one URL prefix through
    /// `GIT_CONFIG_*` environment variables (git ≥ 2.31). Username `x-access-token`,
    /// the stored secret as password (personal access tokens).
    GitHttpHeader { url_prefix: String },
}

/// A secret a plan will use, by name, consumer, and delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialUse {
    pub secret_name: SecretName,
    pub consumer: ConsumerScope,
    pub injection: Injection,
}

/// A handle issued for one call.
#[derive(Clone)]
pub(crate) struct IssuedSecret {
    pub handle: SecretHandle,
    pub secret_name: SecretName,
    pub injection: Injection,
}

impl fmt::Debug for IssuedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IssuedSecret")
            .field("handle_id", &self.handle.handle_id)
            .field("secret_name", &self.secret_name.as_str())
            .field("consumer", &self.handle.consumer.to_string())
            .finish()
    }
}

/// Environment variables for a launch; values may be secret. Not `Debug`/`Clone`.
pub(crate) struct SecretEnv(pub Vec<(String, String)>);

/// The broker's view of the vault.
#[derive(Clone, Default)]
pub(crate) struct Secrets {
    vault: Option<Arc<VaultService>>,
}

impl Secrets {
    pub fn new(vault: Option<Arc<VaultService>>) -> Self {
        Self { vault }
    }

    pub fn vault(&self) -> Option<&Arc<VaultService>> {
        self.vault.as_ref()
    }

    /// The stored credential for an HTTPS git remote: the first secret (by name)
    /// whose policy allows `git:<host[:port]>`. `None` when there is no vault, it is
    /// locked, or no secret allows the consumer (the push then runs without one).
    pub fn git_credential(&self, remote: &GitRemote) -> Option<CredentialUse> {
        let consumer = remote.credential_consumer()?;
        let url_prefix = remote.url_prefix()?;
        let mut candidates: Vec<SecretName> = self
            .vault
            .as_ref()?
            .list_secrets()
            .ok()?
            .into_iter()
            .filter(|meta| meta.policy.allows(&consumer))
            .map(|meta| meta.name)
            .collect();
        candidates.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let secret_name = candidates.into_iter().next()?;
        Some(CredentialUse {
            secret_name,
            consumer,
            injection: Injection::GitHttpHeader { url_prefix },
        })
    }

    /// Issue one handle per use, valid for `ttl`.
    pub fn issue(
        &self,
        uses: &[CredentialUse],
        ttl: Duration,
    ) -> Result<Vec<IssuedSecret>, BrokerError> {
        if uses.is_empty() {
            return Ok(Vec::new());
        }
        let vault = self.vault.as_ref().ok_or_else(|| {
            BrokerError::Internal("a credential is required but no vault is available".into())
        })?;
        let mut issued = Vec::with_capacity(uses.len());
        for use_ in uses {
            match vault.issue_handle(&use_.secret_name, &use_.consumer, ttl) {
                Ok(handle) => issued.push(IssuedSecret {
                    handle,
                    secret_name: use_.secret_name.clone(),
                    injection: use_.injection.clone(),
                }),
                Err(error) => {
                    self.revoke(&issued);
                    return Err(BrokerError::Internal(format!(
                        "cannot issue a handle for secret `{}`: {error}",
                        use_.secret_name.as_str()
                    )));
                }
            }
        }
        Ok(issued)
    }

    /// Resolve handles into launch environment variables (inside the broker only).
    pub fn environment(&self, issued: &[IssuedSecret]) -> Result<SecretEnv, String> {
        let mut env = Vec::new();
        if issued.is_empty() {
            return Ok(SecretEnv(env));
        }
        let vault = self
            .vault
            .as_ref()
            .ok_or_else(|| "no vault is available".to_owned())?;
        let mut git_headers = Vec::new();
        for secret in issued {
            let value: SecretBytes = vault
                .resolve(&secret.handle, &secret.handle.consumer)
                .map_err(|error| {
                    format!(
                        "secret `{}` could not be resolved: {error}",
                        secret.secret_name.as_str()
                    )
                })?;
            match &secret.injection {
                Injection::GitHttpHeader { url_prefix } => {
                    let token = std::str::from_utf8(value.expose())
                        .map(str::trim)
                        .ok()
                        .filter(|token| {
                            !token.is_empty() && token.bytes().all(|b| b.is_ascii_graphic())
                        })
                        .ok_or_else(|| {
                            format!(
                                "secret `{}` is not a printable token",
                                secret.secret_name.as_str()
                            )
                        })?;
                    let credential = base64(format!("x-access-token:{token}").as_bytes());
                    git_headers.push((
                        format!("http.{url_prefix}.extraHeader"),
                        format!("Authorization: Basic {credential}"),
                    ));
                }
            }
        }
        if !git_headers.is_empty() {
            // Reset credential helpers so no other credential is consulted or stored,
            // and ignore system config so the URL the broker classified is the one used.
            let mut entries = vec![("credential.helper".to_owned(), String::new())];
            entries.extend(git_headers);
            env.push(("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()));
            env.push(("GIT_CONFIG_COUNT".to_owned(), entries.len().to_string()));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                env.push((format!("GIT_CONFIG_KEY_{index}"), key));
                env.push((format!("GIT_CONFIG_VALUE_{index}"), value));
            }
        }
        Ok(SecretEnv(env))
    }

    /// Revoke issued handles (best effort; the vault may have locked meanwhile).
    pub fn revoke(&self, issued: &[IssuedSecret]) -> usize {
        let Some(vault) = &self.vault else { return 0 };
        issued
            .iter()
            .filter(|secret| vault.revoke_handle(&secret.handle.handle_id))
            .count()
    }
}
