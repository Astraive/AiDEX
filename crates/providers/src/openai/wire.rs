//! OpenAI chat-completions wire format: request bodies, responses, and errors.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    ASSUMED_CONTEXT_WINDOW, CapabilitySource, ChatMessage, ChatRequest, ChatResponse, ContentPart,
    CostHint, FinishReason, Locality, ModelCapabilities, ModelConfig, ModelDescriptor, ModelId,
    ModelRef, ModelRole, ProviderError, ProviderId, ResponseFormat, Role, StreamEvent, ToolCall,
    ToolCallSupport, ToolChoice, Usage,
};

/// Most models accepted from one `/models` listing.
const MAX_LISTED_MODELS: usize = 1000;

/// The JSON body for `POST /chat/completions`.
pub(crate) fn request_body(request: &ChatRequest, stream: bool) -> Vec<u8> {
    request_body_with(request, stream, false)
}

/// Whether `host` is OpenRouter's API, which needs its file-parser plugin
/// pinned to the model's native PDF reading.
pub(crate) fn is_openrouter(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "openrouter.ai" || host.ends_with(".openrouter.ai")
}

/// A PDF filename for OpenAI file inputs, which expect the extension.
pub(crate) fn pdf_filename(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".pdf") {
        name.to_owned()
    } else {
        format!("{name}.pdf")
    }
}

/// [`request_body`]; with `openrouter`, a request carrying PDF parts pins
/// OpenRouter's `file-parser` plugin to the `native` engine, so the document
/// is never routed to a third-party parser.
pub(crate) fn request_body_with(request: &ChatRequest, stream: bool, openrouter: bool) -> Vec<u8> {
    let mut body = Map::new();
    body.insert("model".into(), json!(request.model.as_str()));
    body.insert(
        "messages".into(),
        Value::Array(request.messages.iter().map(message).collect()),
    );
    body.insert("stream".into(), json!(stream));
    if stream {
        body.insert("stream_options".into(), json!({ "include_usage": true }));
    }
    if !request.tools.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(
                request
                    .tools
                    .iter()
                    .map(|tool| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": tool.name,
                                "description": tool.description,
                                "parameters": tool.parameters,
                            }
                        })
                    })
                    .collect(),
            ),
        );
        body.insert(
            "tool_choice".into(),
            match &request.tool_choice {
                ToolChoice::Auto => json!("auto"),
                ToolChoice::None => json!("none"),
                ToolChoice::Required => json!("required"),
                ToolChoice::Tool { name } => {
                    json!({ "type": "function", "function": { "name": name } })
                }
            },
        );
    }
    if let Some(ResponseFormat::JsonSchema {
        name,
        schema,
        strict,
    }) = &request.response_format
    {
        body.insert(
            "response_format".into(),
            json!({
                "type": "json_schema",
                "json_schema": { "name": name, "schema": schema, "strict": strict },
            }),
        );
    }
    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(max_tokens) = request.max_tokens {
        body.insert("max_tokens".into(), json!(max_tokens));
    }
    if !request.stop.is_empty() {
        body.insert("stop".into(), json!(request.stop));
    }
    if openrouter
        && request
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .any(ContentPart::is_pdf)
    {
        body.insert(
            "plugins".into(),
            json!([{ "id": "file-parser", "pdf": { "engine": "native" } }]),
        );
    }
    // Serializing a `Value` cannot fail.
    serde_json::to_vec(&Value::Object(body)).unwrap_or_default()
}

fn message(message: &ChatMessage) -> Value {
    let mut wire = Map::new();
    wire.insert("role".into(), json!(message.role.as_str()));
    match message.role {
        Role::Assistant if !message.tool_calls.is_empty() => {
            wire.insert(
                "content".into(),
                if message.content.is_empty() {
                    Value::Null
                } else {
                    json!(message.content)
                },
            );
            wire.insert(
                "tool_calls".into(),
                Value::Array(
                    message
                        .tool_calls
                        .iter()
                        .map(|call| {
                            json!({
                                "id": call.id,
                                "type": "function",
                                "function": { "name": call.name, "arguments": call.arguments },
                            })
                        })
                        .collect(),
                ),
            );
        }
        Role::Tool => {
            wire.insert("tool_call_id".into(), json!(message.tool_call_id));
            wire.insert("content".into(), json!(message.content));
        }
        // Messages without parts keep plain string content: some compatible
        // servers reject content arrays.
        Role::User if !message.parts.is_empty() => {
            let mut content = Vec::with_capacity(message.parts.len() + 1);
            if !message.content.is_empty() {
                content.push(json!({ "type": "text", "text": message.content }));
            }
            for part in &message.parts {
                content.push(match part {
                    ContentPart::Image { .. } => {
                        json!({ "type": "image_url", "image_url": { "url": part.data_url() } })
                    }
                    ContentPart::Pdf { name, .. } => json!({
                        "type": "file",
                        "file": { "filename": pdf_filename(name), "file_data": part.data_url() },
                    }),
                });
            }
            wire.insert("content".into(), Value::Array(content));
        }
        _ => {
            wire.insert("content".into(), json!(message.content));
        }
    }
    Value::Object(wire)
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct WireUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

impl From<WireUsage> for Usage {
    fn from(usage: WireUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
        }
    }
}

pub(crate) fn finish_reason(reason: &str) -> FinishReason {
    match reason {
        "length" => FinishReason::Length,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

#[derive(Deserialize)]
struct Completion {
    #[serde(default)]
    choices: Vec<CompletionChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct CompletionChoice {
    #[serde(default)]
    index: u32,
    message: CompletionMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct CompletionMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<CompletionToolCall>,
}

#[derive(Deserialize)]
struct CompletionToolCall {
    #[serde(default)]
    id: Option<String>,
    function: CompletionFunction,
}

#[derive(Deserialize)]
struct CompletionFunction {
    name: String,
    #[serde(default)]
    arguments: Value,
}

/// Parse a non-streaming completion and replay it as stream events.
pub(crate) fn parse_completion(
    body: &str,
    on_event: &mut dyn FnMut(&StreamEvent),
) -> Result<ChatResponse, ProviderError> {
    let completion: Completion = serde_json::from_str(body)
        .map_err(|_| ProviderError::Protocol("malformed completion".into()))?;
    let choice = completion
        .choices
        .into_iter()
        .find(|choice| choice.index == 0)
        .ok_or_else(|| ProviderError::Protocol("the completion has no choices".into()))?;
    let text = choice.message.content.unwrap_or_default();
    if !text.is_empty() {
        on_event(&StreamEvent::TextDelta { text: text.clone() });
    }
    let mut tool_calls = Vec::new();
    for (index, call) in choice.message.tool_calls.into_iter().enumerate() {
        if call.function.name.is_empty() {
            return Err(ProviderError::Protocol(
                "a tool call has no function name".into(),
            ));
        }
        let arguments = match call.function.arguments {
            Value::String(text) => text,
            Value::Null => String::new(),
            other => other.to_string(),
        };
        let call = ToolCall {
            provider_metadata: None,
            id: call
                .id
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| format!("call_{index}")),
            name: call.function.name,
            arguments,
        };
        on_event(&StreamEvent::ToolCallDelta {
            index: u32::try_from(index).unwrap_or(u32::MAX),
            id: Some(call.id.clone()),
            name: Some(call.name.clone()),
            arguments: call.arguments.clone(),
        });
        tool_calls.push(call);
    }
    let usage: Usage = completion.usage.unwrap_or_default().into();
    on_event(&StreamEvent::Usage(usage));
    let finish_reason = match choice.finish_reason.as_deref().map(finish_reason) {
        Some(FinishReason::Stop) | None if !tool_calls.is_empty() => FinishReason::ToolCalls,
        Some(reason) => reason,
        None => FinishReason::Stop,
    };
    on_event(&StreamEvent::Done { finish_reason });
    let mut message = ChatMessage::assistant(text);
    message.tool_calls = tool_calls;
    Ok(ChatResponse {
        message,
        finish_reason,
        usage,
    })
}

/// A model reported by a provider's model listing, with the metadata the
/// vendor supplied. A field the listing did not report is `None`; the
/// known-family hint pass (`hints`) may fill such fields later, never the
/// reported ones.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredModel {
    pub id: ModelId,
    /// Context length when the server reports one (`context_length`,
    /// `max_model_len`, `context_window`, `max_context_length`, llama.cpp
    /// `meta.n_ctx`/`meta.n_ctx_train`, or OpenRouter
    /// `top_provider.context_length`).
    pub context_window: Option<u32>,
    /// A human label different from the id (`name` or `display_name`).
    pub display_name: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub tool_calls: Option<ToolCallSupport>,
    pub json_schema_output: Option<bool>,
    pub vision: Option<bool>,
    pub embeddings: Option<bool>,
    /// Whether the model accepts chat requests, when the listing says so
    /// (embedding, image, audio, and moderation entries report `false`).
    pub chat: Option<bool>,
    /// USD per million tokens (OpenRouter `pricing`).
    pub cost: Option<CostHint>,
}

impl DiscoveredModel {
    pub(crate) fn new(id: ModelId) -> Self {
        Self {
            id,
            context_window: None,
            display_name: None,
            max_output_tokens: None,
            tool_calls: None,
            json_schema_output: None,
            vision: None,
            embeddings: None,
            chat: None,
            cost: None,
        }
    }

    /// The descriptor this listing entry produces for `provider`.
    ///
    /// Limits reported by the listing make the descriptor `Discovered`; limits
    /// supplied only by the known-family table make it `Catalog`; otherwise it
    /// is `Assumed` with [`ASSUMED_CONTEXT_WINDOW`].
    pub(crate) fn into_descriptor(
        mut self,
        provider: &ProviderId,
        locality: Locality,
    ) -> ModelDescriptor {
        let reported = self.context_window.is_some();
        let hinted = crate::hints::apply(&mut self);
        let source = if reported {
            CapabilitySource::Discovered
        } else if hinted {
            CapabilitySource::Catalog
        } else {
            CapabilitySource::Assumed
        };
        let embeddings = self.embeddings.unwrap_or(false);
        let chat = self.chat.unwrap_or(!embeddings);
        let mut descriptor = ModelDescriptor::new(
            ModelRef::new(provider.clone(), self.id),
            locality,
            self.context_window.unwrap_or(ASSUMED_CONTEXT_WINDOW),
        )
        .with_capabilities(ModelCapabilities {
            chat,
            streaming: true,
            tool_calls: self.tool_calls.unwrap_or_default(),
            json_schema_output: self.json_schema_output.unwrap_or(false),
            vision: self.vision.unwrap_or(false),
            embeddings,
        })
        .with_roles([if embeddings {
            ModelRole::Embed
        } else {
            ModelRole::General
        }])
        .with_source(source);
        descriptor.max_output_tokens = self.max_output_tokens;
        descriptor.cost = self.cost;
        if let Some(name) = self.display_name {
            descriptor.display_name = name;
        }
        descriptor
    }
}

/// Declared models first (they override discoveries with the same id), then
/// each discovered model once, in listing order.
pub(crate) fn merge_discovered(
    provider: &ProviderId,
    locality: Locality,
    declared: &[ModelConfig],
    discovered: Vec<DiscoveredModel>,
) -> Vec<ModelDescriptor> {
    let mut seen: HashSet<ModelId> = declared.iter().map(|model| model.id.clone()).collect();
    let mut models: Vec<ModelDescriptor> = declared
        .iter()
        .map(|model| model.descriptor(provider, locality))
        .collect();
    for found in discovered {
        if seen.insert(found.id.clone()) {
            models.push(found.into_descriptor(provider, locality));
        }
    }
    models
}

fn malformed_list() -> ProviderError {
    ProviderError::Protocol("malformed model list".into())
}

/// Parse a `/models` listing: `{"data": [...]}` or a top-level array.
/// Entries with unusable ids or `"active": false` are skipped; malformed
/// optional fields are ignored rather than failing the listing.
pub(crate) fn parse_models(body: &str) -> Result<Vec<DiscoveredModel>, ProviderError> {
    let value: Value = serde_json::from_str(body).map_err(|_| malformed_list())?;
    parse_model_list(&value)
}

/// [`parse_models`] over an already parsed body.
pub(crate) fn parse_model_list(value: &Value) -> Result<Vec<DiscoveredModel>, ProviderError> {
    let entries = match value {
        Value::Array(entries) => entries,
        Value::Object(object) => object
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(malformed_list)?,
        _ => return Err(malformed_list()),
    };
    check_list_size(entries.len())?;
    Ok(entries.iter().filter_map(parse_model_entry).collect())
}

pub(crate) fn check_list_size(len: usize) -> Result<(), ProviderError> {
    if len > MAX_LISTED_MODELS {
        return Err(ProviderError::Protocol(
            "the model list is larger than 1000 entries".into(),
        ));
    }
    Ok(())
}

/// One OpenAI-style listing entry (also used for Anthropic's `data[]`).
pub(crate) fn parse_model_entry(entry: &Value) -> Option<DiscoveredModel> {
    if entry.get("active").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let id = ModelId::new(entry.get("id")?.as_str()?).ok()?;
    let mut model = DiscoveredModel::new(id);
    model.display_name = model_label(entry.get("name"), &model.id)
        .or_else(|| model_label(entry.get("display_name"), &model.id));
    let meta = entry.get("meta");
    let top = entry.get("top_provider");
    model.context_window = [
        entry.get("context_length"),
        entry.get("max_model_len"),
        entry.get("context_window"),
        entry.get("max_context_length"),
        meta.and_then(|meta| meta.get("n_ctx")),
        meta.and_then(|meta| meta.get("n_ctx_train")),
        top.and_then(|top| top.get("context_length")),
    ]
    .into_iter()
    .find_map(positive_u32);
    model.max_output_tokens = [
        top.and_then(|top| top.get("max_completion_tokens")),
        entry.get("max_completion_tokens"),
        entry.get("max_output_tokens"),
    ]
    .into_iter()
    .find_map(positive_u32);

    let parameters = entry.get("supported_parameters").and_then(Value::as_array);
    let capabilities = entry.get("capabilities").filter(|value| value.is_object());
    let capability = |name: &str| {
        capabilities
            .and_then(|capabilities| capabilities.get(name))
            .and_then(Value::as_bool)
    };
    model.tool_calls = parameters
        .map(|parameters| {
            if contains(parameters, "tools") {
                ToolCallSupport::Native
            } else {
                ToolCallSupport::None
            }
        })
        .or_else(|| {
            capability("function_calling").map(|supported| {
                if supported {
                    ToolCallSupport::Native
                } else {
                    ToolCallSupport::None
                }
            })
        });
    model.json_schema_output = parameters.map(|parameters| {
        contains(parameters, "structured_outputs") || contains(parameters, "response_format")
    });
    let image_input = entry
        .get("architecture")
        .and_then(|architecture| architecture.get("input_modalities"))
        .and_then(Value::as_array)
        .map(|modalities| contains(modalities, "image"));
    model.vision = match (image_input, capability("vision")) {
        (None, None) => None,
        (image, vision) => Some(image == Some(true) || vision == Some(true)),
    };
    let kind = entry
        .get("type")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    if matches!(kind.as_deref(), Some("embedding" | "embeddings"))
        || model.id.as_str().to_ascii_lowercase().contains("embed")
    {
        model.embeddings = Some(true);
        model.chat = Some(false);
    } else if matches!(
        kind.as_deref(),
        Some("image" | "audio" | "moderation" | "rerank")
    ) || capability("completion_chat") == Some(false)
    {
        model.chat = Some(false);
    }
    model.cost = entry.get("pricing").and_then(|pricing| {
        Some(CostHint {
            input_per_million: per_million(pricing.get("prompt"))?,
            output_per_million: per_million(pricing.get("completion"))?,
        })
    });
    Some(model)
}

/// A positive count that fits `u32`.
pub(crate) fn positive_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
}

/// A vendor label usable as a display name: 1-128 characters, no control
/// characters, and different from the model id.
pub(crate) fn model_label(value: Option<&Value>, id: &ModelId) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|label| {
            !label.is_empty()
                && label.chars().count() <= 128
                && !label.chars().any(char::is_control)
                && *label != id.as_str()
        })
        .map(str::to_owned)
}

fn contains(values: &[Value], needle: &str) -> bool {
    values.iter().any(|value| value.as_str() == Some(needle))
}

/// A per-token USD price (decimal string or number) scaled to per million
/// tokens; negative, non-finite, or unparsable prices are ignored.
fn per_million(value: Option<&Value>) -> Option<f64> {
    let per_token = match value? {
        Value::String(text) => text.trim().parse::<f64>().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    let scaled = (per_token * 1_000_000.0 * 1e6).round() / 1e6;
    (per_token.is_finite() && per_token >= 0.0 && scaled.is_finite()).then_some(scaled)
}

/// Map a failed HTTP status (and its error body, parsed for machine codes
/// only) to a provider error. Server-supplied text is never copied into it.
pub(crate) fn http_error(
    status: u16,
    retry_after: Option<std::time::Duration>,
    body: &str,
    model: Option<&ModelId>,
) -> ProviderError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let error = parsed.as_ref().and_then(|value| {
        value
            .get("error")
            .filter(|error| error.is_object())
            .or(Some(value))
    });
    classify(status, error, retry_after, model)
}

/// An `{"error": ...}` object received inside a stream.
pub(crate) fn stream_error(error: &Value) -> ProviderError {
    let status = error
        .get("code")
        .and_then(Value::as_str)
        .and_then(crate::error::chatgpt_error_status)
        .or_else(|| {
            error
                .get("code")
                .and_then(Value::as_u64)
                .and_then(|code| u16::try_from(code).ok())
                .filter(|code| (400..600).contains(code))
        })
        .unwrap_or(500);
    classify(status, Some(error), None, None)
}

fn classify(
    status: u16,
    error: Option<&Value>,
    retry_after: Option<std::time::Duration>,
    model: Option<&ModelId>,
) -> ProviderError {
    let field = |name: &str| {
        error
            .and_then(|error| error.get(name))
            .and_then(Value::as_str)
    };
    let code = field("code")
        .or_else(|| field("type"))
        .and_then(sanitize_code);
    if code
        .as_deref()
        .and_then(crate::error::chatgpt_error_status)
        .is_some()
    {
        return ProviderError::Http { status, code };
    }
    let kind = field("type").unwrap_or_default();
    let message = field("message").unwrap_or_default().to_ascii_lowercase();
    let number = |name: &str| {
        error
            .and_then(|error| error.get(name))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let context_overflow = code.as_deref() == Some("context_length_exceeded")
        || kind == "exceed_context_size_error"
        || [
            "context length",
            "context size",
            "context window",
            "maximum context",
        ]
        .iter()
        .any(|needle| message.contains(needle));
    match status {
        401 | 403 => ProviderError::Unauthorized,
        408 => ProviderError::Timeout,
        429 => ProviderError::RateLimited { retry_after },
        400 | 413 if context_overflow => ProviderError::ContextOverflow {
            limit: number("n_ctx"),
            requested: number("n_prompt_tokens"),
        },
        404 if code.as_deref().is_some_and(|code| code.contains("model"))
            || message.contains("model") =>
        {
            ProviderError::ModelNotFound {
                model: model.map(ToString::to_string).unwrap_or_default(),
            }
        }
        503 => ProviderError::unavailable("the provider is overloaded or still loading (HTTP 503)"),
        _ => ProviderError::Http { status, code },
    }
}

/// Keep short identifier-like codes; drop anything that could be free text.
fn sanitize_code(code: &str) -> Option<String> {
    if crate::error::chatgpt_error_status(code).is_some() {
        return Some(code.to_owned());
    }
    // Identifier-shaped strings can still be credentials. Only documented,
    // recognized machine codes may cross the error/log boundary.
    matches!(
        code,
        "context_length_exceeded"
            | "exceed_context_size_error"
            | "invalid_request_error"
            | "invalid_request"
            | "model_not_found"
            | "rate_limit_exceeded"
            | "rate_limit_error"
            | "insufficient_quota"
            | "authentication_error"
            | "permission_error"
            | "not_found_error"
            | "overloaded_error"
            | "server_error"
            | "api_error"
            | "RESOURCE_EXHAUSTED"
            | "INVALID_ARGUMENT"
            | "UNAUTHENTICATED"
            | "PERMISSION_DENIED"
            | "NOT_FOUND"
            | "UNAVAILABLE"
            | "INTERNAL"
    )
    .then(|| code.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelId, ToolSpec};

    #[test]
    fn request_bodies_follow_the_openai_schema() {
        let mut request = ChatRequest::new(
            ModelId::new("gpt-x").unwrap(),
            vec![
                ChatMessage::system("be brief"),
                ChatMessage::user("read a"),
                ChatMessage {
                    tool_calls: vec![ToolCall {
                        provider_metadata: None,
                        id: "c1".into(),
                        name: "read_file".into(),
                        arguments: "{\"path\":\"a\"}".into(),
                    }],
                    ..ChatMessage::assistant("")
                },
                ChatMessage::tool_result("c1", "contents"),
            ],
        )
        .with_tools(vec![ToolSpec {
            name: "read_file".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object"}),
        }])
        .with_response_format(ResponseFormat::JsonSchema {
            name: "action".into(),
            schema: json!({"type": "object"}),
            strict: true,
        })
        .with_max_tokens(64);
        request.stop = vec!["END".into()];
        let body: Value = serde_json::from_slice(&request_body(&request, true)).unwrap();
        assert_eq!(body["model"], "gpt-x");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["messages"][2]["content"], Value::Null);
        assert_eq!(
            body["messages"][2]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
        assert_eq!(body["messages"][3]["tool_call_id"], "c1");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["name"], "action");
        assert_eq!(body["max_tokens"], 64);
        assert_eq!(body["stop"][0], "END");
        let plain: Value = serde_json::from_slice(&request_body(
            &ChatRequest::new(ModelId::new("m").unwrap(), vec![ChatMessage::user("x")]),
            false,
        ))
        .unwrap();
        assert!(plain.get("tools").is_none() && plain.get("stream_options").is_none());
    }

    #[test]
    fn user_parts_become_content_arrays_and_openrouter_pins_native_pdfs() {
        use crate::parts::fixtures::{pdf, png};
        use base64::Engine as _;
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let image = || ContentPart::image("image/png", png(2, 2));
        let image_url = format!("data:image/png;base64,{}", b64(&png(2, 2)));
        let request = ChatRequest::new(
            ModelId::new("m").unwrap(),
            vec![
                ChatMessage::system("sys"),
                ChatMessage::user("look")
                    .with_parts(vec![image(), ContentPart::pdf("spec", pdf(), 1)]),
                ChatMessage::assistant(""),
                ChatMessage::user("").with_parts(vec![image()]),
                ChatMessage::user("plain"),
            ],
        );
        let body: Value = serde_json::from_slice(&request_body(&request, true)).unwrap();
        assert_eq!(
            body["messages"][1]["content"],
            json!([
                {"type": "text", "text": "look"},
                {"type": "image_url", "image_url": {"url": image_url}},
                {"type": "file", "file": {
                    "filename": "spec.pdf",
                    "file_data": format!("data:application/pdf;base64,{}", b64(&pdf())),
                }},
            ])
        );
        // Messages without parts keep today's exact form.
        assert_eq!(
            body["messages"][0],
            json!({"role": "system", "content": "sys"})
        );
        assert_eq!(
            body["messages"][2],
            json!({"role": "assistant", "content": ""})
        );
        assert_eq!(
            body["messages"][3]["content"],
            json!([{"type": "image_url", "image_url": {"url": image_url}}])
        );
        assert_eq!(
            body["messages"][4],
            json!({"role": "user", "content": "plain"})
        );
        assert!(body.get("plugins").is_none());

        let routed: Value =
            serde_json::from_slice(&request_body_with(&request, true, true)).unwrap();
        assert_eq!(
            routed["plugins"],
            json!([{"id": "file-parser", "pdf": {"engine": "native"}}])
        );
        let images_only = ChatRequest::new(
            ModelId::new("m").unwrap(),
            vec![ChatMessage::user("x").with_parts(vec![image()])],
        );
        let routed: Value =
            serde_json::from_slice(&request_body_with(&images_only, true, true)).unwrap();
        assert!(routed.get("plugins").is_none());
        for host in ["openrouter.ai", "OpenRouter.AI", "eu.openrouter.ai"] {
            assert!(is_openrouter(host), "{host}");
        }
        for host in [
            "openrouter.ai.example.com",
            "notopenrouter.ai",
            "api.openai.com",
            "127.0.0.1",
        ] {
            assert!(!is_openrouter(host), "{host}");
        }
        assert_eq!(pdf_filename("a.PDF"), "a.PDF");
        assert_eq!(pdf_filename("notes"), "notes.pdf");
    }

    #[test]
    fn model_listings_report_known_context_fields() {
        let models = parse_models(
            r#"{"object":"list","data":[
                {"id":"gpt-4o","object":"model"},
                {"id":"meta-llama/Llama-3.1-8B","max_model_len":131072},
                {"id":"qwen","meta":{"n_ctx_train":32768}},
                {"id":"bad id with spaces"},
                {"id":"~x-ai/grok-latest"},
                {"id":"router/x","context_length":8192}
            ]}"#,
        )
        .unwrap();
        let summary: Vec<_> = models
            .iter()
            .map(|model| (model.id.as_str(), model.context_window))
            .collect();
        assert_eq!(
            summary,
            [
                ("gpt-4o", None),
                ("meta-llama/Llama-3.1-8B", Some(131_072)),
                ("qwen", Some(32_768)),
                ("router/x", Some(8192))
            ]
        );
        assert!(parse_models("{}").is_err());
    }

    #[test]
    fn errors_map_to_first_class_outcomes_without_server_text() {
        let model = ModelId::new("m").unwrap();
        let echoed = r#"{"error":{"message":"Incorrect API key provided: sk-live-123","type":"invalid_request_error","code":"invalid_api_key"}}"#;
        let unauthorized = http_error(401, None, echoed, Some(&model));
        assert_eq!(unauthorized, ProviderError::Unauthorized);
        assert!(!format!("{unauthorized} {unauthorized:?}").contains("sk-live-123"));
        assert_eq!(
            http_error(429, Some(std::time::Duration::from_secs(7)), "", None),
            ProviderError::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(7))
            }
        );
        assert_eq!(
            http_error(
                400,
                None,
                r#"{"error":{"code":400,"message":"the request exceeds the available context size","type":"exceed_context_size_error","n_prompt_tokens":5000,"n_ctx":4096}}"#,
                None
            ),
            ProviderError::ContextOverflow {
                limit: Some(4096),
                requested: Some(5000)
            }
        );
        assert!(matches!(
            http_error(
                404,
                None,
                r#"{"error":{"code":"model_not_found"}}"#,
                Some(&model)
            ),
            ProviderError::ModelNotFound { .. }
        ));
        let free_text = http_error(
            500,
            None,
            r#"{"error":{"code":"has spaces and sk-1"}}"#,
            None,
        );
        assert_eq!(
            free_text,
            ProviderError::Http {
                status: 500,
                code: None
            }
        );
        assert!(matches!(
            http_error(503, None, "", None),
            ProviderError::Unavailable { .. }
        ));
    }

    #[test]
    fn non_streaming_completions_replay_as_events() {
        let mut events = Vec::new();
        let response = parse_completion(
            r#"{"choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"id":"x","type":"function","function":{"name":"f","arguments":{"a":1}}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#,
            &mut |event| events.push(event.clone()),
        )
        .unwrap();
        assert_eq!(response.message.tool_calls[0].arguments, r#"{"a":1}"#);
        assert_eq!(response.finish_reason, FinishReason::ToolCalls);
        assert_eq!(events.len(), 3);
    }
}
