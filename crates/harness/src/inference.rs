//! One selected-model attempt, at most two retries of a transient failure
//! without a fallback, and at most one user-selected transient fallback.
//!
//! Each attempt gets its own copy of the turn's request, prepared for the
//! model it goes to: attachments as parts, wrapped text, or placeholders
//! ([`crate::attachments`]), historical tool calls the request does not offer
//! flattened to text, empty assistant placeholders dropped
//! ([`crate::history`]), and, for a declared or reported context window, a
//! replay view that fits it ([`crate::replay`]).
use crate::{
    NativeEngine,
    attachments::TurnAttachments,
    history::{drop_empty_assistants, flatten_tool_history},
    replay::{Pinned, trim_replay},
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
    cell::Cell,
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

/// Most retries of a transient failure of one turn (without a fallback).
const MAX_INFERENCE_RETRIES: u32 = 2;
/// Wait before the first retry; the second waits twice as long.
const RETRY_BACKOFF: Duration = Duration::from_millis(200);
/// Longest provider-requested wait (`Retry-After`) a retry honours; a longer
/// one fails the turn.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(2);

/// What every attempt of one turn shares.
pub(crate) struct Turn {
    /// Index of the run transcript in the request's messages.
    pub start: usize,
    /// The run's delimiter nonce.
    pub nonce: String,
    /// History index of the assistant placeholder the reply streams into.
    pub index: Option<usize>,
    pub attachments: TurnAttachments,
    /// Whether an attempt streamed text: a retry would show it twice, so
    /// none follows.
    pub streamed: Cell<bool>,
}

/// How long to wait before retry number `retries + 1` after `error`; `None`
/// when it is not retried (not transient, retries used up, or the provider
/// asked to wait longer than [`MAX_RETRY_AFTER`]).
fn retry_delay(error: &ProviderError, retries: u32) -> Option<Duration> {
    if !error.is_retryable() || retries >= MAX_INFERENCE_RETRIES {
        return None;
    }
    let backoff = RETRY_BACKOFF * (retries + 1);
    match error {
        ProviderError::RateLimited {
            retry_after: Some(after),
        } => (*after <= MAX_RETRY_AFTER).then(|| (*after).max(backoff)),
        _ => Some(backoff),
    }
}

/// Wait `delay` unless `cancel` stops first; whether the wait completed.
fn pause(cancel: &CancelToken, delay: Duration) -> bool {
    let until = Instant::now() + delay;
    while !cancel.is_stopped() {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(Duration::from_millis(20)));
    }
    false
}

fn store_unavailable() -> ProviderError {
    ProviderError::InvalidRequest("private session store unavailable".into())
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
    /// Send one request of `turn`. Text deltas go to the run's history
    /// placeholder, to live `agent.run_delta` events (every run), and to
    /// `agent.delta` (root runs), and mark the turn as streamed.
    fn attempt(
        &self,
        provider: &dyn ModelProvider,
        request: &ChatRequest,
        run: &Run,
        turn: &Turn,
        cancel: &CancelToken,
        pdf_page_tokens: u32,
    ) -> Result<ChatResponse, ProviderError> {
        let index = turn.index;
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
            .map_err(|_| store_unavailable())?;
        if let Some(index) = index {
            self.store
                .update(&run.session_id, |s| {
                    s.messages[index].content.clear();
                    Ok(())
                })
                .map_err(|_| store_unavailable())?;
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
        let agent_id = EntityId::new(&run.id).ok();
        let root = run.parent_run_id.is_none();
        let mut partial = String::new();
        let mut last = Instant::now() - Duration::from_secs(1);
        let mut saved = 0;
        provider.chat(
            request,
            &mut |event| {
                if let StreamEvent::TextDelta { text } = event {
                    if !text.is_empty() {
                        turn.streamed.set(true);
                    }
                    partial.push_str(text);
                    if let Some(scope) = &scope {
                        if let Some(agent_id) = &agent_id {
                            let _ = self.events.emit(
                                scope.clone(),
                                EventKind::AgentRunDelta {
                                    agent_id: agent_id.clone(),
                                    text: text.clone(),
                                },
                            );
                        }
                        if root {
                            let _ = self
                                .events
                                .emit(scope.clone(), EventKind::AgentDelta { text: text.clone() });
                        }
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
    /// (images, PDFs, or text) of messages before the run's own one, and
    /// which messages its replay view must keep: the instructions and
    /// context before the transcript, the run's own user message, and user
    /// messages with attachments.
    fn prepare(
        &self,
        base: &ChatRequest,
        model: &ModelDescriptor,
        turn: &Turn,
        drop_earlier: bool,
    ) -> (ChatRequest, bool, Pinned) {
        let mut request = base.clone();
        // Over the transcript; a child's own message is its first.
        let own = turn.attachments.own.unwrap_or(0);
        let mut pinned = vec![false; request.messages.len().saturating_sub(turn.start)];
        for index in std::iter::once(own).chain(turn.attachments.by_position.keys().copied()) {
            if let Some(pin) = pinned.get_mut(index) {
                *pin = true;
            }
        }
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
        let kept = drop_empty_assistants(&mut request.messages, turn.start);
        let transcript = kept.get(turn.start..).unwrap_or_default();
        let current = transcript.iter().take(own).filter(|kept| **kept).count();
        let pinned: Vec<bool> = pinned
            .into_iter()
            .zip(transcript)
            .filter_map(|(pin, kept)| kept.then_some(pin))
            .collect();
        ensure_preamble(
            &mut request.messages,
            &turn.nonce,
            !turn.attachments.is_empty(),
            wrapped_text || wrapped_results,
        );
        // The preamble may be inserted before the transcript; everything
        // before it is kept.
        let prefix = request.messages.len().saturating_sub(pinned.len());
        let mut keep = vec![true; prefix];
        keep.extend(pinned);
        let pinned = Pinned {
            keep,
            current: prefix + current,
        };
        (request, earlier, pinned)
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
            let send = |drop_earlier: bool| {
                let (prepared, earlier, pinned) = self.prepare(base, model, turn, drop_earlier);
                let result = bound_request(&prepared, &pinned, model, config, page_tokens)
                    .and_then(|(bounded, trimmed)| {
                        self.store
                            .record_context_trim(&run.id, trimmed)
                            .map_err(|_| store_unavailable())?;
                        self.attempt(provider, &bounded, run, turn, cancel, page_tokens)
                    });
                (earlier, result)
            };
            let refused = self.refused_earlier(&run.id, model);
            match send(refused) {
                (true, Err(error))
                    if !refused && refused_for_size_or_content(&error) && !cancel.is_stopped() =>
                {
                    self.remember_refused_earlier(&run.id, model);
                    send(true).1
                }
                (_, result) => result,
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
        let mut result = run_model(provider.as_ref(), &model, request);
        // Without a fallback, a transient failure is retried up to twice
        // when nothing was streamed. Each retry uses a step of the run's
        // budget and is announced; no tool effect is repeated (tool calls run
        // only after a complete response).
        if config.fallback_model.is_none() {
            let mut retries = 0;
            while let Err(error) = &result {
                let Some(delay) = retry_delay(error, retries) else {
                    break;
                };
                if turn.streamed.get()
                    || cancel.is_stopped()
                    || self.store.reserve_inference_retry(&run.id).is_err()
                {
                    break;
                }
                retries += 1;
                self.emit(
                    &run.session_id,
                    EventKind::AgentRetry {
                        attempt: retries,
                        reason: "transient inference failure; nothing was streamed".into(),
                    },
                );
                if !pause(cancel, delay) {
                    break;
                }
                result = run_model(provider.as_ref(), &model, request);
            }
        }
        match result {
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

/// `request` with its output budget set, and how many of its messages the
/// replay view left out or clipped. PDF pages are estimated at
/// `pdf_page_tokens` each, the model family's cost
/// ([`aidex_providers::PartLimits::pdf_page_tokens`]).
fn bound_request(
    request: &ChatRequest,
    pinned: &Pinned,
    model: &ModelDescriptor,
    configuration: &SessionConfig,
    pdf_page_tokens: u32,
) -> Result<(ChatRequest, u32), ProviderError> {
    let mut bounded = request.clone();
    let mut budget = configuration
        .max_tokens
        .min(model.max_output_tokens.unwrap_or(configuration.max_tokens));
    let mut trimmed = 0;
    // Only a declared or reported window is a real ceiling. Catalog windows are
    // conservative lower bounds and Assumed ones are guesses, so neither may
    // reject, trim, or truncate a request the model would accept; the
    // provider's own context error covers a real overflow.
    let exact_window = matches!(
        model.source,
        CapabilitySource::Configured | CapabilitySource::Discovered
    );
    if exact_window && model.context_window > 0 {
        // Leave room for the reply: the output budget, up to a quarter of
        // the window.
        let reserve = budget.min(model.context_window / 4).max(1);
        trimmed = trim_replay(
            &mut bounded,
            pinned,
            model.context_window.saturating_sub(reserve),
            pdf_page_tokens,
        );
        let input = bounded.estimated_input_tokens_with(pdf_page_tokens);
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
    Ok((bounded, trimmed))
}
