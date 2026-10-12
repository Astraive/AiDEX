//! Vendor model listings: pagination, vendor metadata, and the known-family
//! hint pass, against loopback fixtures (no external network).

mod support;

use std::sync::Arc;

use aidex_providers::{
    AuthConfig, CapabilitySource, CostHint, ModelDescriptor, ModelProvider, NoVault, OfflineMode,
    OpenAiCompatibleProvider, ProviderConfig, ProviderId, ProviderKind, ProviderRegistry,
    SecretBytes, StaticCredentials, ToolCallSupport,
};
use serde_json::json;
use support::{FakeServer, Reply, sentinel_secret};

/// A cloud-protocol account on the fixture, authenticated with `secret`.
fn cloud(kind: ProviderKind, server: &FakeServer, secret: &str) -> Arc<dyn ModelProvider> {
    let id = ProviderId::new("account").unwrap();
    let mut config = ProviderConfig::new(id.clone(), kind, server.base_url());
    config.auth = AuthConfig::VaultSecret {
        secret_name: "key".into(),
    };
    let keys = Arc::new(StaticCredentials::new());
    keys.insert(id.clone(), "key", SecretBytes::from(secret.to_owned()));
    let registry = ProviderRegistry::in_memory(keys, Arc::new(OfflineMode::new(false)));
    registry.add_provider(config).unwrap();
    registry.provider(&id).unwrap()
}

/// A key-less OpenAI-compatible endpoint on the fixture.
fn compatible(server: &FakeServer) -> OpenAiCompatibleProvider {
    let config = ProviderConfig::new(
        ProviderId::new("gateway").unwrap(),
        ProviderKind::OpenaiCompatible,
        server.base_url(),
    );
    OpenAiCompatibleProvider::from_config(
        &config,
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    )
    .unwrap()
}

fn find<'a>(models: &'a [ModelDescriptor], id: &str) -> &'a ModelDescriptor {
    models
        .iter()
        .find(|model| model.model.model_id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is listed"))
}

#[test]
fn anthropic_listing_follows_pages_and_fills_known_family_limits() {
    let server = FakeServer::start(vec![
        Reply::Json(
            200,
            json!({
                "data": [{"type": "model", "id": "claude-sonnet-4-5", "display_name": "Claude Sonnet 4.5", "created_at": "2025-09-29T00:00:00Z"}],
                "has_more": true,
                "first_id": "claude-sonnet-4-5",
                "last_id": "claude-sonnet-4-5"
            })
            .to_string(),
        ),
        Reply::Json(
            200,
            json!({
                "data": [{"type": "model", "id": "claude-3-haiku-20240307", "display_name": "Claude Haiku 3"}],
                "has_more": false,
                "first_id": "claude-3-haiku-20240307",
                "last_id": "claude-3-haiku-20240307"
            })
            .to_string(),
        ),
    ]);
    let secret = sentinel_secret();
    let models = cloud(ProviderKind::Anthropic, &server, &secret)
        .models()
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/models?limit=1000");
    assert_eq!(
        requests[1].path,
        "/v1/models?limit=1000&after_id=claude-sonnet-4-5"
    );
    for request in &requests {
        assert_eq!(request.header("x-api-key"), Some(secret.as_str()));
        assert_eq!(request.header("anthropic-version"), Some("2023-06-01"));
        assert!(request.header("authorization").is_none());
        let raw = String::from_utf8_lossy(&request.raw);
        assert_eq!(
            raw.matches(secret.as_str()).count(),
            1,
            "only the key header"
        );
    }
    let names: Vec<_> = models
        .iter()
        .map(|model| (model.model.model_id.as_str(), model.display_name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("claude-sonnet-4-5", "Claude Sonnet 4.5"),
            ("claude-3-haiku-20240307", "Claude Haiku 3")
        ]
    );
    for model in &models {
        assert_eq!(model.source, CapabilitySource::Catalog, "{model:?}");
        assert_eq!(model.context_window, 200_000);
        assert_eq!(model.capabilities.tool_calls, ToolCallSupport::Native);
        assert!(model.capabilities.vision && model.capabilities.chat);
        assert!(!format!("{model:?}").contains(&secret));
    }
}

#[test]
fn anthropic_pagination_stops_after_ten_pages() {
    let pages: Vec<Reply> = (0..12)
        .map(|page| {
            Reply::Json(
                200,
                json!({
                    "data": [{"type": "model", "id": format!("claude-opus-4-{page}")}],
                    "has_more": true,
                    "last_id": format!("claude-opus-4-{page}")
                })
                .to_string(),
            )
        })
        .collect();
    let server = FakeServer::start(pages);
    let models = cloud(ProviderKind::Anthropic, &server, "fixture-key")
        .models()
        .unwrap();
    assert_eq!(server.requests().len(), 10);
    assert_eq!(models.len(), 10);
}

#[test]
fn gemini_listing_pages_filters_generation_methods_and_reads_limits() {
    let server = FakeServer::start(vec![
        Reply::Json(
            200,
            json!({
                "models": [
                    {
                        "name": "models/gemini-2.5-pro",
                        "displayName": "Gemini 2.5 Pro",
                        "inputTokenLimit": 1_048_576,
                        "outputTokenLimit": 65_536,
                        "supportedGenerationMethods": ["generateContent", "countTokens"]
                    },
                    {
                        "name": "models/gemini-embedding-001",
                        "displayName": "Gemini Embedding 001",
                        "inputTokenLimit": 2048,
                        "supportedGenerationMethods": ["embedContent", "countTextTokens"]
                    },
                    {"name": "models/aqa", "supportedGenerationMethods": ["generateAnswer"]},
                    {"name": "models/imagen-4.0-generate-001", "supportedGenerationMethods": ["predict"]},
                    {"name": "models/no-methods"}
                ],
                "nextPageToken": "page/2="
            })
            .to_string(),
        ),
        Reply::Json(
            200,
            json!({
                "models": [{
                    "name": "models/gemini-2.0-flash",
                    "displayName": "Gemini 2.0 Flash",
                    "inputTokenLimit": 1_048_576,
                    "outputTokenLimit": 8192,
                    "supportedGenerationMethods": ["generateContent"]
                }]
            })
            .to_string(),
        ),
    ]);
    let models = cloud(ProviderKind::Gemini, &server, "fixture-key")
        .models()
        .unwrap();
    let requests = server.requests();
    assert_eq!(requests[0].path, "/v1/models?pageSize=1000");
    assert_eq!(
        requests[1].path,
        "/v1/models?pageSize=1000&pageToken=page%2F2%3D"
    );
    assert_eq!(requests[0].header("x-goog-api-key"), Some("fixture-key"));
    let ids: Vec<_> = models
        .iter()
        .map(|model| model.model.model_id.as_str())
        .collect();
    assert_eq!(
        ids,
        ["gemini-2.5-pro", "gemini-embedding-001", "gemini-2.0-flash"]
    );
    let pro = find(&models, "gemini-2.5-pro");
    assert_eq!(pro.display_name, "Gemini 2.5 Pro");
    assert_eq!(pro.context_window, 1_048_576);
    assert_eq!(pro.max_output_tokens, Some(65_536));
    assert_eq!(pro.source, CapabilitySource::Discovered);
    assert_eq!(pro.capabilities.tool_calls, ToolCallSupport::Native);
    assert!(pro.capabilities.json_schema_output && pro.capabilities.vision);
    let embedding = find(&models, "gemini-embedding-001");
    assert!(embedding.capabilities.embeddings && !embedding.capabilities.chat);
    assert_eq!(embedding.capabilities.tool_calls, ToolCallSupport::None);
}

#[test]
fn openrouter_metadata_becomes_capabilities_limits_and_cost() {
    let server = FakeServer::start(vec![Reply::Json(
        200,
        json!({"data": [
            {
                "id": "anthropic/claude-3.5-sonnet",
                "name": "Anthropic: Claude 3.5 Sonnet",
                "context_length": 200_000,
                "architecture": {"input_modalities": ["text", "image"], "output_modalities": ["text"]},
                "pricing": {"prompt": "0.000003", "completion": "0.000015"},
                "top_provider": {"context_length": 200_000, "max_completion_tokens": 8192},
                "supported_parameters": ["tools", "tool_choice", "max_tokens", "structured_outputs"]
            },
            {
                "id": "meta-llama/llama-3-8b-instruct",
                "name": "Meta: Llama 3 8B Instruct",
                "context_length": 8192,
                "architecture": {"input_modalities": ["text"]},
                "pricing": {"prompt": "0.00000003", "completion": "0.00000006"},
                "supported_parameters": ["max_tokens", "temperature"]
            },
            {
                "id": "openrouter/auto",
                "name": "Auto Router",
                "context_length": 2_000_000,
                "pricing": {"prompt": "-1", "completion": "-1"},
                "supported_parameters": "malformed"
            },
            {"id": "x-ai/grok-4", "name": "xAI: Grok 4", "top_provider": {"context_length": 256_000}}
        ]})
        .to_string(),
    )]);
    let models = compatible(&server).models().unwrap();
    let sonnet = find(&models, "anthropic/claude-3.5-sonnet");
    assert_eq!(sonnet.display_name, "Anthropic: Claude 3.5 Sonnet");
    assert_eq!(sonnet.context_window, 200_000);
    assert_eq!(sonnet.max_output_tokens, Some(8192));
    assert_eq!(sonnet.source, CapabilitySource::Discovered);
    assert_eq!(sonnet.capabilities.tool_calls, ToolCallSupport::Native);
    assert!(sonnet.capabilities.vision && sonnet.capabilities.json_schema_output);
    assert_eq!(
        sonnet.cost,
        Some(CostHint {
            input_per_million: 3.0,
            output_per_million: 15.0
        })
    );
    let llama = find(&models, "meta-llama/llama-3-8b-instruct");
    assert_eq!(llama.capabilities.tool_calls, ToolCallSupport::None);
    assert!(!llama.capabilities.vision && !llama.capabilities.json_schema_output);
    assert_eq!(
        llama.cost,
        Some(CostHint {
            input_per_million: 0.03,
            output_per_million: 0.06
        })
    );
    let auto = find(&models, "openrouter/auto");
    assert_eq!(auto.cost, None, "negative prices are ignored");
    assert_eq!(auto.context_window, 2_000_000);
    let grok = find(&models, "x-ai/grok-4");
    assert_eq!(grok.context_window, 256_000, "top_provider context is read");
    assert_eq!(
        grok.capabilities.tool_calls,
        ToolCallSupport::Native,
        "unreported tools come from the family hint"
    );
    assert_eq!(grok.source, CapabilitySource::Discovered);
}

#[test]
fn together_top_level_arrays_and_model_types_are_understood() {
    let server = FakeServer::start(vec![Reply::Json(
        200,
        json!([
            {
                "id": "meta-llama/Llama-3.3-70B-Instruct-Turbo",
                "object": "model",
                "type": "chat",
                "display_name": "Meta Llama 3.3 70B Instruct Turbo",
                "context_length": 131_072,
                "pricing": {"input": 0.88, "output": 0.88}
            },
            {"id": "BAAI/bge-large-en-v1.5", "type": "embedding", "context_length": 512},
            {"id": "black-forest-labs/FLUX.1-schnell", "type": "image"}
        ])
        .to_string(),
    )]);
    let models = compatible(&server).models().unwrap();
    assert_eq!(models.len(), 3);
    let llama = find(&models, "meta-llama/Llama-3.3-70B-Instruct-Turbo");
    assert_eq!(llama.display_name, "Meta Llama 3.3 70B Instruct Turbo");
    assert_eq!(llama.context_window, 131_072);
    assert!(llama.capabilities.chat);
    let embedding = find(&models, "BAAI/bge-large-en-v1.5");
    assert!(embedding.capabilities.embeddings && !embedding.capabilities.chat);
    assert!(
        !find(&models, "black-forest-labs/FLUX.1-schnell")
            .capabilities
            .chat
    );
}

#[test]
fn mistral_capabilities_report_tools_vision_and_embeddings() {
    let server = FakeServer::start(vec![Reply::Json(
        200,
        json!({"object": "list", "data": [
            {
                "id": "mistral-large-latest",
                "name": "mistral-large-2411",
                "max_context_length": 131_072,
                "capabilities": {"completion_chat": true, "function_calling": true, "vision": false}
            },
            {
                "id": "pixtral-large-latest",
                "max_context_length": 131_072,
                "capabilities": {"completion_chat": true, "function_calling": true, "vision": true}
            },
            {
                "id": "open-mistral-nemo",
                "max_context_length": 131_072,
                "capabilities": {"completion_chat": true, "function_calling": false}
            },
            {
                "id": "mistral-embed",
                "max_context_length": 8192,
                "capabilities": {"completion_chat": false, "function_calling": false}
            }
        ]})
        .to_string(),
    )]);
    let models = compatible(&server).models().unwrap();
    let large = find(&models, "mistral-large-latest");
    assert_eq!(large.display_name, "mistral-large-2411");
    assert_eq!(large.context_window, 131_072);
    assert_eq!(large.source, CapabilitySource::Discovered);
    assert_eq!(large.capabilities.tool_calls, ToolCallSupport::Native);
    assert!(!large.capabilities.vision);
    assert!(find(&models, "pixtral-large-latest").capabilities.vision);
    assert_eq!(
        find(&models, "open-mistral-nemo").capabilities.tool_calls,
        ToolCallSupport::None
    );
    let embed = find(&models, "mistral-embed");
    assert!(embed.capabilities.embeddings && !embed.capabilities.chat);
}

#[test]
fn openai_non_chat_families_are_not_chat_models() {
    let ids = [
        "gpt-4o",
        "o3-mini",
        "codex-mini-latest",
        "text-embedding-3-small",
        "dall-e-3",
        "whisper-1",
        "tts-1",
        "gpt-4o-realtime-preview",
        "gpt-4o-mini-transcribe",
        "gpt-image-1",
        "omni-moderation-latest",
    ];
    let data: Vec<_> = ids
        .iter()
        .map(|id| json!({"id": id, "object": "model", "owned_by": "system"}))
        .collect();
    let server = FakeServer::start(vec![Reply::Json(
        200,
        json!({"object": "list", "data": data}).to_string(),
    )]);
    let models = compatible(&server).models().unwrap();
    let gpt = find(&models, "gpt-4o");
    assert_eq!(gpt.source, CapabilitySource::Catalog);
    assert_eq!(gpt.context_window, 128_000);
    assert_eq!(gpt.capabilities.tool_calls, ToolCallSupport::Native);
    assert!(gpt.capabilities.json_schema_output && gpt.capabilities.vision);
    assert_eq!(find(&models, "o3-mini").source, CapabilitySource::Catalog);
    let codex = find(&models, "codex-mini-latest");
    assert!(codex.capabilities.chat);
    assert_eq!(codex.source, CapabilitySource::Assumed);
    let embedding = find(&models, "text-embedding-3-small");
    assert!(embedding.capabilities.embeddings && !embedding.capabilities.chat);
    for id in &ids[4..] {
        let model = find(&models, id);
        assert!(!model.capabilities.chat, "{id}");
        assert_eq!(model.source, CapabilitySource::Assumed, "{id}");
    }
}

#[test]
fn inactive_groq_models_are_skipped_and_output_limits_read() {
    let server = FakeServer::start(vec![Reply::Json(
        200,
        json!({"object": "list", "data": [
            {"id": "llama-3.3-70b-versatile", "context_window": 131_072, "max_completion_tokens": 32_768, "active": true},
            {"id": "retired-model", "context_window": 8192, "active": false}
        ]})
        .to_string(),
    )]);
    let models = compatible(&server).models().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].max_output_tokens, Some(32_768));
    assert_eq!(models[0].context_window, 131_072);
}

#[test]
fn responses_api_key_listings_use_the_hint_pass() {
    let server = FakeServer::start(vec![Reply::Json(
        200,
        json!({"object": "list", "data": [{"id": "gpt-5"}, {"id": "dall-e-3"}]}).to_string(),
    )]);
    let models = cloud(ProviderKind::OpenaiResponses, &server, "fixture-key")
        .models()
        .unwrap();
    assert_eq!(server.requests()[0].path, "/v1/models");
    let gpt = find(&models, "gpt-5");
    assert_eq!(gpt.source, CapabilitySource::Catalog);
    assert_eq!(gpt.capabilities.tool_calls, ToolCallSupport::Native);
    assert!(!find(&models, "dall-e-3").capabilities.chat);
}
