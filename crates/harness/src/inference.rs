//! One selected-model attempt and at most one user-selected transient fallback.
//!
//! Each attempt gets its own copy of the turn's request, prepared for the
//! model it goes to: attachments as parts, wrapped text, or placeholders
//! ([`crate::attachments`]), historical tool calls the request does not offer
//! flattened to text, and empty assistant placeholders dropped
//! ([`crate::history`]).
use crate::{
    NativeEngine,
    attachments::TurnAttachments,
    history::{drop_empty_assistants, flatten_tool_history},
    untrusted::ensure_preamble,
};
use aidex_core::{CancelToken, EntityId};
use aidex_events::EventKind;
use aidex_native_sessions::{Mode, Run, SessionConfig};
use aidex_providers::{
    CapabilitySource, ChatRequest, ChatResponse, ModelDescriptor, ModelProvider, ModelRef,
    PDF_PAGE_TOKENS, ProviderError, RegistryError, StreamEvent, ToolCallSupport,
};
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

/// What every attempt of one turn shares.
pub(crate) struct Turn {
    /// Index of the run transcript in the request's messages.
    pub start: usize,
    /// The run's delimiter nonce.
    pub nonce: String,
    /// History index of the assistant placeholder the reply streams into.
    pub index: Option<usize>,
    pub attachments: TurnAttachments,
}

/// A refusal that may come from attachments (images, PDFs, or text) of
/// earlier messages: the attempt is repeated once without them.
fn refused_for_size_or_content(error: &ProviderError) -> bool {
    matches!(
        error,
        ProviderError::InvalidRequest(_)
            | ProviderError::ContextOverflow { .. }
            | ProviderError::Http {
                status: 400 | 413,
                ..
            }
    )
}

impl NativeEngine {
    fn validate_model(
        &self,
        target: &ModelRef,
        config: &SessionConfig,
    ) -> Result<(Arc<dyn ModelProvider>, ModelDescriptor), ProviderError> {
        let absent = || ProviderError::unavailable("configured model provider is absent");
        let provider = self
            .registry
            .provider(&target.provider_id)
            .ok_or_else(absent)?;
        // List only the run's own provider; its listing failure is the error.
        let models =
            self.registry
                .models_for(&target.provider_id)
                .map_err(|error| match error {
                    RegistryError::Provider { error, .. } => error,
                    _ => absent(),
                })?;
        let model = models.iter().find(|m| m.model == *target).ok_or_else(|| {
            ProviderError::InvalidRequest("configured model is not in the provider catalog".into())
        })?;
        if !model.capabilities.chat || !model.capabilities.streaming {
            return Err(ProviderError::InvalidRequest(
                "configured model does not support streamed chat".into(),
            ));
        }
        if config.mode == Mode::Agent && model.capabilities.tool_calls != ToolCallSupport::Native {
            return Err(ProviderError::InvalidRequest(
                "native agent requires a model with native tool calling".into(),
            ));
        }
        Ok((provider, model.clone()))
    }
    fn attempt(
        &self,
        provider: &dyn ModelProvider,
        request: &ChatRequest,
        run: &Run,
        index: Option<usize>,
        cancel: &CancelToken,
        pdf_page_tokens: u32,
    ) -> Result<ChatResponse, ProviderError> {
        if cancel.is_stopped() {
            return Err(ProviderError::Cancelled);
        }
        self.store
            .update(&run.session_id, |s| {
                let run = s
                    .runs
                    .iter_mut()
                    .find(|r| r.id == run.id)
                    .ok_or("run not found")?;
                run.effective_max_tokens = request.max_tokens;
                run.estimated_input_tokens =
                    Some(request.estimated_input_tokens_with(pdf_page_tokens));
                run.model_used = Some(format!("{}/{}", provider.descriptor().id, request.model));
                Ok(())
            })
            .map_err(|_| {
                ProviderError::InvalidRequest("private session store unavailable".into())
            })?;
        if let Some(index) = index {
            self.store
                .update(&run.session_id, |s| {
                    s.messages[index].content.clear();
                    Ok(())
                })
                .map_err(|_| {
                    ProviderError::InvalidRequest("private session store unavailable".into())
                })?;
        }
        let scope = self.store.get(&run.session_id).ok().and_then(|s| {
            let workspace = s
                .configuration
                .workspace_id
                .unwrap_or_else(|| "workspace_chat".into());
            Some(aidex_events::EventScope::session(
                EntityId::new(workspace).ok()?,
                EntityId::new(&run.session_id).ok()?,
            ))
        });
        let mut partial = String::new();
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut saved = 0;
        provider.chat(
            request,
            &mut |event| {
                if let StreamEvent::TextDelta { text } = event {
                    partial.push_str(text);
                    if let Some(scope) = &scope {
                        let _ = self
                            .events
                            .emit(scope.clone(), EventKind::AgentDelta { text: text.clone() });
                    }
                    if last.elapsed() >= Duration::from_millis(100) || partial.len() - saved >= 4096
                    {
                        if let Some(index) = index
                            && self
                                .store
                                .update(&run.session_id, |s| {
                                    if let Some(message) = s.messages.get_mut(index) {
                                        message.content = partial.clone();
                                    }
                                    Ok(())
                                })
                                .is_err()
                        {
                            cancel.cancel();
                        }
                        last = Instant::now();
                        saved = partial.len();
                    }
                }
            },
            cancel,
        )
    }
    /// The configured model, checked against its provider's catalog (once per
    /// turn, before the request is built, so tools match its capabilities).
    pub(crate) fn primary_model(
        &self,
        config: &SessionConfig,
    ) -> Result<(Arc<dyn ModelProvider>, ModelDescriptor), String> {
        let target = ModelRef::new(
            aidex_providers::ProviderId::new(&config.provider_id).map_err(|e| e.to_string())?,
            aidex_providers::ModelId::new(&config.model_id).map_err(|e| e.to_string())?,
        );
        self.validate_model(&target, config)
            .map_err(|e| e.to_string())
    }
    /// `base` prepared for `model`; also whether it carries attachments
    /// (images, PDFs, or text) of messages before the run's own one.
    fn prepare(
        &self,
        base: &ChatRequest,
        model: &ModelDescriptor,
        turn: &Turn,
        drop_earlier: bool,
    ) -> (ChatRequest, bool) {
        let mut request = base.clone();
        let (earlier, wrapped_text) = self.apply_attachments(
            &mut request.messages,
            turn.start,
            model,
            &turn.attachments,
            &turn.nonce,
            drop_earlier,
        );
        let offered: HashSet<String> = request.tools.iter().map(|t| t.name.clone()).collect();
        let wrapped_results =
            flatten_tool_history(&mut request.messages, turn.start, &offered, &turn.nonce);
        drop_empty_assistants(&mut request.messages, turn.start);
        ensure_preamble(
            &mut request.messages,
            &turn.nonce,
            !turn.attachments.is_empty(),
            wrapped_text || wrapped_results,
        );
        (request, earlier)
    }
    pub(crate) fn infer(
        &self,
        request: &ChatRequest,
        run: &Run,
        config: &SessionConfig,
        cancel: &CancelToken,
        primary: (Arc<dyn ModelProvider>, ModelDescriptor),
        turn: &Turn,
    ) -> Result<ChatResponse, String> {
        let index = turn.index;
        // One model's attempt. When the provider refuses a request carrying
        // attachments of earlier messages, it is repeated once with those as
        // placeholders, so one unreadable or oversized old file cannot block
        // the chat. The run remembers that refusal (in memory, per model), so
        // its later steps do not send the refused request first again.
        let run_model = |provider: &dyn ModelProvider,
                         model: &ModelDescriptor,
                         base: &ChatRequest|
         -> Result<ChatResponse, ProviderError> {
            let page_tokens = self
                .registry
                .part_limits(model)
                .map_or(PDF_PAGE_TOKENS, |limits| limits.pdf_page_tokens);
            let refused = self.refused_earlier(&run.id, model);
            let (prepared, earlier) = self.prepare(base, model, turn, refused);
            let first = bound_request(&prepared, model, config, page_tokens).and_then(|bounded| {
                self.attempt(provider, &bounded, run, index, cancel, page_tokens)
            });
            match first {
                Err(error)
                    if !refused
                        && earlier
                        && refused_for_size_or_content(&error)
                        && !cancel.is_stopped() =>
                {
                    self.remember_refused_earlier(&run.id, model);
                    let (prepared, _) = self.prepare(base, model, turn, true);
                    let bounded = bound_request(&prepared, model, config, page_tokens)?;
                    self.attempt(provider, &bounded, run, index, cancel, page_tokens)
                }
                other => other,
            }
        };
        let target = ModelRef::new(
            aidex_providers::ProviderId::new(&config.provider_id).map_err(|e| e.to_string())?,
            request.model.clone(),
        );
        let (provider, model) = primary;
        self.broker.authorize_native(
            &run.session_id,
            config,
            aidex_policy::Action::ModelInvoke,
            Some(match provider.descriptor().locality {
                aidex_providers::Locality::Local => aidex_policy::ModelLocality::Local,
                aidex_providers::Locality::Remote => aidex_policy::ModelLocality::Remote,
            }),
        )?;
        match run_model(provider.as_ref(), &model, request) {
            Ok(response) => Ok(response),
            Err(error) => {
                if !error.is_retryable() || cancel.is_stopped() {
                    return Err(error.to_string());
                }
                let Some(fallback) = &config.fallback_model else {
                    return Err(error.to_string());
                };
                let target_fallback = ModelRef::parse(fallback).map_err(|e| e.to_string())?;
                let (fallback_provider, fallback_model) = self
                    .validate_model(&target_fallback, config)
                    .map_err(|e| e.to_string())?;
                self.broker.authorize_native(
                    &run.session_id,
                    config,
                    aidex_policy::Action::ModelInvoke,
                    Some(match fallback_provider.descriptor().locality {
                        aidex_providers::Locality::Local => aidex_policy::ModelLocality::Local,
                        aidex_providers::Locality::Remote => aidex_policy::ModelLocality::Remote,
                    }),
                )?;
                // Event references encode provider/model with portable punctuation;
                // full configured model references remain in encrypted session configuration.
                self.emit(
                    &run.session_id,
                    EventKind::ModelFallback {
                        from: model_event_id(&target)?,
                        to: model_event_id(&target_fallback)?,
                        reason: "retryable inference failure; one configured fallback attempt"
                            .into(),
                    },
                );
                let mut fallback_request = request.clone();
                fallback_request.model = target_fallback.model_id;
                // A fallback without native tool calling answers without tools
                // (Agent mode already refused such a fallback above).
                if fallback_model.capabilities.tool_calls != ToolCallSupport::Native {
                    fallback_request.tools.clear();
                    fallback_request.tool_choice = aidex_providers::ToolChoice::None;
                }
                run_model(
                    fallback_provider.as_ref(),
                    &fallback_model,
                    &fallback_request,
                )
                .map_err(|e| e.to_string())
            }
        }
    }
}

fn model_event_id(model: &ModelRef) -> Result<EntityId, String> {
    let text = model
        .to_string()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':') {
                c
            } else {
                '.'
            }
        })
        .take(128)
        .collect::<String>();
    EntityId::new(text).map_err(|e| e.to_string())
}

/// `request` with its output budget set. PDF pages are estimated at
/// `pdf_page_tokens` each, the model family's cost
/// ([`aidex_providers::PartLimits::pdf_page_tokens`]).
fn bound_request(
    request: &ChatRequest,
    model: &ModelDescriptor,
    configuration: &SessionConfig,
    pdf_page_tokens: u32,
) -> Result<ChatRequest, ProviderError> {
    let mut bounded = request.clone();
    let mut budget = configuration
        .max_tokens
        .min(model.max_output_tokens.unwrap_or(configuration.max_tokens));
    // Only a declared or reported window is a real ceiling. Catalog windows are
    // conservative lower bounds and Assumed ones are guesses, so neither may
    // reject or truncate a request the model would accept; the provider's own
    // context error covers a real overflow.
    let exact_window = matches!(
        model.source,
        CapabilitySource::Configured | CapabilitySource::Discovered
    );
    if exact_window && model.context_window > 0 {
        let input = request.estimated_input_tokens_with(pdf_page_tokens);
        let remaining = model.context_window.saturating_sub(input);
        if remaining == 0 {
            return Err(ProviderError::ContextOverflow {
                limit: Some(model.context_window),
                requested: Some(input.saturating_add(1)),
            });
        }
        budget = budget.min(remaining);
    }
    if budget == 0 {
        return Err(ProviderError::InvalidRequest(
            "model has no usable output budget".into(),
        ));
    }
    bounded.max_tokens = Some(budget);
    Ok(bounded)
}
