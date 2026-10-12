//! Routing and fallback with deterministic fake providers (no network).

use std::sync::Arc;

use aidex_providers::{
    CancelToken, CapabilitySource, ChatMessage, ChatRequest, FakeProvider, FakeScript, FakeTurn,
    FallbackEvent, Locality, ModelDescriptor, ModelId, ModelProvider, ModelRef, ModelRole, NoVault,
    OfflineMode, ProviderError, ProviderErrorKind, ProviderId, ProviderRegistry, ToolCallSupport,
};
use aidex_router::{
    HostSummary, NetworkState, PrivacyMode, RejectReason, RemoteBlock, RouteError, RouteRequest,
    Router, TaskKind, run_with_fallback,
};

fn model(reference: &str, locality: Locality, context: u32) -> ModelDescriptor {
    ModelDescriptor::new(ModelRef::parse(reference).unwrap(), locality, context)
}

/// A local coder, a local general model, and a large cloud model.
fn catalog() -> Vec<ModelDescriptor> {
    vec![
        model("local/coder", Locality::Local, 32_768)
            .with_roles([ModelRole::Code])
            .with_tool_calls(ToolCallSupport::Structured)
            .with_json_schema_output(true),
        model("local/small", Locality::Local, 4096),
        model("cloud/big", Locality::Remote, 200_000)
            .with_roles([ModelRole::Code, ModelRole::Plan])
            .with_tool_calls(ToolCallSupport::Native)
            .with_json_schema_output(true),
    ]
}

fn route(request: &RouteRequest) -> Result<aidex_router::RouteDecision, RouteError> {
    Router::new().route(request, &catalog())
}

fn names(decision: &aidex_router::RouteDecision) -> Vec<String> {
    decision.candidates().map(ToString::to_string).collect()
}

#[test]
fn privacy_offline_and_secrets_never_route_to_cloud() {
    let needs_cloud = RouteRequest::new(TaskKind::Plan).with_required_context(100_000);
    assert_eq!(names(&route(&needs_cloud).unwrap()), ["cloud/big"]);
    for (request, cause) in [
        (
            needs_cloud.clone().with_privacy(PrivacyMode::LocalOnly),
            RemoteBlock::LocalOnly,
        ),
        (
            needs_cloud.clone().with_privacy(PrivacyMode::AirGapped),
            RemoteBlock::AirGapped,
        ),
        (
            needs_cloud.clone().with_network(NetworkState::Offline),
            RemoteBlock::Offline,
        ),
        (needs_cloud.clone().secret_heavy(), RemoteBlock::SecretHeavy),
        (
            needs_cloud
                .clone()
                .with_privacy(PrivacyMode::CloudPreferred)
                .with_network(NetworkState::Offline),
            RemoteBlock::Offline,
        ),
    ] {
        let error = route(&request).unwrap_err();
        let RouteError::NoEligibleModel { rejected, reasons } = &error else {
            panic!("expected no eligible model, got {error:?}");
        };
        let cloud = rejected
            .iter()
            .find(|r| r.model.to_string() == "cloud/big")
            .unwrap();
        assert_eq!(cloud.reason, RejectReason::RemoteForbidden { cause });
        assert!(
            reasons.iter().any(|reason| reason == cause.describe()),
            "{reasons:?}"
        );

        // With a request that fits locally, every candidate is local.
        let mut local_fit = request.clone();
        local_fit.required_context = 1000;
        let decision = route(&local_fit).unwrap();
        assert!(!decision.allow_remote);
        assert!(
            names(&decision)
                .iter()
                .all(|name| name.starts_with("local/")),
            "{decision:?}"
        );
    }
}

#[test]
fn context_tool_and_schema_requirements_filter_candidates() {
    let decision = route(&RouteRequest::new(TaskKind::Code).with_required_context(8000)).unwrap();
    assert_eq!(names(&decision), ["local/coder", "cloud/big"]);
    let small = decision
        .rejected
        .iter()
        .find(|r| r.model.to_string() == "local/small")
        .unwrap();
    assert_eq!(
        small.reason,
        RejectReason::ContextTooSmall {
            window: 4096,
            required: 8000
        }
    );

    let tools = route(&RouteRequest::new(TaskKind::Chat).needing_tools()).unwrap();
    assert!(!names(&tools).contains(&"local/small".to_owned()));
    assert!(
        tools
            .rejected
            .iter()
            .any(|r| r.reason == RejectReason::NoToolSupport)
    );

    let schema = route(&RouteRequest::new(TaskKind::Classify).needing_json_schema()).unwrap();
    assert!(
        schema
            .rejected
            .iter()
            .any(|r| r.reason == RejectReason::NoJsonSchema)
    );

    let embed = route(&RouteRequest::new(TaskKind::Embed)).unwrap_err();
    assert!(
        embed
            .rejected()
            .iter()
            .all(|r| matches!(r.reason, RejectReason::WrongCapability { .. }))
    );
}

#[test]
fn ranking_follows_privacy_role_tools_and_preference() {
    // Hybrid: local first, role match before general.
    assert_eq!(
        names(&route(&RouteRequest::new(TaskKind::Code)).unwrap()),
        ["local/coder", "local/small", "cloud/big"]
    );
    // Cloud preferred: remote first, local fallback.
    assert_eq!(
        names(
            &route(&RouteRequest::new(TaskKind::Code).with_privacy(PrivacyMode::CloudPreferred))
                .unwrap()
        ),
        ["cloud/big", "local/coder", "local/small"]
    );
    // Chat prefers the general role among local models.
    assert_eq!(
        names(&route(&RouteRequest::new(TaskKind::Chat)).unwrap())[0],
        "local/small"
    );
    // A preferred (default) model wins when eligible and is explained when not.
    let preferred = route(
        &RouteRequest::new(TaskKind::Code).with_preferred(ModelRef::parse("cloud/big").unwrap()),
    )
    .unwrap();
    assert_eq!(preferred.primary.to_string(), "cloud/big");
    let skipped = route(
        &RouteRequest::new(TaskKind::Code)
            .with_preferred(ModelRef::parse("cloud/big").unwrap())
            .with_privacy(PrivacyMode::LocalOnly),
    )
    .unwrap();
    assert_eq!(skipped.primary.to_string(), "local/coder");
    assert!(
        skipped
            .reasons
            .iter()
            .any(|r| r.starts_with("preferred model cloud/big skipped"))
    );
    assert_eq!(
        Router::new()
            .with_max_fallbacks(1)
            .route(&RouteRequest::new(TaskKind::Code), &catalog())
            .unwrap()
            .fallbacks
            .len(),
        1
    );
}

#[test]
fn host_memory_and_unverified_limits_are_hard_constraints() {
    let mut models = catalog();
    models[0] = models[0].clone().with_min_ram_bytes(16 << 30);
    models.push(
        model("cloud/mystery", Locality::Remote, 4096).with_source(CapabilitySource::Assumed),
    );
    let request = RouteRequest::new(TaskKind::Code).with_host(HostSummary {
        ram_bytes: Some(8 << 30),
        gpu: false,
    });
    let decision = Router::new().route(&request, &models).unwrap();
    assert_eq!(decision.primary.to_string(), "local/small");
    let reasons: Vec<_> = decision
        .rejected
        .iter()
        .map(|r| (r.model.to_string(), r.reason.clone()))
        .collect();
    assert!(reasons.contains(&(
        "local/coder".into(),
        RejectReason::InsufficientMemory {
            required: 16 << 30,
            available: 8 << 30
        }
    )));
    assert!(reasons.contains(&("cloud/mystery".into(), RejectReason::UnverifiedCapabilities)));
    // An explicit choice may use a model with assumed limits.
    let explicit = Router::new()
        .route(
            &request
                .clone()
                .with_override(ModelRef::parse("cloud/mystery").unwrap()),
            &models,
        )
        .unwrap();
    assert_eq!(explicit.primary.to_string(), "cloud/mystery");
}

#[test]
fn catalog_limits_are_eligible_and_rank_after_discovered_ones() {
    let models = vec![
        model("cloud/catalog", Locality::Remote, 200_000)
            .with_tool_calls(ToolCallSupport::Native)
            .with_source(CapabilitySource::Catalog),
        model("cloud/discovered", Locality::Remote, 200_000)
            .with_tool_calls(ToolCallSupport::Native)
            .with_source(CapabilitySource::Discovered),
        model("cloud/configured", Locality::Remote, 200_000)
            .with_tool_calls(ToolCallSupport::Native)
            .with_source(CapabilitySource::Configured),
        model("cloud/assumed", Locality::Remote, 200_000)
            .with_tool_calls(ToolCallSupport::Native)
            .with_source(CapabilitySource::Assumed),
    ];
    let request = RouteRequest::new(TaskKind::Code)
        .needing_tools()
        .with_required_context(100_000);
    let decision = Router::new()
        .with_max_fallbacks(3)
        .route(&request, &models)
        .unwrap();
    assert_eq!(
        names(&decision),
        ["cloud/configured", "cloud/discovered", "cloud/catalog"]
    );
    assert!(
        decision
            .rejected
            .iter()
            .any(|r| r.model.to_string() == "cloud/assumed"
                && r.reason == RejectReason::UnverifiedCapabilities)
    );
    let only_catalog = Router::new().route(&request, &models[..1]).unwrap();
    assert_eq!(only_catalog.primary.to_string(), "cloud/catalog");
}

#[test]
fn user_overrides_are_honoured_or_refused_explicitly() {
    let chosen = route(
        &RouteRequest::new(TaskKind::Code).with_override(ModelRef::parse("local/small").unwrap()),
    )
    .unwrap();
    assert_eq!(chosen.primary.to_string(), "local/small");
    assert!(
        chosen.fallbacks.is_empty(),
        "explicit choices are not silently swapped"
    );
    let offline = route(
        &RouteRequest::new(TaskKind::Code)
            .with_network(NetworkState::Offline)
            .with_override(ModelRef::parse("cloud/big").unwrap()),
    )
    .unwrap_err();
    let RouteError::OverrideRejected { rejection } = &offline else {
        panic!("{offline:?}");
    };
    assert_eq!(
        rejection.reason,
        RejectReason::RemoteForbidden {
            cause: RemoteBlock::Offline
        }
    );
    assert!(offline.to_string().contains("cloud/big"));
    let too_small = route(
        &RouteRequest::new(TaskKind::Code)
            .with_required_context(10_000)
            .with_override(ModelRef::parse("local/small").unwrap()),
    )
    .unwrap_err();
    assert!(matches!(too_small, RouteError::OverrideRejected { .. }));
    assert!(matches!(
        route(&RouteRequest::new(TaskKind::Code).with_override(ModelRef::parse("x/y").unwrap())),
        Err(RouteError::OverrideNotFound { .. })
    ));
}

#[test]
fn decisions_are_inspectable_as_json() {
    let decision = route(&RouteRequest::new(TaskKind::Code).with_required_context(8000)).unwrap();
    let json = serde_json::to_value(&decision).unwrap();
    assert_eq!(json["primary"]["provider_id"], "local");
    assert_eq!(json["rejected"][0]["reason"], "context_too_small");
    assert_eq!(json["rejected"][0]["window"], 4096);
    assert!(
        json["rejected"][0]["explanation"]
            .as_str()
            .unwrap()
            .contains("4096")
    );
    assert!(
        json["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().starts_with("selected local/coder"))
    );
    let back: aidex_router::RouteDecision = serde_json::from_value(json).unwrap();
    assert_eq!(back, decision);
}

/// Fake providers registered in a registry, keyed by provider id.
fn registry(providers: Vec<FakeProvider>) -> ProviderRegistry {
    let registry = ProviderRegistry::in_memory(
        Arc::new(NoVault::default()),
        Arc::new(OfflineMode::new(false)),
    );
    for provider in providers {
        registry.register(Box::new(provider)).unwrap();
    }
    registry
}

fn fake(id: &str, locality: Locality, script: FakeScript) -> FakeProvider {
    let provider = ProviderId::new(id).unwrap();
    FakeProvider::new(provider.clone(), locality)
        .with_model(
            ModelDescriptor::new(
                ModelRef::new(provider, ModelId::new("m").unwrap()),
                locality,
                8192,
            )
            .with_roles([ModelRole::Code]),
        )
        .with_script(script)
}

fn chat() -> ChatRequest {
    ChatRequest::new(
        ModelId::new("placeholder").unwrap(),
        vec![ChatMessage::user("fix the bug")],
    )
}

#[test]
fn the_fake_provider_runs_through_the_router_without_network() {
    let registry = registry(vec![fake(
        "local",
        Locality::Local,
        FakeScript::new().then(FakeTurn::text("patched")),
    )]);
    let listing = registry.models();
    let decision = Router::new()
        .route(&RouteRequest::new(TaskKind::Code), &listing.models)
        .unwrap();
    let mut text = String::new();
    let mut fallbacks = Vec::new();
    let (response, used) = run_with_fallback(
        &decision,
        &registry,
        &chat(),
        &mut |event| {
            if let aidex_providers::StreamEvent::TextDelta { text: delta } = event {
                text.push_str(delta);
            }
        },
        &CancelToken::new(),
        &mut |event: &FallbackEvent| fallbacks.push(event.clone()),
    )
    .unwrap();
    assert_eq!(used.to_string(), "local/m");
    assert_eq!(
        (response.message.content.as_str(), text.as_str()),
        ("patched", "patched")
    );
    assert!(fallbacks.is_empty());
}

#[test]
fn retryable_failures_fall_back_with_events_and_others_do_not() {
    let primary = fake(
        "first",
        Locality::Local,
        FakeScript::new()
            .then(FakeTurn::error(ProviderError::unavailable(
                "server restarting",
            )))
            .then(FakeTurn::error(ProviderError::Unauthorized)),
    );
    let secondary = fake(
        "second",
        Locality::Local,
        FakeScript::new().then(FakeTurn::text("from second")),
    );
    let registry = registry(vec![primary, secondary]);
    let decision = Router::new()
        .route(
            &RouteRequest::new(TaskKind::Code),
            &registry.models().models,
        )
        .unwrap();
    assert_eq!(names(&decision), ["first/m", "second/m"]);

    let mut fallbacks = Vec::new();
    let (response, used) = run_with_fallback(
        &decision,
        &registry,
        &chat(),
        &mut |_| {},
        &CancelToken::new(),
        &mut |event| fallbacks.push(event.clone()),
    )
    .unwrap();
    assert_eq!(used.to_string(), "second/m");
    assert_eq!(response.message.content, "from second");
    assert_eq!(fallbacks.len(), 1);
    assert_eq!(fallbacks[0].from.to_string(), "first/m");
    assert_eq!(fallbacks[0].to.to_string(), "second/m");
    assert_eq!(fallbacks[0].reason, ProviderErrorKind::Unavailable);
    assert!(fallbacks[0].message.contains("server restarting"));
    let event =
        serde_json::to_value(aidex_providers::ModelEvent::Fallback(fallbacks[0].clone())).unwrap();
    assert_eq!(event["kind"], "model.fallback");

    // A non-retryable error is returned, not hidden behind a fallback.
    let mut fallbacks = Vec::new();
    let error = run_with_fallback(
        &decision,
        &registry,
        &chat(),
        &mut |_| {},
        &CancelToken::new(),
        &mut |event| fallbacks.push(event.clone()),
    )
    .unwrap_err();
    assert_eq!(error, ProviderError::Unauthorized);
    assert!(fallbacks.is_empty());

    // Cancellation stops the chain.
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        run_with_fallback(
            &decision,
            &registry,
            &chat(),
            &mut |_| {},
            &cancel,
            &mut |_| {}
        )
        .unwrap_err(),
        ProviderError::Cancelled
    );
}

#[test]
fn fallback_never_moves_from_local_to_cloud_when_privacy_forbids_it() {
    let local = fake(
        "local",
        Locality::Local,
        FakeScript::new()
            .then(FakeTurn::error(ProviderError::unavailable("model crashed")))
            .then(FakeTurn::error(ProviderError::unavailable(
                "model crashed again",
            ))),
    );
    let cloud = Arc::new(fake(
        "cloud",
        Locality::Remote,
        FakeScript::new().then(FakeTurn::text("leaked")),
    ));
    let registry = registry(vec![local]);
    registry.register_shared(cloud.clone()).unwrap();
    let decision = Router::new()
        .route(
            &RouteRequest::new(TaskKind::Code).with_privacy(PrivacyMode::LocalOnly),
            &registry.models().models,
        )
        .unwrap();
    assert_eq!(names(&decision), ["local/m"]);
    let error = run_with_fallback(
        &decision,
        &registry,
        &chat(),
        &mut |_| {},
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(error, ProviderError::Unavailable { .. }));

    // Even a tampered decision listing a cloud fallback never reaches the cloud.
    let mut tampered = decision.clone();
    tampered.fallbacks.push(ModelRef::parse("cloud/m").unwrap());
    let mut fallbacks = Vec::new();
    let error = run_with_fallback(
        &tampered,
        &registry,
        &chat(),
        &mut |_| {},
        &CancelToken::new(),
        &mut |event| fallbacks.push(event.clone()),
    )
    .unwrap_err();
    assert!(
        matches!(error, ProviderError::PolicyDenied { .. }),
        "{error:?}"
    );
    assert!(cloud.requests().is_empty(), "the cloud provider was called");
    assert_eq!(fallbacks.len(), 1);
    assert_eq!(cloud.descriptor().locality, Locality::Remote);
}

#[test]
fn closures_can_serve_as_provider_lookups() {
    let provider: Arc<dyn ModelProvider> = Arc::new(fake(
        "solo",
        Locality::Local,
        FakeScript::new().then(FakeTurn::text("hi")),
    ));
    let decision = Router::new()
        .route(
            &RouteRequest::new(TaskKind::Chat),
            &provider.models().unwrap(),
        )
        .unwrap();
    let lookup = |id: &ProviderId| (id.as_str() == "solo").then(|| Arc::clone(&provider));
    let (response, _) = run_with_fallback(
        &decision,
        &lookup,
        &chat(),
        &mut |_| {},
        &CancelToken::new(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(response.message.content, "hi");
}
