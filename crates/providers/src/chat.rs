//! Provider-neutral chat requests, streamed events, and responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ContentPart, MAX_PARTS_PER_MESSAGE, ModelId, PDF_PAGE_TOKENS, ProviderError};

/// Most messages accepted in one request.
pub const MAX_MESSAGES: usize = 4096;
/// Most tools offered in one request.
pub const MAX_TOOLS: usize = 256;
/// Most stop sequences in one request.
pub const MAX_STOP_SEQUENCES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::Tool => "tool",
        }
    }
}

/// A completed tool call requested by the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments exactly as the model produced them (normally a JSON object).
    /// They are untrusted: parse and validate before use.
    pub arguments: String,
    /// Opaque provider state required to continue the originating tool turn.
    /// Persist this with the assistant message; do not interpret or alter it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// The call answered by a `tool` message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Images and PDFs the model reads with this (user) message, for one
    /// request only. Never serialized or deserialized, so persisted
    /// transcripts and older daemons never see them; the harness resolves
    /// them per request from the attachment store.
    #[serde(skip)]
    pub parts: Vec<ContentPart>,
}

impl ChatMessage {
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            parts: Vec::new(),
        }
    }

    /// This message with `parts` (only user messages may carry them).
    #[must_use]
    pub fn with_parts(mut self, parts: Vec<ContentPart>) -> Self {
        self.parts = parts;
        self
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }

    /// The result of tool call `call_id`.
    pub fn tool_result(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            tool_call_id: Some(call_id.into()),
            ..Self::new(Role::Tool, content)
        }
    }
}

/// A tool offered to the model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    /// 1-64 ASCII letters, digits, `_`, or `-`.
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments object.
    pub parameters: Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolChoice {
    /// The model decides.
    #[default]
    Auto,
    /// The model must not call tools.
    None,
    /// The model must call at least one tool.
    Required,
    /// The model must call this tool.
    Tool { name: String },
}

/// Constrained output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResponseFormat {
    /// The response must be JSON matching `schema`.
    JsonSchema {
        name: String,
        schema: Value,
        /// Ask for strict adherence (OpenAI `strict`); servers that constrain
        /// decoding with a grammar always adhere.
        #[serde(default = "default_true")]
        strict: bool,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatRequest {
    pub model: ModelId,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub tool_choice: ToolChoice,
    #[serde(default)]
    pub response_format: Option<ResponseFormat>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stop: Vec<String>,
}

impl ChatRequest {
    pub fn new(model: ModelId, messages: Vec<ChatMessage>) -> Self {
        Self {
            model,
            messages,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
            response_format: None,
            temperature: None,
            max_tokens: None,
            stop: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub fn with_response_format(mut self, format: ResponseFormat) -> Self {
        self.response_format = Some(format);
        self
    }

    #[must_use]
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Reject requests no provider should receive. Every provider calls this
    /// before doing any work; messages describe the rule, never the content.
    pub fn validate(&self) -> Result<(), ProviderError> {
        let invalid = |message: &str| Err(ProviderError::InvalidRequest(message.to_owned()));
        if self.messages.is_empty() {
            return invalid("a chat request needs at least one message");
        }
        if self.messages.len() > MAX_MESSAGES {
            return invalid("a chat request has too many messages");
        }
        if self.tools.len() > MAX_TOOLS {
            return invalid("a chat request offers too many tools");
        }
        if self.stop.len() > MAX_STOP_SEQUENCES || self.stop.iter().any(String::is_empty) {
            return invalid("stop sequences must be non-empty and at most 16");
        }
        if self
            .temperature
            .is_some_and(|t| !t.is_finite() || !(0.0..=2.0).contains(&t))
        {
            return invalid("temperature must be between 0 and 2");
        }
        if self.max_tokens == Some(0) {
            return invalid("max_tokens must be positive");
        }
        let mut names = std::collections::HashSet::new();
        for tool in &self.tools {
            if !is_tool_name(&tool.name) {
                return invalid("tool names must be 1-64 ASCII letters, digits, `_`, or `-`");
            }
            if !names.insert(tool.name.as_str()) {
                return invalid("tool names must be unique");
            }
            if !tool.parameters.is_object() {
                return invalid("tool parameters must be a JSON Schema object");
            }
        }
        match &self.tool_choice {
            ToolChoice::Required if self.tools.is_empty() => {
                return invalid("tool_choice `required` needs at least one tool");
            }
            ToolChoice::Tool { name } if !names.contains(name.as_str()) => {
                return invalid("tool_choice names a tool that is not offered");
            }
            _ => {}
        }
        if let Some(ResponseFormat::JsonSchema { name, schema, .. }) = &self.response_format {
            if !is_tool_name(name) {
                return invalid(
                    "response format names must be 1-64 ASCII letters, digits, `_`, or `-`",
                );
            }
            if !schema.is_object() {
                return invalid("response format schema must be a JSON Schema object");
            }
        }
        for message in &self.messages {
            match message.role {
                Role::Tool if message.tool_call_id.as_deref().is_none_or(str::is_empty) => {
                    return invalid("tool messages need the tool_call_id they answer");
                }
                Role::Assistant => {}
                _ if !message.tool_calls.is_empty() => {
                    return invalid("only assistant messages may carry tool calls");
                }
                _ => {}
            }
            if message
                .tool_calls
                .iter()
                .any(|call| call.id.is_empty() || !is_tool_name(&call.name))
            {
                return invalid("tool calls need an id and a valid tool name");
            }
            if !message.parts.is_empty() {
                if message.role != Role::User {
                    return invalid("only user messages may carry image or PDF parts");
                }
                if message.parts.len() > MAX_PARTS_PER_MESSAGE {
                    return invalid("a message may carry at most 20 image or PDF parts");
                }
                for part in &message.parts {
                    part.check().or_else(invalid)?;
                }
            }
        }
        Ok(())
    }

    /// A rough prompt-size estimate (about four bytes per token plus framing,
    /// and each image or PDF part by [`ContentPart::estimated_tokens`], never
    /// by its base64 size), for budgeting before a provider reports real
    /// usage.
    pub fn estimated_input_tokens(&self) -> u32 {
        self.estimated_input_tokens_with(PDF_PAGE_TOKENS)
    }

    /// [`Self::estimated_input_tokens`] with `pdf_page_tokens` per PDF page,
    /// the model's [`crate::PartLimits::pdf_page_tokens`].
    pub fn estimated_input_tokens_with(&self, pdf_page_tokens: u32) -> u32 {
        let bytes: usize = self
            .messages
            .iter()
            .map(|message| {
                16 + message.content.len()
                    + message
                        .tool_calls
                        .iter()
                        .map(|call| call.name.len() + call.arguments.len() + 16)
                        .sum::<usize>()
            })
            .chain(self.tools.iter().map(|tool| {
                tool.name.len() + tool.description.len() + tool.parameters.to_string().len()
            }))
            .sum();
        let parts: u64 = self
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .map(|part| u64::from(part.estimated_tokens_with(pdf_page_tokens)))
            .sum();
        u32::try_from((bytes.div_ceil(4) as u64).saturating_add(parts)).unwrap_or(u32::MAX)
    }
}

/// Whether `name` is an acceptable tool or schema name.
pub fn is_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Token usage reported by the provider; `None` when it reported nothing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Cancelled,
}

impl FinishReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Length => "length",
            Self::ToolCalls => "tool_calls",
            Self::ContentFilter => "content_filter",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Incremental output delivered while a chat streams.
///
/// Every successful chat ends with exactly one `Done`. A cancelled chat emits
/// `Done(Cancelled)` and then returns [`ProviderError::Cancelled`]. Deltas from
/// an attempt that later fails are not retracted; a caller that falls back to
/// another model discards what it buffered for the failed attempt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta {
        text: String,
    },
    /// A fragment of tool call `index`; `id` and `name` arrive at most once,
    /// `arguments` fragments concatenate in order.
    ToolCallDelta {
        index: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        arguments: String,
    },
    Usage(Usage),
    Done {
        finish_reason: FinishReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatResponse {
    /// The assistant message (content and any tool calls).
    pub message: ChatMessage,
    pub finish_reason: FinishReason,
    pub usage: Usage,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> ChatRequest {
        ChatRequest::new(ModelId::new("m").unwrap(), vec![ChatMessage::user("hello")])
    }

    fn tool(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: "d".into(),
            parameters: json!({"type": "object"}),
        }
    }

    #[test]
    fn valid_requests_pass_and_malformed_ones_are_invalid_requests() {
        assert!(request().validate().is_ok());
        let mut with_tools = request().with_tools(vec![tool("read_file")]);
        with_tools.tool_choice = ToolChoice::Tool {
            name: "read_file".into(),
        };
        assert!(with_tools.validate().is_ok());

        type Mutation = Box<dyn Fn(&mut ChatRequest)>;
        let cases: Vec<Mutation> = vec![
            Box::new(|r| r.messages.clear()),
            Box::new(|r| r.tools = vec![tool("bad name")]),
            Box::new(|r| r.tools = vec![tool("a"), tool("a")]),
            Box::new(|r| {
                r.tools = vec![ToolSpec {
                    parameters: json!("x"),
                    ..tool("a")
                }]
            }),
            Box::new(|r| r.tool_choice = ToolChoice::Required),
            Box::new(|r| {
                r.tool_choice = ToolChoice::Tool {
                    name: "missing".into(),
                }
            }),
            Box::new(|r| r.temperature = Some(f32::NAN)),
            Box::new(|r| r.temperature = Some(3.0)),
            Box::new(|r| r.max_tokens = Some(0)),
            Box::new(|r| r.stop = vec![String::new()]),
            Box::new(|r| r.messages.push(ChatMessage::new(Role::Tool, "result"))),
            Box::new(|r| {
                r.response_format = Some(ResponseFormat::JsonSchema {
                    name: "x".into(),
                    schema: json!(true),
                    strict: true,
                })
            }),
            Box::new(|r| {
                let mut message = ChatMessage::user("u");
                message.tool_calls.push(ToolCall {
                    provider_metadata: None,
                    id: "1".into(),
                    name: "a".into(),
                    arguments: "{}".into(),
                });
                r.messages.push(message);
            }),
        ];
        for (index, mutate) in cases.iter().enumerate() {
            let mut request = request();
            mutate(&mut request);
            assert!(
                matches!(request.validate(), Err(ProviderError::InvalidRequest(_))),
                "case {index}"
            );
        }
    }

    #[test]
    fn parts_are_request_only_and_belong_to_user_messages() {
        use crate::parts::fixtures::{pdf, png};
        let image = || ContentPart::image("image/png", png(28, 28));
        let message = ChatMessage::user("see").with_parts(vec![image()]);
        // Parts are never serialized: sealed transcripts and older daemons
        // see exactly the text-only record.
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value, json!({"role": "user", "content": "see"}));
        let back: ChatMessage = serde_json::from_value(value).unwrap();
        assert!(back.parts.is_empty());
        assert!(
            serde_json::from_value::<ChatMessage>(
                json!({"role": "user", "content": "x", "parts": []})
            )
            .is_err()
        );
        let model = || ModelId::new("m").unwrap();
        assert!(
            ChatRequest::new(model(), vec![message.clone()])
                .validate()
                .is_ok()
        );
        let blank = ChatMessage::user("").with_parts(vec![image()]);
        assert!(ChatRequest::new(model(), vec![blank]).validate().is_ok());

        type Mutation = Box<dyn Fn(&mut ChatMessage)>;
        let cases: Vec<Mutation> = vec![
            Box::new(|m| m.role = Role::Assistant),
            Box::new(|m| m.role = Role::System),
            Box::new(|m| {
                m.role = Role::Tool;
                m.tool_call_id = Some("c".into());
            }),
            Box::new(move |m| m.parts = vec![image(); MAX_PARTS_PER_MESSAGE + 1]),
            Box::new(|m| m.parts = vec![ContentPart::image("image/jpeg", png(1, 1))]),
            Box::new(|m| m.parts = vec![ContentPart::image("image/bmp", b"BM".to_vec())]),
            Box::new(|m| m.parts = vec![ContentPart::pdf("a.pdf", pdf(), 0)]),
            Box::new(|m| m.parts = vec![ContentPart::pdf("a.pdf", png(1, 1), 1)]),
        ];
        for (index, mutate) in cases.iter().enumerate() {
            let mut message = message.clone();
            mutate(&mut message);
            let request = ChatRequest::new(model(), vec![message]);
            assert!(
                matches!(request.validate(), Err(ProviderError::InvalidRequest(_))),
                "case {index}"
            );
        }
        let full = ChatMessage::user("x").with_parts(vec![image(); MAX_PARTS_PER_MESSAGE]);
        assert!(ChatRequest::new(model(), vec![full]).validate().is_ok());
    }

    #[test]
    fn estimates_count_parts_by_size_and_pages_not_bytes() {
        use crate::parts::fixtures::{pdf, png};
        let model = || ModelId::new("m").unwrap();
        let mut bytes = png(28, 28);
        bytes.resize(4 * 1024 * 1024, 0);
        let image = ChatRequest::new(
            model(),
            vec![ChatMessage::user("").with_parts(vec![ContentPart::image("image/png", bytes)])],
        );
        // 16 bytes of framing (4 tokens) plus one 28x28 tile.
        assert_eq!(image.estimated_input_tokens(), 5);
        let document = ChatRequest::new(
            model(),
            vec![ChatMessage::user("").with_parts(vec![ContentPart::pdf("a.pdf", pdf(), 10)])],
        );
        assert_eq!(document.estimated_input_tokens(), 4 + 10 * PDF_PAGE_TOKENS);
        // A family's own page cost (Gemini's 560); text and images are
        // estimated the same way.
        assert_eq!(document.estimated_input_tokens_with(560), 4 + 10 * 560);
        assert_eq!(image.estimated_input_tokens_with(560), 5);
        assert_eq!(
            request().estimated_input_tokens(),
            ChatRequest::new(model(), vec![ChatMessage::user("hello")]).estimated_input_tokens()
        );
    }

    #[test]
    fn stream_events_and_requests_serialize_with_stable_tags() {
        let event = StreamEvent::ToolCallDelta {
            index: 0,
            id: Some("call_1".into()),
            name: None,
            arguments: "{\"a\"".into(),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"type": "tool_call_delta", "index": 0, "id": "call_1", "arguments": "{\"a\""})
        );
        let done = StreamEvent::Done {
            finish_reason: FinishReason::ToolCalls,
        };
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            json!({"type": "done", "finish_reason": "tool_calls"})
        );
        let round = serde_json::from_value::<ChatRequest>(serde_json::to_value(request()).unwrap())
            .unwrap();
        assert_eq!(round, request());
        assert!(request().estimated_input_tokens() > 0);
    }
}
