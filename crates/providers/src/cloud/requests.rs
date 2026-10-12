use crate::*;
use serde_json::{Value, json};

fn arguments(text: &str) -> Result<Value, ProviderError> {
    serde_json::from_str(text)
        .map_err(|_| ProviderError::InvalidRequest("tool arguments must be JSON".into()))
}
pub(super) fn encode_segment(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
/// Vendor turns assembled from consecutive same-role messages.
///
/// Anthropic, Gemini and Bedrock need alternating roles and reject empty
/// content arrays. Messages that produce no blocks (a failed run's empty
/// assistant placeholder) are skipped, and a message whose role matches the
/// previous turn joins it: tool results first (vendors require them to lead
/// the turn that answers the calls), then the other blocks in message order.
#[derive(Default)]
pub(super) struct Turns(Vec<Turn>);

pub(super) struct Turn {
    pub role: &'static str,
    pub results: Vec<Value>,
    pub blocks: Vec<Value>,
}

impl Turn {
    /// The turn's content array: tool results, then the other blocks.
    pub fn into_content(self) -> Vec<Value> {
        let mut content = self.results;
        content.extend(self.blocks);
        content
    }
}

impl Turns {
    pub fn push(&mut self, role: &'static str, results: Vec<Value>, blocks: Vec<Value>) {
        if results.is_empty() && blocks.is_empty() {
            return;
        }
        match self.0.last_mut() {
            Some(last) if last.role == role => {
                last.results.extend(results);
                last.blocks.extend(blocks);
            }
            _ => self.0.push(Turn {
                role,
                results,
                blocks,
            }),
        }
    }

    pub fn into_inner(self) -> Vec<Turn> {
        self.0
    }
}

pub(super) fn build(kind: ProviderKind, r: &ChatRequest) -> Result<(String, Value), ProviderError> {
    crate::input::check_request(kind, r)?;
    match kind {
        ProviderKind::AzureOpenai => Ok((
            format!(
                "deployments/{}/chat/completions?api-version=2024-10-21",
                encode_segment(r.model.as_str())
            ),
            serde_json::from_slice(&crate::openai::wire::request_body(r, true))
                .map_err(|_| super::protocol())?,
        )),
        ProviderKind::OpenaiResponses => responses(r),
        ProviderKind::Anthropic => anthropic(r),
        ProviderKind::Gemini => gemini(r),
        ProviderKind::Bedrock => super::bedrock::request(r),
        _ => Err(ProviderError::InvalidRequest(
            "unsupported cloud protocol".into(),
        )),
    }
}
fn responses(r: &ChatRequest) -> Result<(String, Value), ProviderError> {
    let mut input = Vec::new();
    for m in &r.messages {
        if m.role == Role::Tool {
            input.push(
                json!({"type":"function_call_output","call_id":m.tool_call_id,"output":m.content}),
            );
        } else {
            let role = if m.role == Role::System {
                "developer"
            } else {
                m.role.as_str()
            };
            if !m.parts.is_empty() {
                // Only user messages carry parts (validated); a message with
                // parts is sent even when its text is empty.
                let mut content = Vec::with_capacity(m.parts.len() + 1);
                if !m.content.is_empty() {
                    content.push(json!({"type":"input_text","text":m.content}));
                }
                for part in &m.parts {
                    content.push(match part {
                        ContentPart::Image { .. } => {
                            json!({"type":"input_image","image_url":part.data_url(),"detail":"auto"})
                        }
                        ContentPart::Pdf { name, .. } => {
                            json!({"type":"input_file","filename":crate::openai::wire::pdf_filename(name),"file_data":part.data_url()})
                        }
                    });
                }
                input.push(json!({"role":role,"content":content}));
            } else if !m.content.is_empty() {
                input.push(json!({"role":role,"content":m.content}));
            }
            for call in &m.tool_calls {
                input.push(json!({"type":"function_call","call_id":call.id,"name":call.name,"arguments":call.arguments}));
            }
        }
    }
    let mut body = json!({"model":r.model,"input":input,"store":false,"stream":true});
    if !r.tools.is_empty() {
        body["tools"] = json!(r.tools.iter().map(|t| json!({"type":"function","name":t.name,"description":t.description,"parameters":t.parameters})).collect::<Vec<_>>());
        body["tool_choice"] = match &r.tool_choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::None => json!("none"),
            ToolChoice::Required => json!("required"),
            ToolChoice::Tool { name } => json!({"type":"function","name":name}),
        };
    }
    if let Some(n) = r.max_tokens {
        body["max_output_tokens"] = json!(n);
    }
    if let Some(t) = r.temperature {
        body["temperature"] = json!(t);
    }
    if !r.stop.is_empty() {
        return Err(ProviderError::InvalidRequest(
            "Responses does not accept stop sequences".into(),
        ));
    }
    if let Some(ResponseFormat::JsonSchema {
        name,
        schema,
        strict,
    }) = &r.response_format
    {
        body["text"] =
            json!({"format":{"type":"json_schema","name":name,"schema":schema,"strict":strict}});
    }
    Ok(("responses".into(), body))
}
fn anthropic(r: &ChatRequest) -> Result<(String, Value), ProviderError> {
    if r.response_format.is_some() {
        return Err(ProviderError::InvalidRequest(
            "Anthropic constrained output is not supported by this adapter".into(),
        ));
    }
    let mut turns = Turns::default();
    let mut system = Vec::new();
    for m in &r.messages {
        if m.role == Role::System {
            system.push(json!({"type":"text","text":m.content}));
            continue;
        }
        let (mut results, mut blocks) = (Vec::new(), Vec::new());
        if m.role == Role::Tool {
            results.push(
                json!({"type":"tool_result","tool_use_id":m.tool_call_id,"content":m.content}),
            );
        } else {
            // Images and documents go before the text that refers to them.
            for part in &m.parts {
                blocks.push(match part {
                    ContentPart::Image { mime, .. } => {
                        json!({"type":"image","source":{"type":"base64","media_type":mime,"data":part.base64()}})
                    }
                    ContentPart::Pdf { .. } => {
                        json!({"type":"document","source":{"type":"base64","media_type":crate::PDF_MIME,"data":part.base64()}})
                    }
                });
            }
            if !m.content.is_empty() {
                blocks.push(json!({"type":"text","text":m.content}));
            }
            for call in &m.tool_calls {
                blocks.push(json!({"type":"tool_use","id":call.id,"name":call.name,"input":arguments(&call.arguments)?}));
            }
        }
        turns.push(
            if m.role == Role::Assistant {
                "assistant"
            } else {
                "user"
            },
            results,
            blocks,
        );
    }
    let messages: Vec<Value> = turns
        .into_inner()
        .into_iter()
        .map(|turn| {
            let role = turn.role;
            json!({"role":role,"content":turn.into_content()})
        })
        .collect();
    let mut body = json!({"model":r.model,"messages":messages,"max_tokens":r.max_tokens.unwrap_or(4096),"stream":true});
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if let Some(t) = r.temperature {
        body["temperature"] = json!(t);
    }
    if !r.stop.is_empty() {
        body["stop_sequences"] = json!(r.stop);
    }
    if !r.tools.is_empty() {
        body["tools"] = json!(r.tools.iter().map(|t| json!({"name":t.name,"description":t.description,"input_schema":t.parameters})).collect::<Vec<_>>());
        body["tool_choice"] = match &r.tool_choice {
            ToolChoice::Auto => json!({"type":"auto"}),
            ToolChoice::None => json!({"type":"none"}),
            ToolChoice::Required => json!({"type":"any"}),
            ToolChoice::Tool { name } => json!({"type":"tool","name":name}),
        };
    }
    Ok(("messages".into(), body))
}
fn gemini(r: &ChatRequest) -> Result<(String, Value), ProviderError> {
    let mut turns = Turns::default();
    let mut systems = Vec::new();
    for m in &r.messages {
        if m.role == Role::System {
            systems.push(json!({"text":m.content}));
            continue;
        }
        let (mut results, mut parts) = (Vec::new(), Vec::new());
        if m.role == Role::Tool {
            let call = r
                .messages
                .iter()
                .flat_map(|m| &m.tool_calls)
                .find(|c| Some(&c.id) == m.tool_call_id.as_ref())
                .ok_or_else(|| {
                    ProviderError::InvalidRequest(
                        "Gemini tool results require a preceding matching call".into(),
                    )
                })?;
            results.push(json!({"functionResponse":{"id":call.id,"name":call.name,"response":{"result":m.content}}}));
        } else {
            // Inline data goes before the text that refers to it.
            for part in &m.parts {
                parts.push(json!({"inlineData":{"mimeType":part.mime(),"data":part.base64()}}));
            }
            if !m.content.is_empty() {
                parts.push(json!({"text":m.content}));
            }
            for call in &m.tool_calls {
                let args = arguments(&call.arguments)?;
                let part = if let Some(original) = call
                    .provider_metadata
                    .as_ref()
                    .and_then(|v| v.get("gemini"))
                    .and_then(|v| v.get("part"))
                {
                    if original["functionCall"]["name"] != call.name
                        || original["functionCall"]["args"] != args
                        || original["functionCall"]
                            .get("id")
                            .is_some_and(|id| id != &call.id)
                    {
                        return Err(ProviderError::InvalidRequest(
                            "Gemini tool metadata no longer matches its function call".into(),
                        ));
                    }
                    original.clone()
                } else {
                    json!({"functionCall":{"id":call.id,"name":call.name,"args":args}})
                };
                parts.push(part);
            }
        }
        turns.push(
            if m.role == Role::Assistant {
                "model"
            } else {
                "user"
            },
            results,
            parts,
        );
    }
    let contents: Vec<Value> = turns
        .into_inner()
        .into_iter()
        .map(|turn| {
            let role = turn.role;
            json!({"role":role,"parts":turn.into_content()})
        })
        .collect();
    let mut body = json!({"contents":contents,"generationConfig":{}});
    if !systems.is_empty() {
        body["systemInstruction"] = json!({"parts":systems});
    }
    if let Some(n) = r.max_tokens {
        body["generationConfig"]["maxOutputTokens"] = json!(n);
    }
    if let Some(t) = r.temperature {
        body["generationConfig"]["temperature"] = json!(t);
    }
    if !r.stop.is_empty() {
        body["generationConfig"]["stopSequences"] = json!(r.stop);
    }
    if let Some(ResponseFormat::JsonSchema { schema, .. }) = &r.response_format {
        body["generationConfig"]["responseMimeType"] = json!("application/json");
        body["generationConfig"]["responseJsonSchema"] = schema.clone();
    }
    if !r.tools.is_empty() {
        body["tools"] = json!([{"functionDeclarations":r.tools.iter().map(|t| json!({"name":t.name,"description":t.description,"parametersJsonSchema":t.parameters})).collect::<Vec<_>>()}]);
        body["toolConfig"] = json!({"functionCallingConfig":match &r.tool_choice { ToolChoice::Auto => json!({"mode":"AUTO"}), ToolChoice::None => json!({"mode":"NONE"}), ToolChoice::Required => json!({"mode":"ANY"}), ToolChoice::Tool { name } => json!({"mode":"ANY","allowedFunctionNames":[name]}) }});
    }
    Ok((
        format!(
            "models/{}:streamGenerateContent?alt=sse",
            encode_segment(
                r.model
                    .as_str()
                    .strip_prefix("models/")
                    .unwrap_or(r.model.as_str())
            )
        ),
        body,
    ))
}
