//! The synchronous provider contract.

use aidex_core::{CancelToken, Timestamp};
use serde::{Deserialize, Serialize};

use crate::{
    AuthConfig, ChatRequest, ChatResponse, Locality, ModelDescriptor, ProviderError,
    ProviderErrorKind, ProviderId, ProviderKind, StreamEvent,
};

/// What a provider is and how it is reached. Contains no credential.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub kind: ProviderKind,
    /// The endpoint, when the provider is reached over HTTP.
    pub base_url: Option<String>,
    pub locality: Locality,
    pub auth: AuthConfig,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// The provider answered.
    Ready,
    /// Configure/unlock a vault credential or authenticate the account.
    AuthRequired,
    /// The provider was contacted (or a credential was needed) and failed.
    Unavailable,
    /// Nothing was contacted because offline mode or policy refused it.
    Refused,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderHealth {
    pub status: HealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ProviderErrorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    pub checked_at: Timestamp,
}

impl ProviderHealth {
    pub fn ready(latency_ms: Option<u64>) -> Self {
        Self {
            status: HealthStatus::Ready,
            error: None,
            message: None,
            latency_ms,
            checked_at: Timestamp::now(),
        }
    }

    /// Health for a failed check; refusals that opened no connection are `Refused`.
    pub fn from_error(error: &ProviderError) -> Self {
        let status = match error {
            ProviderError::Credential(_) | ProviderError::Unauthorized => {
                HealthStatus::AuthRequired
            }
            ProviderError::OfflineRefused { .. } | ProviderError::PolicyDenied { .. } => {
                HealthStatus::Refused
            }
            _ => HealthStatus::Unavailable,
        };
        Self {
            status,
            error: Some(error.kind()),
            message: Some(error.to_string()),
            latency_ms: None,
            checked_at: Timestamp::now(),
        }
    }
}

/// A source of models (spec §96: the harness talks to providers, never to a
/// runtime directly).
///
/// Implementations are synchronous and shareable across threads. Every method
/// that may reach a remote endpoint consults the provider's egress gate first,
/// and credentials are resolved per call, never stored.
pub trait ModelProvider: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;

    /// Models this provider serves now. Discovery failures are errors, never an
    /// empty list.
    fn models(&self) -> Result<Vec<ModelDescriptor>, ProviderError>;

    /// Run one chat completion, streaming increments to `on_event` as they
    /// arrive and returning the assembled response. Must return promptly with
    /// [`ProviderError::Cancelled`] (after emitting `Done(Cancelled)`) once
    /// `cancel` is cancelled, and with [`ProviderError::Timeout`] once its
    /// deadline passes.
    fn chat(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(&StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResponse, ProviderError>;

    /// A bounded liveness check.
    fn health(&self) -> ProviderHealth;

    /// Drop any cached model listing so the next [`ModelProvider::models`]
    /// call asks the provider again. Providers without a cache do nothing.
    fn invalidate_models(&self) {}
}
