//! A deterministic, scripted provider for tests and offline demos. No I/O.

use std::{
    collections::VecDeque,
    fmt,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};

use aidex_core::{CancelReason, CancelToken};

use crate::{
    AuthConfig, ChatMessage, ChatRequest, ChatResponse, FinishReason, Locality, ModelDescriptor,
    ModelProvider, ModelRef, ProviderDescriptor, ProviderError, ProviderHealth, ProviderId,
    ProviderKind, StreamEvent, ToolCall, Usage,
};

/// What one scripted turn produces.
#[derive(Clone, Debug, PartialEq)]
pub enum FakeOutcome {
    /// Stream `text` and stop.
    Text(String),
    /// Stream optional text, then these tool calls, and finish with `tool_calls`.
    ToolCalls { text: String, calls: Vec<ToolCall> },
    /// Fail with this error (no events are emitted).
    Error(ProviderError),
}

/// One scripted turn: an optional delay (honouring cancellation), then an outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeTurn {
    pub delay: Option<Duration>,
    pub outcome: FakeOutcome,
    /// Text is streamed in deltas of at most this many characters.
    pub chunk_chars: usize,
}

impl FakeTurn {
    pub fn text(text: impl Into<String>) -> Self {
        Self::from(FakeOutcome::Text(text.into()))
    }

    /// One tool call with id `call_<n>` assigned at playback.
    pub fn tool_call(name: impl Into<String>, arguments: impl Into<String>) -> Self {
        Self::from(FakeOutcome::ToolCalls {
            text: String::new(),
            calls: vec![ToolCall {
                provider_metadata: None,
                id: String::new(),
                name: name.into(),
                arguments: arguments.into(),
            }],
        })
    }

    pub fn tool_calls(calls: Vec<ToolCall>) -> Self {
        Self::from(FakeOutcome::ToolCalls {
            text: String::new(),
            calls,
        })
    }

    pub fn error(error: ProviderError) -> Self {
        Self::from(FakeOutcome::Error(error))
    }

    #[must_use]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    #[must_use]
    pub fn chunked(mut self, chars: usize) -> Self {
        self.chunk_chars = chars.max(1);
        self
    }
}

impl From<FakeOutcome> for FakeTurn {
    fn from(outcome: FakeOutcome) -> Self {
        Self {
            delay: None,
            outcome,
            chunk_chars: 8,
        }
    }
}

/// An ordered list of turns; each `chat` call consumes the next one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FakeScript {
    turns: Vec<FakeTurn>,
}

impl FakeScript {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn then(mut self, turn: FakeTurn) -> Self {
        self.turns.push(turn);
        self
    }
}

impl FromIterator<FakeTurn> for FakeScript {
    fn from_iter<I: IntoIterator<Item = FakeTurn>>(turns: I) -> Self {
        Self {
            turns: turns.into_iter().collect(),
        }
    }
}

struct State {
    script: VecDeque<FakeTurn>,
    requests: Vec<ChatRequest>,
    models_error: Option<ProviderError>,
    health_error: Option<ProviderError>,
    calls: u32,
}

/// A scripted [`ModelProvider`].
///
/// Requests are validated, checked against the model list, recorded for
/// assertions (image and PDF parts included), and then checked against the
/// model's context window and input capabilities: a part the model cannot
/// read ([`crate::accepts_part`]) fails as an invalid request, as a vendor
/// would. An exhausted script fails with a protocol error (never a
/// retryable one, so a test bug is not hidden by fallback).
pub struct FakeProvider {
    descriptor: ProviderDescriptor,
    models: Vec<ModelDescriptor>,
    state: Mutex<State>,
}

impl FakeProvider {
    pub fn new(id: ProviderId, locality: Locality) -> Self {
        Self {
            descriptor: ProviderDescriptor {
                id,
                kind: ProviderKind::Fake,
                base_url: None,
                locality,
                auth: AuthConfig::None,
            },
            models: Vec::new(),
            state: Mutex::new(State {
                script: VecDeque::new(),
                requests: Vec::new(),
                models_error: None,
                health_error: None,
                calls: 0,
            }),
        }
    }

    /// Report `kind` in the descriptor, so input-capability decisions
    /// ([`crate::accepts_part`] and friends) follow that protocol family's
    /// rules. Requests are still answered by the script.
    #[must_use]
    pub fn with_kind(mut self, kind: ProviderKind) -> Self {
        self.descriptor.kind = kind;
        self
    }

    /// Add a model; its reference and locality are rewritten to this provider.
    #[must_use]
    pub fn with_model(mut self, mut model: ModelDescriptor) -> Self {
        model.model = ModelRef::new(self.descriptor.id.clone(), model.model.model_id);
        model.locality = self.descriptor.locality;
        self.models.push(model);
        self
    }

    #[must_use]
    pub fn with_script(self, script: FakeScript) -> Self {
        self.lock().script.extend(script.turns);
        self
    }

    pub fn push_turn(&self, turn: FakeTurn) {
        self.lock().script.push_back(turn);
    }

    /// Every request received, in order.
    pub fn requests(&self) -> Vec<ChatRequest> {
        self.lock().requests.clone()
    }

    pub fn remaining_turns(&self) -> usize {
        self.lock().script.len()
    }

    /// Make `models()` fail (or succeed again with `None`).
    pub fn set_models_error(&self, error: Option<ProviderError>) {
        self.lock().models_error = error;
    }

    /// Make `health()` report this failure (or ready with `None`).
    pub fn set_health_error(&self, error: Option<ProviderError>) {
        self.lock().health_error = error;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn wait(delay: Duration, cancel: &CancelToken) -> Result<(), ProviderError> {
        let until = Instant::now() + delay;
        loop {
            stopped(cancel)?;
            let now = Instant::now();
            if now >= until {
                return Ok(());
            }
            thread::sleep((until - now).min(Duration::from_millis(5)));
        }
    }
}

fn stopped(cancel: &CancelToken) -> Result<(), ProviderError> {
    match cancel.state() {
        None => Ok(()),
        Some(CancelReason::Cancelled) => Err(ProviderError::Cancelled),
        Some(CancelReason::DeadlineExceeded) => Err(ProviderError::Timeout),
    }
}

impl fmt::Debug for FakeProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeProvider")
            .field("id", &self.descriptor.id)
            .field("models", &self.models.len())
            .finish_non_exhaustive()
    }
}

impl ModelProvider for FakeProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn models(&self) -> Result<Vec<ModelDescriptor>, ProviderError> {
        match &self.lock().models_error {
            Some(error) => Err(error.clone()),
            None => Ok(self.models.clone()),
        }
    }

    fn chat(
        &self,
        request: &ChatRequest,
        on_event: &mut dyn FnMut(&StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResponse, ProviderError> {
        request.validate()?;
        let model = self
            .models
            .iter()
            .find(|model| model.model.model_id == request.model)
            .ok_or_else(|| ProviderError::ModelNotFound {
                model: request.model.to_string(),
            })?;
        // PDF pages cost what the account's family charges (Gemini's are
        // cheaper), as the harness estimates them.
        let requested = request.estimated_input_tokens_with(
            crate::part_limits(&self.descriptor, None, model).pdf_page_tokens,
        );
        let (turn, call_number) = {
            let mut state = self.lock();
            state.requests.push(request.clone());
            state.calls += 1;
            if requested > model.context_window {
                return Err(ProviderError::ContextOverflow {
                    limit: Some(model.context_window),
                    requested: Some(requested),
                });
            }
            if request
                .messages
                .iter()
                .flat_map(|message| &message.parts)
                .any(|part| !crate::accepts_part(&self.descriptor, None, model, part))
            {
                return Err(ProviderError::InvalidRequest(
                    "the model cannot read an attached image or PDF".into(),
                ));
            }
            (state.script.pop_front(), state.calls)
        };
        let turn = turn.ok_or_else(|| ProviderError::Protocol("fake script exhausted".into()))?;
        let result = (|| {
            if let Some(delay) = turn.delay {
                Self::wait(delay, cancel)?;
            }
            stopped(cancel)?;
            let (text, mut calls) = match turn.outcome {
                FakeOutcome::Error(error) => return Err(error),
                FakeOutcome::Text(text) => (text, Vec::new()),
                FakeOutcome::ToolCalls { text, calls } => (text, calls),
            };
            let chars: Vec<char> = text.chars().collect();
            for chunk in chars.chunks(turn.chunk_chars) {
                stopped(cancel)?;
                on_event(&StreamEvent::TextDelta {
                    text: chunk.iter().collect(),
                });
            }
            for (index, call) in calls.iter_mut().enumerate() {
                if call.id.is_empty() {
                    call.id = format!("call_{call_number}_{index}");
                }
                on_event(&StreamEvent::ToolCallDelta {
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                    id: Some(call.id.clone()),
                    name: Some(call.name.clone()),
                    arguments: call.arguments.clone(),
                });
            }
            let finish_reason = if calls.is_empty() {
                FinishReason::Stop
            } else {
                FinishReason::ToolCalls
            };
            let usage = Usage {
                prompt_tokens: Some(requested),
                completion_tokens: Some(u32::try_from(chars.len().div_ceil(4)).unwrap_or(u32::MAX)),
            };
            on_event(&StreamEvent::Usage(usage));
            on_event(&StreamEvent::Done { finish_reason });
            let mut message = ChatMessage::assistant(text);
            message.tool_calls = calls;
            Ok(ChatResponse {
                message,
                finish_reason,
                usage,
            })
        })();
        if matches!(result, Err(ProviderError::Cancelled)) {
            on_event(&StreamEvent::Done {
                finish_reason: FinishReason::Cancelled,
            });
        }
        result
    }

    fn health(&self) -> ProviderHealth {
        match &self.lock().health_error {
            Some(error) => ProviderHealth::from_error(error),
            None => ProviderHealth::ready(Some(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentPart, ModelCapabilities, ModelId, ToolCallSupport};

    fn provider(script: FakeScript) -> FakeProvider {
        let id = ProviderId::new("fake").unwrap();
        FakeProvider::new(id.clone(), Locality::Local)
            .with_model(
                ModelDescriptor::new(
                    ModelRef::new(id, ModelId::new("tiny").unwrap()),
                    Locality::Local,
                    64,
                )
                .with_tool_calls(ToolCallSupport::Native),
            )
            .with_script(script)
    }

    fn request(text: &str) -> ChatRequest {
        ChatRequest::new(ModelId::new("tiny").unwrap(), vec![ChatMessage::user(text)])
    }

    #[test]
    fn scripted_turns_stream_deterministically_and_record_requests() {
        let fake = provider(
            FakeScript::new()
                .then(FakeTurn::text("hello world").chunked(5))
                .then(FakeTurn::tool_call("read_file", r#"{"path":"a"}"#)),
        );
        let mut events = Vec::new();
        let response = fake
            .chat(
                &request("hi"),
                &mut |event| events.push(event.clone()),
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(response.message.content, "hello world");
        assert_eq!(response.finish_reason, FinishReason::Stop);
        let deltas: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                StreamEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, ["hello", " worl", "d"]);
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));

        let response = fake
            .chat(&request("again"), &mut |_| {}, &CancelToken::new())
            .unwrap();
        assert_eq!(response.finish_reason, FinishReason::ToolCalls);
        assert_eq!(response.message.tool_calls[0].id, "call_2_0");
        assert_eq!(fake.requests().len(), 2);
        assert!(matches!(
            fake.chat(&request("more"), &mut |_| {}, &CancelToken::new()),
            Err(ProviderError::Protocol(_))
        ));
    }

    #[test]
    fn delays_honour_cancellation_and_context_limits_are_enforced() {
        let fake = provider(
            FakeScript::new().then(FakeTurn::text("late").with_delay(Duration::from_secs(30))),
        );
        let cancel = CancelToken::new();
        let canceller = cancel.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            canceller.cancel();
        });
        let started = Instant::now();
        let mut done = None;
        let result = fake.chat(
            &request("hi"),
            &mut |event| {
                if let StreamEvent::Done { finish_reason } = event {
                    done = Some(*finish_reason);
                }
            },
            &cancel,
        );
        handle.join().unwrap();
        assert!(matches!(result, Err(ProviderError::Cancelled)));
        assert_eq!(done, Some(FinishReason::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));

        let fake = provider(FakeScript::new().then(FakeTurn::text("x")));
        let huge = request(&"word ".repeat(200));
        assert!(matches!(
            fake.chat(&huge, &mut |_| {}, &CancelToken::new()),
            Err(ProviderError::ContextOverflow {
                limit: Some(64),
                ..
            })
        ));
        let unknown =
            ChatRequest::new(ModelId::new("other").unwrap(), vec![ChatMessage::user("x")]);
        assert!(matches!(
            fake.chat(&unknown, &mut |_| {}, &CancelToken::new()),
            Err(ProviderError::ModelNotFound { .. })
        ));
    }

    #[test]
    fn parts_are_recorded_and_checked_against_the_model() {
        use crate::parts::fixtures::{gif, pdf, png};
        let id = ProviderId::new("fake").unwrap();
        let model = |name: &str, vision: bool| {
            ModelDescriptor::new(
                ModelRef::new(id.clone(), ModelId::new(name).unwrap()),
                Locality::Local,
                100_000,
            )
            .with_capabilities(ModelCapabilities {
                vision,
                ..ModelCapabilities::default()
            })
        };
        let fake = FakeProvider::new(id.clone(), Locality::Local)
            .with_model(model("eyes", true))
            .with_model(model("blind", false))
            .with_script(FakeScript::new().then(FakeTurn::text("ok")));
        let parts = vec![
            ContentPart::image("image/png", png(2, 2)),
            ContentPart::pdf("a.pdf", pdf(), 1),
        ];
        let request = |name: &str, parts: Vec<ContentPart>| {
            ChatRequest::new(
                ModelId::new(name).unwrap(),
                vec![ChatMessage::user("see").with_parts(parts)],
            )
        };
        let cancel = CancelToken::new();
        fake.chat(&request("eyes", parts.clone()), &mut |_| {}, &cancel)
            .unwrap();
        assert_eq!(fake.requests()[0].messages[0].parts, parts);
        assert!(matches!(
            fake.chat(&request("blind", parts.clone()), &mut |_| {}, &cancel),
            Err(ProviderError::InvalidRequest(_))
        ));
        assert_eq!(fake.requests().len(), 2, "refused requests are recorded");

        let gemini = FakeProvider::new(id.clone(), Locality::Local)
            .with_kind(ProviderKind::Gemini)
            .with_model(model("eyes", true))
            .with_script(FakeScript::new().then(FakeTurn::text("ok")));
        assert_eq!(gemini.descriptor().kind, ProviderKind::Gemini);
        let still = vec![ContentPart::image("image/gif", gif(1))];
        assert!(matches!(
            gemini.chat(&request("eyes", still), &mut |_| {}, &cancel),
            Err(ProviderError::InvalidRequest(_))
        ));
        assert_eq!(gemini.remaining_turns(), 1, "a refusal consumes no turn");

        // 100 PDF pages overflow the 100,000-token window at 3,000 tokens a
        // page, but fit at Gemini's 560.
        let long = vec![ContentPart::pdf("a.pdf", pdf(), 100)];
        assert!(matches!(
            fake.chat(&request("eyes", long.clone()), &mut |_| {}, &cancel),
            Err(ProviderError::ContextOverflow { .. })
        ));
        gemini
            .chat(&request("eyes", long), &mut |_| {}, &cancel)
            .unwrap();
    }
}
