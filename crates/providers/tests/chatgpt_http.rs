//! Public Responses boundaries used by Sign in with ChatGPT plan tokens.
mod support;

use aidex_providers::*;
use serde_json::json;
use std::sync::Arc;
use support::{FakeServer, Reply};

fn provider(server: &FakeServer) -> Arc<dyn ModelProvider> {
    // A loopback HTTP fixture exercises the same bearer transport without
    // relaxing production binding of reserved OAuth secrets to the API root.
    let mut config = ProviderConfig::new(
        ProviderId::new("chatgpt-fixture").unwrap(),
        ProviderKind::OpenaiResponses,
        server.base_url(),
    );
    config.auth = AuthConfig::VaultSecret {
        secret_name: "fixture-key".into(),
    };
    let keys = Arc::new(StaticCredentials::new());
    keys.insert(
        config.id.clone(),
        "fixture-key",
        SecretBytes::from("fixture-token".to_owned()),
    );
    let registry = ProviderRegistry::in_memory(keys, Arc::new(OfflineMode::new(false)));
    registry.add_provider(config.clone()).unwrap();
    registry.provider(&config.id).unwrap()
}

#[test]
fn chatgpt_catalog_uses_visible_slugs_labels_and_server_order() {
    let server = FakeServer::start(vec![Reply::Json(
        200,
        json!({"models":[
            {"slug":"gpt-6.1-sol","display_name":"GPT 6.1 Sol","visibility":"list"},
            {"slug":"hidden-fixture","display_name":"Hidden","visibility":"hidden"},
            {"slug":"gpt-6-sol","display_name":"GPT 6 Sol","visibility":"list"}
        ]})
        .to_string(),
    )]);
    let models = provider(&server).models().unwrap();
    assert_eq!(
        models
            .iter()
            .map(|m| m.model.model_id.as_str())
            .collect::<Vec<_>>(),
        ["gpt-6.1-sol", "gpt-6-sol"]
    );
    assert_eq!(models[0].display_name, "GPT 6.1 Sol");
    assert_eq!(models[1].display_name, "GPT 6 Sol");
    assert_eq!(
        models[0].source,
        CapabilitySource::Catalog,
        "a visible catalog entry reports no limits; only the known-family table supplies them"
    );
    assert_eq!(models[0].context_window, 128_000);
    let sent = &server.requests()[0];
    assert_eq!(sent.path, "/v1/models");
    assert_eq!(sent.header("authorization"), Some("Bearer fixture-token"));
}

#[test]
fn responses_chat_disables_storage_and_streams_the_selected_slug() {
    let server = FakeServer::start(vec![Reply::sse(&[
        r#"{"type":"response.output_text.delta","delta":"Hello, world!"}"#,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    ])]);
    let request = ChatRequest::new(
        ModelId::new("gpt-6.1-sol").unwrap(),
        vec![ChatMessage::user("Say exactly: Hello, world!")],
    );
    let response = provider(&server)
        .chat(&request, &mut |_| {}, &CancelToken::new())
        .unwrap();
    assert_eq!(response.message.content, "Hello, world!");
    let sent = &server.requests()[0];
    assert_eq!(sent.path, "/v1/responses");
    assert_eq!(sent.header("authorization"), Some("Bearer fixture-token"));
    let body = sent.json();
    assert_eq!(body["model"], "gpt-6.1-sol");
    assert_eq!(
        body["store"], false,
        "ChatGPT plan usage requires store:false"
    );
    assert_eq!(body["stream"], true);
}

#[test]
fn chatgpt_plan_failures_keep_status_and_code_without_billing_fallback() {
    for (status, code) in [
        (403, "subscription_sharing_user_not_eligible"),
        (429, "subscription_sharing_usage_limit_exceeded"),
        (503, "subscription_sharing_usage_unavailable"),
    ] {
        for streaming in [false, true] {
            let error =
                json!({"code":code,"param":"model","message":"fixture-token private-prompt"});
            let reply = if streaming {
                let event =
                    json!({"type":"response.failed","response":{"status":"failed","error":error}})
                        .to_string();
                Reply::sse(&[&event])
            } else {
                Reply::Json(status, json!({"error":error}).to_string())
            };
            let server = FakeServer::start(vec![reply]);
            let mut events = Vec::new();
            let request = ChatRequest::new(
                ModelId::new("gpt-6.1-sol").unwrap(),
                vec![ChatMessage::user("hello")],
            );
            let failure = provider(&server)
                .chat(
                    &request,
                    &mut |event| events.push(event.clone()),
                    &CancelToken::new(),
                )
                .unwrap_err();
            assert_eq!(
                failure,
                ProviderError::Http {
                    status,
                    code: Some(code.into())
                }
            );
            assert!(
                !failure.is_retryable(),
                "ChatGPT plan failure must not automatically select another billing path"
            );
            assert!(!failure.to_string().contains("fixture-token"));
            assert!(!failure.to_string().contains("private-prompt"));
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, StreamEvent::Done { .. }))
            );
        }
    }
}

#[test]
fn responses_maps_native_system_context_to_developer_messages() {
    let server = FakeServer::start(vec![Reply::sse(&[
        r#"{"type":"response.output_text.delta","delta":"ok"}"#,
        r#"{"type":"response.completed","response":{"status":"completed"}}"#,
    ])]);
    let request = ChatRequest::new(
        ModelId::new("gpt-6.1-sol").unwrap(),
        vec![
            ChatMessage::system("Be helpful."),
            ChatMessage::user("Hello"),
        ],
    );
    provider(&server)
        .chat(&request, &mut |_| {}, &CancelToken::new())
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&server.requests()[0].body).unwrap();
    assert_eq!(body["input"][0]["role"], "developer");
    assert_eq!(body["input"][0]["content"], "Be helpful.");
}
