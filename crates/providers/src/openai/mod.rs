//! OpenAI-compatible HTTP provider (`/models`, `/chat/completions` with SSE).
//!
//! Works with OpenAI and any server implementing the same API (vLLM,
//! llama.cpp `llama-server`, LM Studio, Ollama's `/v1`, OpenRouter, ...).
//! [`OpenAiClient`] is the bare HTTP client (usable by runtime providers that
//! manage their own server and key); [`OpenAiCompatibleProvider`] adds the
//! configured descriptor, the egress gate, and per-request vault credentials.

mod eventstream;
pub(crate) mod sse;
pub(crate) mod transport;
mod url;
pub(crate) mod wire;

use std::{
    fmt,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use aidex_core::CancelToken;
use zeroize::Zeroizing;

pub use url::{BaseUrl, InvalidUrl, Scheme};
pub use wire::DiscoveredModel;

use crate::{
    AuthConfig, ChatRequest, ChatResponse, CredentialError, CredentialResolver, EgressGate,
    FinishReason, Locality, ModelConfig, ModelDescriptor, ModelProvider, ProviderConfig,
    ProviderDescriptor, ProviderError, ProviderHealth, SecretBytes, StreamEvent,
};
use transport::{Exchange, Frame, HttpRequest, Limits, Method};

/// HTTP timeouts and size limits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HttpOptions {
    pub connect_timeout: Duration,
    /// Time to the response headers (prompt processing on a CPU can be slow).
    pub response_timeout: Duration,
    /// Upper bound on reading one response body; also bounds how long an
    /// abandoned (cancelled) exchange can linger on a silent server.
    pub body_timeout: Duration,
    /// Deadline for `/models` listings and health checks.
    pub list_timeout: Duration,
    pub max_body_bytes: u64,
    pub max_line_bytes: usize,
    /// How long a successful model listing is reused (both HTTP adapters);
    /// [`crate::ModelProvider::invalidate_models`] drops it early.
    pub model_cache_ttl: Duration,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            response_timeout: Duration::from_secs(300),
            body_timeout: Duration::from_secs(30 * 60),
            list_timeout: Duration::from_secs(15),
            max_body_bytes: 16 * 1024 * 1024,
            max_line_bytes: 1024 * 1024,
            model_cache_ttl: Duration::from_secs(60),
        }
    }
}

/// A blocking OpenAI-API client for one base URL.
///
/// Redirects are never followed (a redirect could carry the `Authorization`
/// header elsewhere); environment proxies are ignored for loopback endpoints.
#[derive(Clone)]
pub struct OpenAiClient {
    base: BaseUrl,
    agent: ureq::Agent,
    options: HttpOptions,
}

impl fmt::Debug for OpenAiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiClient")
            .field("base_url", &self.base.to_string())
            .finish_non_exhaustive()
    }
}

impl OpenAiClient {
    pub fn new(base: BaseUrl, options: HttpOptions) -> Self {
        let proxy = if base.is_loopback() {
            None
        } else {
            ureq::Proxy::try_from_env()
        };
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(proxy)
            .timeout_connect(Some(options.connect_timeout))
            .timeout_recv_response(Some(options.response_timeout))
            .timeout_recv_body(Some(options.body_timeout))
            .user_agent(concat!("aidex/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            base,
            agent: config.into(),
            options,
        }
    }

    pub fn base_url(&self) -> &BaseUrl {
        &self.base
    }

    fn limits(&self) -> Limits {
        Limits {
            max_body: self.options.max_body_bytes,
            max_line: self.options.max_line_bytes,
        }
    }

    /// `GET /models` within the list timeout.
    pub fn list_models(
        &self,
        credential: Option<&SecretBytes>,
        cancel: &CancelToken,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        self.list_models_with(bearer(credential)?, cancel)
    }

    fn list_models_with(
        &self,
        authorization: Option<Zeroizing<String>>,
        cancel: &CancelToken,
    ) -> Result<Vec<DiscoveredModel>, ProviderError> {
        let cancel = cancel.with_timeout(self.options.list_timeout);
        let exchange = Exchange::start(
            &self.agent,
            HttpRequest {
                url: self.base.join("models"),
                method: Method::Get,
                authorization,
                headers: Vec::new(),
                stream: false,
            },
            self.limits(),
            &cancel,
        )?;
        let Frame::Head {
            status,
            retry_after,
            ..
        } = exchange.next(&cancel)?
        else {
            return Err(unexpected());
        };
        let body = read_body(&exchange, &cancel)?;
        if (200..300).contains(&status) {
            wire::parse_models(&body)
        } else {
            Err(wire::http_error(status, retry_after, &body, None))
        }
    }

    /// `POST /chat/completions` with streaming; see [`ModelProvider::chat`].
    pub fn chat(
        &self,
        request: &ChatRequest,
        credential: Option<&SecretBytes>,
        on_event: &mut dyn FnMut(&StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResponse, ProviderError> {
        if let Err(error) = transport::check_cancel(cancel) {
            if matches!(error, ProviderError::Cancelled) {
                on_event(&StreamEvent::Done {
                    finish_reason: FinishReason::Cancelled,
                });
            }
            return Err(error);
        }
        let authorization = bearer(credential)?;
        self.chat_with(request, authorization, on_event, cancel)
    }

    fn chat_with(
        &self,
        request: &ChatRequest,
        authorization: Option<Zeroizing<String>>,
        on_event: &mut dyn FnMut(&StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResponse, ProviderError> {
        let result = transport::check_cancel(cancel)
            .and_then(|()| request.validate())
            .and_then(|()| {
                crate::input::check_request(crate::ProviderKind::OpenaiCompatible, request)
            })
            .and_then(|()| {
                let exchange = Exchange::start(
                    &self.agent,
                    HttpRequest {
                        url: self.base.join("chat/completions"),
                        method: Method::Post(wire::request_body_with(
                            request,
                            true,
                            wire::is_openrouter(self.base.host()),
                        )),
                        authorization,
                        headers: Vec::new(),
                        stream: true,
                    },
                    self.limits(),
                    cancel,
                )?;
                consume_chat(&exchange, request, on_event, cancel)
            });
        if matches!(result, Err(ProviderError::Cancelled)) {
            on_event(&StreamEvent::Done {
                finish_reason: FinishReason::Cancelled,
            });
        }
        result
    }
}

fn bearer(credential: Option<&SecretBytes>) -> Result<Option<Zeroizing<String>>, ProviderError> {
    credential
        .map(|secret| {
            secret.bearer_header().ok_or_else(|| {
                ProviderError::Credential(CredentialError::Unavailable {
                    reason: "the credential is not a valid bearer token".into(),
                })
            })
        })
        .transpose()
}

fn unexpected() -> ProviderError {
    ProviderError::Protocol("unexpected HTTP exchange state".into())
}

fn read_body(exchange: &Exchange, cancel: &CancelToken) -> Result<String, ProviderError> {
    match exchange.next(cancel)? {
        Frame::Body(body) => Ok(body),
        Frame::End => Ok(String::new()),
        _ => Err(unexpected()),
    }
}

fn consume_chat(
    exchange: &Exchange,
    request: &ChatRequest,
    on_event: &mut dyn FnMut(&StreamEvent),
    cancel: &CancelToken,
) -> Result<ChatResponse, ProviderError> {
    let Frame::Head {
        status,
        retry_after,
        event_stream,
    } = exchange.next(cancel)?
    else {
        return Err(unexpected());
    };
    if !(200..300).contains(&status) {
        let body = read_body(exchange, cancel)?;
        return Err(wire::http_error(
            status,
            retry_after,
            &body,
            Some(&request.model),
        ));
    }
    if !event_stream {
        return wire::parse_completion(&read_body(exchange, cancel)?, on_event);
    }
    let mut decoder = sse::SseDecoder::default();
    let mut accumulator = sse::StreamAccumulator::default();
    loop {
        match exchange.next(cancel)? {
            Frame::Line(line) => {
                if let Some(payload) = decoder.push_line(&line)? {
                    accumulator.apply(&payload, on_event)?;
                    if accumulator.is_done() {
                        break;
                    }
                }
            }
            Frame::End => {
                if let Some(payload) = decoder.flush() {
                    accumulator.apply(&payload, on_event)?;
                }
                break;
            }
            _ => return Err(unexpected()),
        }
    }
    accumulator.finish(on_event)
}

/// A configured OpenAI-compatible endpoint.
///
/// Locality is derived from the endpoint: loopback hosts are local unless
/// configured `remote`; every other host is remote and cannot be declared
/// local. Remote calls pass the [`EgressGate`] before any socket is opened and
/// before the credential is resolved. Credentials are sent only over `https`
/// (or to loopback).
pub struct OpenAiCompatibleProvider {
    descriptor: ProviderDescriptor,
    client: OpenAiClient,
    declared: Vec<ModelConfig>,
    discover: bool,
    resolver: Arc<dyn CredentialResolver>,
    gate: Arc<dyn EgressGate>,
    cache: Mutex<Option<(Instant, Vec<ModelDescriptor>)>>,
}

/// A provider configuration that cannot be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InvalidProvider(pub String);

impl fmt::Display for InvalidProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidProvider {}

impl OpenAiCompatibleProvider {
    pub fn from_config(
        config: &ProviderConfig,
        resolver: Arc<dyn CredentialResolver>,
        gate: Arc<dyn EgressGate>,
    ) -> Result<Self, InvalidProvider> {
        Self::with_options(config, resolver, gate, HttpOptions::default())
    }

    pub fn with_options(
        config: &ProviderConfig,
        resolver: Arc<dyn CredentialResolver>,
        gate: Arc<dyn EgressGate>,
        options: HttpOptions,
    ) -> Result<Self, InvalidProvider> {
        config.validate().map_err(InvalidProvider)?;
        if config.kind != crate::ProviderKind::OpenaiCompatible {
            return Err(InvalidProvider(
                "OpenAiCompatibleProvider requires openai-compatible kind".into(),
            ));
        }
        let base = BaseUrl::parse(&config.base_url)
            .map_err(|error| InvalidProvider(format!("provider `{}`: {error}", config.id)))?;
        let locality = match (base.is_loopback(), config.locality) {
            (true, locality) => locality.unwrap_or(Locality::Local),
            (false, Some(Locality::Local)) => {
                return Err(InvalidProvider(format!(
                    "provider `{}`: only loopback endpoints can be local; {} is reachable over a network",
                    config.id,
                    base.host()
                )));
            }
            (false, _) => Locality::Remote,
        };
        if base.scheme() == Scheme::Http
            && !base.is_loopback()
            && matches!(config.auth, AuthConfig::VaultSecret { .. })
        {
            return Err(InvalidProvider(format!(
                "provider `{}`: credentials are only sent over https to non-loopback hosts",
                config.id
            )));
        }
        Ok(Self {
            descriptor: ProviderDescriptor {
                id: config.id.clone(),
                kind: config.kind,
                base_url: Some(base.to_string()),
                locality,
                auth: config.auth.clone(),
            },
            client: OpenAiClient::new(base, options),
            declared: config.models.clone(),
            discover: config.discover_models,
            resolver,
            gate,
            cache: Mutex::new(None),
        })
    }

    /// Consult the egress gate for remote endpoints (no I/O happens first).
    fn admit(&self) -> Result<(), ProviderError> {
        if self.descriptor.locality == Locality::Remote {
            self.gate
                .check_remote(&self.descriptor.id, self.client.base_url().host())?;
        }
        Ok(())
    }

    fn authorization(&self) -> Result<Option<Zeroizing<String>>, ProviderError> {
        match &self.descriptor.auth {
            AuthConfig::None => Ok(None),
            AuthConfig::VaultSecret { secret_name } => {
                let secret = self.resolver.resolve(&self.descriptor.id, secret_name)?;
                secret.bearer_header().map(Some).ok_or_else(|| {
                    ProviderError::Credential(CredentialError::Malformed {
                        secret_name: secret_name.clone(),
                    })
                })
            }
        }
    }

    fn declared_descriptors(&self) -> Vec<ModelDescriptor> {
        self.declared
            .iter()
            .map(|model| model.descriptor(&self.descriptor.id, self.descriptor.locality))
            .collect()
    }

    fn merge(&self, discovered: Vec<DiscoveredModel>) -> Vec<ModelDescriptor> {
        wire::merge_discovered(
            &self.descriptor.id,
            self.descriptor.locality,
            &self.declared,
            discovered,
        )
    }
}

impl fmt::Debug for OpenAiCompatibleProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiCompatibleProvider")
            .field("descriptor", &self.descriptor)
            .finish_non_exhaustive()
    }
}

impl ModelProvider for OpenAiCompatibleProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn models(&self) -> Result<Vec<ModelDescriptor>, ProviderError> {
        if !self.discover {
            return Ok(self.declared_descriptors());
        }
        // Gate first even when cached, so an offline listing reports the refusal.
        self.admit()?;
        let ttl = self.client.options.model_cache_ttl;
        if let Ok(cache) = self.cache.lock()
            && let Some((at, models)) = cache.as_ref()
            && at.elapsed() < ttl
        {
            return Ok(models.clone());
        }
        let discovered = self
            .client
            .list_models_with(self.authorization()?, &CancelToken::new())?;
        let models = self.merge(discovered);
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some((Instant::now(), models.clone()));
        }
        Ok(models)
    }

    fn chat(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(&StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResponse, ProviderError> {
        let prepared = transport::check_cancel(cancel)
            .and_then(|()| request.validate())
            .and_then(|()| self.admit())
            .and_then(|()| self.authorization());
        match prepared {
            Ok(authorization) => self
                .client
                .chat_with(request, authorization, on_event, cancel),
            Err(error) => {
                if matches!(error, ProviderError::Cancelled) {
                    on_event(&StreamEvent::Done {
                        finish_reason: FinishReason::Cancelled,
                    });
                }
                Err(error)
            }
        }
    }

    fn health(&self) -> ProviderHealth {
        let started = Instant::now();
        let result = self.admit().and_then(|()| {
            self.client
                .list_models_with(self.authorization()?, &CancelToken::new())
        });
        match result {
            Ok(_) => ProviderHealth::ready(Some(
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            )),
            Err(error) => ProviderHealth::from_error(&error),
        }
    }

    fn invalidate_models(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            *cache = None;
        }
    }
}
