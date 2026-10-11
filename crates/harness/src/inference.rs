//! One selected-model attempt and at most one user-selected transient fallback.
use crate::NativeEngine;
use aidex_core::{CancelToken, EntityId};
use aidex_events::EventKind;
use aidex_native_sessions::{Mode, Run, SessionConfig};
use aidex_providers::{
    CapabilitySource, ChatRequest, ChatResponse, ModelDescriptor, ModelProvider, ModelRef,
    ProviderError, StreamEvent, ToolCallSupport,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
impl NativeEngine {
    fn reserve_inference_retry(&self, run: &Run) -> Result<(), String> {
        self.store.update(&run.session_id, |session| {
            let maximum = session.configuration.max_steps;
            let run = session
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            if run.steps >= maximum {
                return Err("native run step budget exhausted before inference retry".into());
            }
            if run.status == aidex_native_sessions::RunStatus::Cancelled {
                return Err("run cancelled".into());
            }
            run.steps += 1;
            run.inference_retries = run.inference_retries.saturating_add(1);
            Ok(())
        })
    }
    fn validate_model(
        &self,
        target: &ModelRef,
        config: &SessionConfig,
    ) -> Result<(Arc<dyn ModelProvider>, ModelDescriptor), ProviderError> {
        let provider = self
            .registry
            .provider(&target.provider_id)
            .ok_or_else(|| ProviderError::unavailable("configured model provider is absent"))?;
        let listing = self.registry.models();
        let model = listing
            .models
            .iter()
            .find(|m| m.model == *target)
            .ok_or_else(|| {
                ProviderError::InvalidRequest(
                    "configured model is not in the provider catalog".into(),
                )
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
                run.estimated_input_tokens = Some(request.estimated_input_tokens());
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
                        if let Ok(agent_id) = EntityId::new(&run.id) {
                            let _ = self.events.emit(
                                scope.clone(),
                                EventKind::AgentRunDelta {
                                    agent_id,
                                    text: text.clone(),
                                },
                            );
                        }
                        if run.parent_run_id.is_none() {
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
    pub(crate) fn infer(
        &self,
        request: &ChatRequest,
        run: &Run,
        config: &SessionConfig,
        index: Option<usize>,
        cancel: &CancelToken,
    ) -> Result<ChatResponse, String> {
        let target = ModelRef::new(
            aidex_providers::ProviderId::new(&config.provider_id).map_err(|e| e.to_string())?,
            request.model.clone(),
        );
        let (provider, model) = self
            .validate_model(&target, config)
            .map_err(|e| e.to_string())?;
        self.broker.authorize_native(
            &run.session_id,
            config,
            aidex_policy::Action::ModelInvoke,
            Some(match provider.descriptor().locality {
                aidex_providers::Locality::Local => aidex_policy::ModelLocality::Local,
                aidex_providers::Locality::Remote => aidex_policy::ModelLocality::Remote,
            }),
        )?;
        let (bounded, trimmed) =
            bound_request(request, &model, config).map_err(|e| e.to_string())?;
        self.store.update(&run.session_id, |session| {
            let run = session
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            run.context_trimmed_messages = run.context_trimmed_messages.max(trimmed);
            Ok(())
        })?;
        let mut retries = 0;
        let result = loop {
            match self.attempt(provider.as_ref(), &bounded, run, index, cancel) {
                Err(error)
                    if error.is_retryable()
                        && !cancel.is_stopped()
                        && config.fallback_model.is_none()
                        && retries < 2 =>
                {
                    let delay = match &error {
                        ProviderError::RateLimited {
                            retry_after: Some(delay),
                        } if *delay > Duration::from_secs(2) => break Err(error),
                        ProviderError::RateLimited {
                            retry_after: Some(delay),
                        } => (*delay).max(Duration::from_millis(100 * u64::from(retries + 1))),
                        _ => Duration::from_millis(100 * u64::from(retries + 1)),
                    };
                    retries += 1;
                    self.reserve_inference_retry(run)?;
                    self.emit(
                        &run.session_id,
                        EventKind::AgentRetry {
                            attempt: retries,
                            reason: "retryable inference failure; no tool effect retried".into(),
                        },
                    );
                    let deadline = Instant::now() + delay;
                    while Instant::now() < deadline && !cancel.is_stopped() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                result => break result,
            }
        };
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
                self.reserve_inference_retry(run)?;
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
                let (fallback_request, trimmed) =
                    bound_request(&fallback_request, &fallback_model, config)
                        .map_err(|e| e.to_string())?;
                self.store.update(&run.session_id, |session| {
                    let run = session
                        .runs
                        .iter_mut()
                        .find(|r| r.id == run.id)
                        .ok_or("run not found")?;
                    run.context_trimmed_messages = run.context_trimmed_messages.max(trimmed);
                    Ok(())
                })?;
                self.attempt(
                    fallback_provider.as_ref(),
                    &fallback_request,
                    run,
                    index,
                    cancel,
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

fn bound_request(
    request: &ChatRequest,
    model: &ModelDescriptor,
    configuration: &SessionConfig,
) -> Result<(ChatRequest, u32), ProviderError> {
    let mut bounded = request.clone();
    let mut budget = configuration
        .max_tokens
        .min(model.max_output_tokens.unwrap_or(configuration.max_tokens));
    let mut trimmed = 0;
    if model.source != CapabilitySource::Assumed && model.context_window > 0 {
        let reserve = budget.min((model.context_window / 4).max(1));
        trimmed = u32::try_from(crate::context::trim_replay(
            &mut bounded,
            model.context_window.saturating_sub(reserve),
        ))
        .unwrap_or(u32::MAX);
        let input = bounded.estimated_input_tokens();
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
