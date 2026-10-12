//! Protocol-specific cloud providers sharing bounded HTTP transport.
mod bedrock;
mod events;
#[cfg(test)]
mod golden_tests;
mod listing;
mod requests;

use crate::openai::{
    sse::SseDecoder,
    transport::{Exchange, Frame, HttpRequest, Limits, Method},
    wire,
};
use crate::*;
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use zeroize::Zeroizing;

pub(crate) struct CloudProvider {
    descriptor: ProviderDescriptor,
    config: ProviderConfig,
    base: BaseUrl,
    resolver: Arc<dyn CredentialResolver>,
    gate: Arc<dyn EgressGate>,
    agent: ureq::Agent,
    options: HttpOptions,
    /// The last successful listing, reused for `options.model_cache_ttl`.
    cache: Mutex<Option<(Instant, Vec<ModelDescriptor>)>>,
}

impl CloudProvider {
    pub fn new(
        config: &ProviderConfig,
        resolver: Arc<dyn CredentialResolver>,
        gate: Arc<dyn EgressGate>,
        options: HttpOptions,
    ) -> Result<Self, String> {
        config.validate()?;
        let base = if config.kind == ProviderKind::Bedrock {
            BaseUrl::parse_root(&config.base_url)
        } else {
            BaseUrl::parse(&config.base_url)
        }
        .map_err(|e| e.to_string())?;
        let locality = if base.is_loopback() && config.locality != Some(Locality::Remote) {
            Locality::Local
        } else {
            Locality::Remote
        };
        if !base.is_loopback() && config.locality == Some(Locality::Local) {
            return Err("remote endpoints cannot be classified local".into());
        }
        if base.scheme() == Scheme::Http && !base.is_loopback() && config.auth != AuthConfig::None {
            return Err("credentials require HTTPS or loopback".into());
        }
        if matches!(
            config.kind,
            ProviderKind::AzureOpenai | ProviderKind::Bedrock
        ) && config.discover_models
        {
            return Err("deployment/model IDs must be configured for Azure/Bedrock; disable model discovery".into());
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .max_redirects_will_error(false)
            .proxy(if base.is_loopback() {
                None
            } else {
                ureq::Proxy::try_from_env()
            })
            .timeout_connect(Some(options.connect_timeout))
            .timeout_recv_response(Some(options.response_timeout))
            .timeout_recv_body(Some(options.body_timeout))
            .build()
            .into();
        Ok(Self {
            descriptor: ProviderDescriptor {
                id: config.id.clone(),
                kind: config.kind,
                base_url: Some(base.to_string()),
                locality,
                auth: config.auth.clone(),
            },
            config: config.clone(),
            base,
            resolver,
            gate,
            agent,
            options,
            cache: Mutex::new(None),
        })
    }

    fn headers(&self) -> Result<Vec<(String, Zeroizing<String>)>, ProviderError> {
        if self.descriptor.locality == Locality::Remote {
            self.gate
                .check_remote(&self.descriptor.id, self.base.host())?;
        }
        let mut headers = Vec::new();
        if self.config.kind == ProviderKind::Anthropic {
            headers.push((
                "anthropic-version".into(),
                Zeroizing::new("2023-06-01".into()),
            ));
        }
        if let AuthConfig::VaultSecret { secret_name } = &self.config.auth {
            let secret = self.resolver.resolve(&self.config.id, secret_name)?;
            let bearer = secret.bearer_header().ok_or_else(|| {
                ProviderError::Credential(CredentialError::Malformed {
                    secret_name: secret_name.clone(),
                })
            })?;
            let (name, value) = match self.config.kind {
                ProviderKind::Anthropic => ("x-api-key", Zeroizing::new(bearer[7..].to_owned())),
                ProviderKind::Gemini => ("x-goog-api-key", Zeroizing::new(bearer[7..].to_owned())),
                ProviderKind::AzureOpenai => ("api-key", Zeroizing::new(bearer[7..].to_owned())),
                _ => ("authorization", bearer),
            };
            headers.push((name.into(), value));
        } else if self.descriptor.locality == Locality::Remote {
            return Err(ProviderError::Credential(CredentialError::Unavailable {
                reason: "authentication required; configure a vault secret".into(),
            }));
        }
        Ok(headers)
    }
    fn exchange(
        &self,
        endpoint: &str,
        method: Method,
        stream: bool,
        cancel: &CancelToken,
    ) -> Result<Exchange, ProviderError> {
        crate::openai::transport::check_cancel(cancel)?;
        let url = self.base.join(endpoint);
        let headers = if self.config.kind == ProviderKind::Bedrock {
            if self.descriptor.locality == Locality::Remote {
                self.gate.check_remote(&self.config.id, self.base.host())?;
            }
            let AuthConfig::VaultSecret { secret_name } = &self.config.auth else {
                return Err(ProviderError::Credential(CredentialError::Unavailable {
                    reason: "Bedrock requires AWS credentials in a vault secret".into(),
                }));
            };
            let secret = self.resolver.resolve(&self.config.id, secret_name)?;
            let body = match &method {
                Method::Post(body) => body.as_slice(),
                _ => &[],
            };
            bedrock::sign(&url, body, &secret)?
        } else {
            self.headers()?
        };
        Exchange::start(
            &self.agent,
            HttpRequest {
                url,
                method,
                authorization: None,
                headers,
                stream,
            },
            Limits {
                max_body: self.options.max_body_bytes,
                max_line: self.options.max_line_bytes,
            },
            cancel,
        )
    }
    fn run_chat(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(&StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResponse, ProviderError> {
        request.validate()?;
        if let Some(reason) = cancel.state() {
            return Err(if reason == CancelReason::Cancelled {
                ProviderError::Cancelled
            } else {
                ProviderError::Timeout
            });
        }
        let (endpoint, body) = requests::build(self.config.kind, request)?;
        let exchange =
            self.exchange(
                &endpoint,
                Method::Post(serde_json::to_vec(&body).map_err(|_| {
                    ProviderError::InvalidRequest("request cannot be encoded".into())
                })?),
                true,
                cancel,
            )?;
        let Frame::Head {
            status,
            retry_after,
            event_stream,
        } = exchange.next(cancel)?
        else {
            return Err(protocol());
        };
        if !(200..300).contains(&status) {
            let body = match exchange.next(cancel)? {
                Frame::Body(body) => body,
                _ => String::new(),
            };
            return Err(wire::http_error(
                status,
                retry_after,
                &body,
                Some(&request.model),
            ));
        }
        if !event_stream {
            return Err(ProviderError::Protocol(
                "cloud endpoint did not return an event stream".into(),
            ));
        }
        let mut decoder = SseDecoder::default();
        let mut assembler = events::Assembler::default();
        loop {
            match exchange.next(cancel)? {
                Frame::Line(line) => {
                    if let Some(payload) = decoder.push_line(&line)? {
                        assembler.apply(self.config.kind, &payload, on_event)?;
                    }
                    if assembler.done {
                        break;
                    }
                }
                Frame::End => {
                    if let Some(payload) = decoder.flush() {
                        assembler.apply(self.config.kind, &payload, on_event)?;
                    }
                    break;
                }
                Frame::Event {
                    event_type,
                    payload,
                } if self.config.kind == ProviderKind::Bedrock => {
                    assembler.bedrock(&event_type, &payload, on_event)?
                }
                _ => return Err(protocol()),
            }
        }
        assembler.finish(on_event)
    }

    /// `GET endpoint` and return the 2xx body.
    fn fetch(&self, endpoint: &str, cancel: &CancelToken) -> Result<String, ProviderError> {
        let exchange = self.exchange(endpoint, Method::Get, false, cancel)?;
        let Frame::Head {
            status,
            retry_after,
            ..
        } = exchange.next(cancel)?
        else {
            return Err(protocol());
        };
        let body = match exchange.next(cancel)? {
            Frame::Body(body) => body,
            Frame::End => String::new(),
            _ => return Err(protocol()),
        };
        if !(200..300).contains(&status) {
            return Err(wire::http_error(status, retry_after, &body, None));
        }
        Ok(body)
    }

    /// The vendor's model listing, following pagination for Anthropic and
    /// Gemini (at most [`listing::MAX_PAGES`] pages within one list timeout).
    fn discover(&self) -> Result<Vec<DiscoveredModel>, ProviderError> {
        let cancel = CancelToken::new().with_timeout(self.options.list_timeout);
        type PageParser = fn(&str) -> Result<listing::Page, ProviderError>;
        let (first, cursor_param, parse): (&str, &str, PageParser) = match self.config.kind {
            ProviderKind::Anthropic => ("models?limit=1000", "after_id", listing::anthropic_page),
            ProviderKind::Gemini => ("models?pageSize=1000", "pageToken", listing::gemini_page),
            _ => {
                let body = self.fetch("models", &cancel)?;
                let value: serde_json::Value =
                    serde_json::from_str(&body).map_err(|_| protocol())?;
                // ChatGPT plan tokens use an account-specific catalog envelope
                // at the same public /models route; API-key accounts retain
                // data/id discovery.
                return if self.config.kind == ProviderKind::OpenaiResponses
                    && value.get("models").is_some()
                {
                    listing::chatgpt_catalog(&value)
                } else {
                    wire::parse_model_list(&value)
                };
            }
        };
        let mut models = Vec::new();
        let mut endpoint = first.to_owned();
        for _ in 0..listing::MAX_PAGES {
            let page = parse(&self.fetch(&endpoint, &cancel)?)?;
            models.extend(page.models);
            let Some(next) = page.next else {
                break;
            };
            endpoint = format!("{first}&{cursor_param}={}", requests::encode_segment(&next));
        }
        Ok(models)
    }
}
pub(super) fn protocol() -> ProviderError {
    ProviderError::Protocol("malformed or incomplete provider stream".into())
}

impl ModelProvider for CloudProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }
    fn models(&self) -> Result<Vec<ModelDescriptor>, ProviderError> {
        if !self.config.discover_models {
            return Ok(self
                .config
                .models
                .iter()
                .map(|m| m.descriptor(&self.config.id, self.descriptor.locality))
                .collect());
        }
        // Gate first even when cached, so an offline listing reports the refusal.
        if self.descriptor.locality == Locality::Remote {
            self.gate
                .check_remote(&self.descriptor.id, self.base.host())?;
        }
        if let Ok(cache) = self.cache.lock()
            && let Some((at, models)) = cache.as_ref()
            && at.elapsed() < self.options.model_cache_ttl
        {
            return Ok(models.clone());
        }
        let models = wire::merge_discovered(
            &self.config.id,
            self.descriptor.locality,
            &self.config.models,
            self.discover()?,
        );
        if let Ok(mut cache) = self.cache.lock() {
            *cache = Some((Instant::now(), models.clone()));
        }
        Ok(models)
    }
    fn invalidate_models(&self) {
        if let Ok(mut cache) = self.cache.lock() {
            *cache = None;
        }
    }
    fn chat(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(&StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResponse, ProviderError> {
        let result = self.run_chat(request, on_event, cancel);
        if matches!(result, Err(ProviderError::Cancelled)) {
            on_event(&StreamEvent::Done {
                finish_reason: FinishReason::Cancelled,
            });
        }
        result
    }
    fn health(&self) -> ProviderHealth {
        let start = Instant::now();
        // Health means a live server answered, not merely a configured model list.
        let endpoint = if self.config.kind == ProviderKind::Bedrock {
            let credentials = (|| {
                if self.descriptor.locality == Locality::Remote {
                    self.gate.check_remote(&self.config.id, self.base.host())?;
                }
                let AuthConfig::VaultSecret { secret_name } = &self.config.auth else {
                    return Err(ProviderError::Credential(CredentialError::Unavailable {
                        reason: "Bedrock requires AWS credentials in a vault secret".into(),
                    }));
                };
                let secret = self.resolver.resolve(&self.config.id, secret_name)?;
                bedrock::sign(&self.base.join("models"), &[], &secret).map(|_| ())
            })();
            if let Err(error) = credentials {
                return ProviderHealth::from_error(&error);
            }
            return ProviderHealth::from_error(&ProviderError::InvalidRequest("Bedrock health requires an explicit inference request; no free liveness endpoint exists".into()));
        } else if self.config.kind == ProviderKind::AzureOpenai {
            "models?api-version=2024-10-21"
        } else {
            "models"
        };
        let token = CancelToken::new().with_timeout(self.options.list_timeout);
        let result = self
            .exchange(endpoint, Method::Get, false, &token)
            .and_then(|exchange| match exchange.next(&token)? {
                Frame::Head {
                    status,
                    retry_after,
                    ..
                } if !(200..300).contains(&status) => {
                    Err(wire::http_error(status, retry_after, "", None))
                }
                Frame::Head { .. } => Ok(()),
                _ => Err(protocol()),
            });
        match result {
            Ok(()) => ProviderHealth::ready(Some(start.elapsed().as_millis() as u64)),
            Err(e) => ProviderHealth::from_error(&e),
        }
    }
}
