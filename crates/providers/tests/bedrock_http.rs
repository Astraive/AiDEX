mod support;
use aidex_providers::*;
use serde_json::json;
use std::sync::Arc;
use support::{FakeServer, Reply};

fn crc(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for b in bytes {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb88320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}
fn frame(name: &str, body: serde_json::Value) -> Vec<u8> {
    let mut headers = Vec::new();
    for (key, value) in [
        (":message-type", "event"),
        (":event-type", name),
        (":content-type", "application/json"),
    ] {
        headers.push(key.len() as u8);
        headers.extend(key.as_bytes());
        headers.push(7);
        headers.extend((value.len() as u16).to_be_bytes());
        headers.extend(value.as_bytes());
    }
    let body = body.to_string();
    let mut out = Vec::new();
    out.extend(((16 + headers.len() + body.len()) as u32).to_be_bytes());
    out.extend((headers.len() as u32).to_be_bytes());
    out.extend(crc(&out).to_be_bytes());
    out.extend(headers);
    out.extend(body.as_bytes());
    out.extend(crc(&out).to_be_bytes());
    out
}
fn provider(server: &FakeServer) -> Arc<dyn ModelProvider> {
    let kind = "bedrock".parse::<ProviderKind>();
    assert!(kind.is_ok(), "Bedrock must be configurable: {kind:?}");
    let mut cfg = ProviderConfig::new(
        ProviderId::new("aws").unwrap(),
        kind.unwrap(),
        format!("http://{}", server.addr),
    );
    cfg.models.push(ModelConfig::new(
        ModelId::new("anthropic.model:0").unwrap(),
        4096,
    ));
    cfg.discover_models = false;
    cfg.auth = AuthConfig::VaultSecret {
        secret_name: "aws".into(),
    };
    let keys = Arc::new(StaticCredentials::new());
    keys.insert(cfg.id.clone(), "aws", SecretBytes::from(json!({"access_key_id":"AKIDEXAMPLE","secret_access_key":"fixture-secret","region":"us-east-1","session_token":"fixture-session"}).to_string()));
    let registry = ProviderRegistry::in_memory(keys, Arc::new(OfflineMode::new(false)));
    registry.add_provider(cfg).unwrap();
    registry.provider(&ProviderId::new("aws").unwrap()).unwrap()
}
fn request() -> ChatRequest {
    ChatRequest::new(
        ModelId::new("anthropic.model:0").unwrap(),
        vec![ChatMessage::user("hello")],
    )
}
#[test]
fn bedrock_signs_converse_and_decodes_checked_binary_event_frames() {
    let frames: Vec<u8> = [
        frame("contentBlockDelta", json!({"contentBlockIndex":0,"delta":{"text":"Hi"}})),
        frame("contentBlockStart", json!({"contentBlockIndex":1,"start":{"toolUse":{"toolUseId":"call1","name":"lookup"}}})),
        frame("contentBlockDelta", json!({"contentBlockIndex":1,"delta":{"toolUse":{"input":"{\"x\":"}}})),
        frame("contentBlockDelta", json!({"contentBlockIndex":1,"delta":{"toolUse":{"input":"1}"}}})),
        frame("messageStop", json!({"stopReason":"tool_use"})),
        frame("metadata", json!({"usage":{"inputTokens":5,"outputTokens":7}}))
    ].concat();
    let server = FakeServer::start(vec![Reply::Binary(frames)]);
    let result = provider(&server)
        .chat(&request(), &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(result.message.content, "Hi");
    assert_eq!(result.message.tool_calls[0].arguments, "{\"x\":1}");
    assert_eq!(result.finish_reason, FinishReason::ToolCalls);
    assert_eq!(result.usage.completion_tokens, Some(7));
    let req = &server.requests()[0];
    assert_eq!(req.path, "/model/anthropic.model%3A0/converse-stream");
    assert_eq!(req.json()["messages"][0]["content"][0]["text"], "hello");
    let auth = req.header("authorization").unwrap();
    assert!(auth.starts_with("AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/"));
    assert!(auth.contains("/us-east-1/bedrock/aws4_request"));
    assert_eq!(req.header("x-amz-security-token"), Some("fixture-session"));
    // Hand-derived canonical request, built from the observed HTTP boundary,
    // independently of the adapter's signer helpers.
    use sha2::{Digest, Sha256};
    let digest = |bytes: &[u8]| -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };
    let date = req.header("x-amz-date").unwrap();
    let body_hash = digest(&req.body);
    assert_eq!(req.header("x-amz-content-sha256"), Some(body_hash.as_str()));
    let canonical = format!(
        "POST\n/model/anthropic.model%253A0/converse-stream\n\ncontent-type:application/json\nhost:{}\nx-amz-content-sha256:{body_hash}\nx-amz-date:{date}\nx-amz-security-token:fixture-session\n\ncontent-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token\n{body_hash}",
        server.addr
    );
    let scope = format!("{}/us-east-1/bedrock/aws4_request", &date[..8]);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",
        digest(canonical.as_bytes())
    );
    use hmac::{Hmac, Mac};
    let sign = |key: &[u8], text: &str| -> Vec<u8> {
        let mut hmac = Hmac::<Sha256>::new_from_slice(key).unwrap();
        hmac.update(text.as_bytes());
        hmac.finalize().into_bytes().to_vec()
    };
    let day = sign(b"AWS4fixture-secret", &date[..8]);
    let region = sign(&day, "us-east-1");
    let service = sign(&region, "bedrock");
    let key = sign(&service, "aws4_request");
    let expected: String = sign(&key, &to_sign)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(
        auth.ends_with(&format!("Signature={expected}")),
        "signature must authenticate the exact observed request"
    );
    assert!(!String::from_utf8_lossy(&req.raw).contains("fixture-secret"));
}
#[test]
fn bedrock_rejects_crc_corruption_and_incomplete_messages() {
    let mut corrupt = frame("messageStop", json!({"stopReason":"end_turn"}));
    corrupt[12] ^= 1;
    for bytes in [
        corrupt,
        frame(
            "contentBlockDelta",
            json!({"contentBlockIndex":0,"delta":{"text":"partial"}}),
        ),
    ] {
        let server = FakeServer::start(vec![Reply::Binary(bytes)]);
        assert_eq!(
            provider(&server)
                .chat(&request(), &mut |_| {}, &CancelToken::new())
                .unwrap_err()
                .kind(),
            ProviderErrorKind::Protocol
        );
    }
}

#[test]
fn bedrock_http_errors_and_cancellation_are_sanitized() {
    for (status, expected) in [
        (401, ProviderErrorKind::Unauthorized),
        (429, ProviderErrorKind::RateLimited),
        (500, ProviderErrorKind::Http),
        (503, ProviderErrorKind::Unavailable),
    ] {
        let server = FakeServer::start(vec![Reply::Json(
            status,
            json!({"message":"fixture-secret"}).to_string(),
        )]);
        let error = provider(&server)
            .chat(&request(), &mut |_| {}, &CancelToken::new())
            .unwrap_err();
        assert_eq!(error.kind(), expected);
        assert!(!error.to_string().contains("fixture-secret"));
    }
    let server = FakeServer::start(vec![Reply::Sse(vec![(
        std::time::Duration::from_secs(5),
        "data: {}\n\n".into(),
    )])]);
    let cancel = CancelToken::new();
    let other = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(60));
        other.cancel();
    });
    let mut events = Vec::new();
    assert_eq!(
        provider(&server)
            .chat(&request(), &mut |e| events.push(e.clone()), &cancel)
            .unwrap_err(),
        ProviderError::Cancelled
    );
    assert_eq!(
        events,
        vec![StreamEvent::Done {
            finish_reason: FinishReason::Cancelled
        }]
    );
}

#[test]
fn bedrock_health_distinguishes_missing_credentials_from_unsupported_probe() {
    let server = FakeServer::start(Vec::new());
    let registry = ProviderRegistry::in_memory(
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    );
    let mut config = ProviderConfig::new(
        ProviderId::new("aws").unwrap(),
        ProviderKind::Bedrock,
        format!("http://{}", server.addr),
    );
    config.discover_models = false;
    config
        .models
        .push(ModelConfig::new(ModelId::new("model").unwrap(), 4096));
    registry.add_provider(config).unwrap();
    assert_eq!(
        registry
            .health(&ProviderId::new("aws").unwrap())
            .unwrap()
            .status,
        HealthStatus::AuthRequired
    );
    assert_eq!(provider(&server).health().status, HealthStatus::Unavailable);
    assert_eq!(server.connections(), 0);
}

#[test]
fn bedrock_parallel_results_form_one_user_turn_with_every_call_id() {
    let server = FakeServer::start(vec![Reply::Binary(frame(
        "messageStop",
        json!({"stopReason":"end_turn"}),
    ))]);
    let assistant: ChatMessage = serde_json::from_value(json!({"role":"assistant","content":"checking","tool_calls":[{"id":"one","name":"lookup","arguments":"{\"x\":1}"},{"id":"two","name":"lookup","arguments":"{\"x\":2}"}]})).unwrap();
    let request = ChatRequest::new(
        ModelId::new("anthropic.model:0").unwrap(),
        vec![
            ChatMessage::user("hello"),
            assistant,
            ChatMessage::tool_result("one", "first"),
            ChatMessage::tool_result("two", "second"),
            ChatMessage::user("summarize"),
        ],
    );
    provider(&server)
        .chat(&request, &mut |_| {}, &CancelToken::new())
        .unwrap();
    let body = server.requests()[0].json();
    // Converse needs alternating roles: the follow-up user text joins the
    // turn that answers the calls, after the tool results.
    assert_eq!(body["messages"].as_array().unwrap().len(), 3);
    assert_eq!(body["messages"][2]["role"], "user");
    assert_eq!(
        body["messages"][2]["content"],
        json!([
            {"toolResult":{"toolUseId":"one","content":[{"text":"first"}]}},
            {"toolResult":{"toolUseId":"two","content":[{"text":"second"}]}},
            {"text":"summarize"}
        ])
    );
}

#[test]
fn bedrock_filter_and_malformed_stop_reasons_keep_their_outcomes() {
    for stop in ["guardrail_intervened", "content_filtered"] {
        let server = FakeServer::start(vec![Reply::Binary(frame(
            "messageStop",
            json!({"stopReason":stop}),
        ))]);
        let result = provider(&server)
            .chat(&request(), &mut |_| {}, &CancelToken::new())
            .unwrap();
        assert_eq!(result.finish_reason, FinishReason::ContentFilter, "{stop}");
    }
    for stop in [
        "malformed_model_output",
        "malformed_tool_use",
        "unknown-private-prompt",
    ] {
        let server = FakeServer::start(vec![Reply::Binary(frame(
            "messageStop",
            json!({"stopReason":stop}),
        ))]);
        let mut events = Vec::new();
        let error = provider(&server)
            .chat(
                &request(),
                &mut |e| events.push(e.clone()),
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(error.kind(), ProviderErrorKind::Protocol);
        assert!(!error.to_string().contains(stop));
        assert!(!events.iter().any(|e| matches!(e, StreamEvent::Done { .. })));
    }
}
