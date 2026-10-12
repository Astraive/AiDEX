//! Image and PDF parts over HTTP, and input capabilities by account.

mod support;

use std::sync::Arc;

use aidex_providers::*;
use serde_json::{Value, json};
use support::{FakeServer, Reply};

fn png() -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&64u32.to_be_bytes());
    bytes.extend_from_slice(&48u32.to_be_bytes());
    bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

fn gif() -> Vec<u8> {
    let mut bytes = b"GIF89a\x01\x00\x01\x00\x00\x00\x00".to_vec();
    bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 2, 0x4C, 0x01, 0, 0x3B]);
    bytes
}

fn pdf() -> Vec<u8> {
    b"%PDF-1.7\n1 0 obj << /Type /Page >> endobj\n%%EOF\n".to_vec()
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn registry() -> ProviderRegistry {
    let keys = Arc::new(StaticCredentials::new());
    for id in ["compat", "claude", "gem", "plan", "other"] {
        let id = ProviderId::new(id).unwrap();
        keys.insert(
            id.clone(),
            "key",
            SecretBytes::from("fixture-secret".to_owned()),
        );
        keys.insert(
            id.clone(),
            format!("oauth.{id}.access"),
            SecretBytes::from("fixture-secret".to_owned()),
        );
    }
    ProviderRegistry::in_memory(keys, Arc::new(OfflineMode::new(false)))
}

fn vision_model(id: &str) -> ModelConfig {
    let mut model = ModelConfig::new(ModelId::new(id).unwrap(), 128_000);
    model.vision = true;
    model
}

fn add(
    registry: &ProviderRegistry,
    id: &str,
    kind: ProviderKind,
    base_url: &str,
    preset: Option<&str>,
    secret_name: &str,
) -> ModelDescriptor {
    let mut config = ProviderConfig::new(ProviderId::new(id).unwrap(), kind, base_url);
    config.discover_models = false;
    config.models.push(vision_model("eyes"));
    config.auth = AuthConfig::VaultSecret {
        secret_name: secret_name.into(),
    };
    registry
        .add_provider_with(
            config,
            AccountMetadata {
                preset_id: preset.map(str::to_owned),
                ..AccountMetadata::default()
            },
        )
        .unwrap();
    let provider = registry.provider(&ProviderId::new(id).unwrap()).unwrap();
    provider.models().unwrap().remove(0)
}

fn request(parts: Vec<ContentPart>) -> ChatRequest {
    ChatRequest::new(
        ModelId::new("eyes").unwrap(),
        vec![
            ChatMessage::system("sys"),
            ChatMessage::user("what is in these").with_parts(parts),
        ],
    )
}

#[test]
fn compatible_requests_carry_image_and_file_parts() {
    let server = FakeServer::start(vec![Reply::sse(&[
        r#"{"choices":[{"delta":{"content":"a chart"},"finish_reason":"stop"}]}"#,
        "[DONE]",
    ])]);
    let registry = registry();
    let model = add(
        &registry,
        "compat",
        ProviderKind::OpenaiCompatible,
        &server.base_url(),
        Some("openai-chat"),
        "key",
    );
    assert_eq!(
        registry.model_input(&model),
        ModelInput {
            images: true,
            pdf: true
        }
    );
    let parts = vec![
        ContentPart::image("image/png", png()),
        ContentPart::pdf("report", pdf(), 1),
    ];
    assert!(parts.iter().all(|part| registry.accepts_part(&model, part)));
    let provider = registry.provider(&model.model.provider_id).unwrap();
    let response = provider
        .chat(&request(parts), &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(response.message.content, "a chart");
    let body = server.requests()[0].json();
    assert_eq!(
        body["messages"][0],
        json!({"role": "system", "content": "sys"})
    );
    assert_eq!(
        body["messages"][1]["content"],
        json!([
            {"type": "text", "text": "what is in these"},
            {"type": "image_url", "image_url": {"url": data_url("image/png", &png())}},
            {"type": "file", "file": {"filename": "report.pdf", "file_data": data_url("application/pdf", &pdf())}},
        ])
    );
    // A loopback endpoint is not OpenRouter: no plugin field.
    assert!(body.get("plugins").is_none());
}

#[test]
fn anthropic_requests_carry_image_and_document_blocks() {
    let server = FakeServer::start(vec![Reply::sse(&[
        r#"{"type":"message_start","message":{"usage":{"input_tokens":5,"output_tokens":0}}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"ok"}}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}"#,
        r#"{"type":"message_stop"}"#,
    ])]);
    let registry = registry();
    let model = add(
        &registry,
        "claude",
        ProviderKind::Anthropic,
        &server.base_url(),
        Some("anthropic"),
        "key",
    );
    let provider = registry.provider(&model.model.provider_id).unwrap();
    let mut history = request(vec![
        ContentPart::image("image/png", png()),
        ContentPart::pdf("report.pdf", pdf(), 1),
    ]);
    // A failed earlier run left an empty placeholder after the question.
    history.messages.push(ChatMessage::assistant(""));
    history.messages.push(ChatMessage::user("try again"));
    provider
        .chat(&history, &mut |_| {}, &CancelToken::new())
        .unwrap();
    let body = server.requests()[0].json();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1, "{body}");
    let content = &messages[0]["content"];
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["source"]["media_type"], "image/png");
    assert_eq!(content[1]["type"], "document");
    assert_eq!(content[1]["source"]["media_type"], "application/pdf");
    assert_eq!(
        content[2],
        json!({"type": "text", "text": "what is in these"})
    );
    assert_eq!(content[3], json!({"type": "text", "text": "try again"}));
    let raw = String::from_utf8_lossy(&server.requests()[0].body).into_owned();
    assert!(
        !raw.contains("report.pdf"),
        "Anthropic receives no file name"
    );
}

#[test]
fn refused_parts_open_no_connection() {
    let server = FakeServer::start(Vec::new());
    let registry = registry();
    let model = add(
        &registry,
        "gem",
        ProviderKind::Gemini,
        &server.base_url(),
        Some("gemini"),
        "key",
    );
    let gif = ContentPart::image("image/gif", gif());
    assert!(!registry.accepts_part(&model, &gif));
    let provider = registry.provider(&model.model.provider_id).unwrap();
    let error = provider
        .chat(&request(vec![gif]), &mut |_| {}, &CancelToken::new())
        .unwrap_err();
    assert!(
        matches!(error, ProviderError::InvalidRequest(_)),
        "{error:?}"
    );
    // A part on a non-user message is invalid for every protocol.
    let mut wrong_role = request(Vec::new());
    wrong_role.messages[0] =
        ChatMessage::system("sys").with_parts(vec![ContentPart::image("image/png", png())]);
    assert!(matches!(
        provider.chat(&wrong_role, &mut |_| {}, &CancelToken::new()),
        Err(ProviderError::InvalidRequest(_))
    ));
    assert_eq!(server.connections(), 0);
}

#[test]
fn xai_accounts_take_only_png_and_jpeg_images() {
    let registry = registry();
    let grok = add(
        &registry,
        "compat",
        ProviderKind::OpenaiCompatible,
        "https://api.x.ai/v1",
        Some("xai"),
        "key",
    );
    let other = add(
        &registry,
        "other",
        ProviderKind::OpenaiCompatible,
        "https://api.groq.com/openai/v1",
        Some("groq"),
        "key",
    );
    let mut webp = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0".to_vec();
    webp.extend_from_slice(&[0, 0, 0, 0x9D, 0x01, 0x2A, 0x40, 0x00, 0x30, 0x00]);
    let webp = ContentPart::image("image/webp", webp);
    let gif = ContentPart::image("image/gif", gif());
    let png = ContentPart::image("image/png", png());
    assert!(registry.accepts_part(&grok, &png));
    assert!(!registry.accepts_part(&grok, &webp));
    assert!(!registry.accepts_part(&grok, &gif));
    for part in [&png, &webp, &gif] {
        assert!(registry.accepts_part(&other, part), "{part:?}");
    }
}

#[test]
fn input_follows_preset_plan_and_runtime_providers() {
    let registry = registry();
    let custom = add(
        &registry,
        "other",
        ProviderKind::OpenaiCompatible,
        "http://127.0.0.1:9/v1",
        Some("custom"),
        "key",
    );
    assert_eq!(
        registry.model_input(&custom),
        ModelInput {
            images: true,
            pdf: false
        }
    );
    let plan = add(
        &registry,
        "plan",
        ProviderKind::OpenaiResponses,
        "https://api.openai.com/v1",
        Some("openai"),
        "oauth.plan.access",
    );
    assert_eq!(
        registry.model_input(&plan),
        ModelInput {
            images: true,
            pdf: false
        }
    );
    assert!(!registry.accepts_part(&plan, &ContentPart::pdf("a.pdf", pdf(), 1)));
    assert_eq!(
        registry
            .part_limits(&plan)
            .map(|limits| limits.max_request_encoded_bytes),
        Some(32_000_000)
    );

    let id = ProviderId::new("local").unwrap();
    let blind = ModelDescriptor::new(
        ModelRef::new(id.clone(), ModelId::new("blind").unwrap()),
        Locality::Local,
        4096,
    );
    registry
        .register(Box::new(
            FakeProvider::new(id, Locality::Local).with_model(blind.clone()),
        ))
        .unwrap();
    assert_eq!(registry.model_input(&blind), ModelInput::default());
    let mut unknown = blind.clone();
    unknown.model.provider_id = ProviderId::new("missing").unwrap();
    unknown.capabilities.vision = true;
    assert_eq!(registry.model_input(&unknown), ModelInput::default());
    assert!(registry.part_limits(&unknown).is_none());
    let value: Value = serde_json::to_value(registry.model_input(&custom)).unwrap();
    assert_eq!(value, json!({"images": true, "pdf": false}));
}
