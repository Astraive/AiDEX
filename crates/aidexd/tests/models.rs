//! `model.*`, `provider.*`, and `network.*` over IPC: vault-handle provider
//! authentication end to end, offline refusal, and inspectable routing.

mod common;

use aidex_providers::{
    FakeProvider, Locality, ModelDescriptor, ModelId, ModelRef, ModelRole, ProviderEntry,
    ProviderId, ToolCallSupport,
};
use aidex_vault::KdfParams;
use aidexd::{
    Client, ClientError, ErrorCode,
    methods::{
        models::{
            MODEL_LIST, MODEL_PROVIDERS, MODEL_ROUTE, ModelList, NETWORK_OFFLINE, NETWORK_STATUS,
            NetworkStatus, PROVIDER_ADD, PROVIDER_OPEN_LINK, PROVIDER_PATCH, ProviderList,
            RouteResult,
        },
        runtime::{RUNTIME_CATALOG, RUNTIME_LIST},
        settings::{MODEL_PREFERENCES_GET, MODEL_PREFERENCES_SET},
        vault::{INIT, LOCK, SECRET_SET},
    },
};
use common::{options, start};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

/// A loopback HTTP server answering every request with one `/models` listing
/// and recording each request head.
struct ModelServer {
    port: u16,
    connections: Arc<AtomicUsize>,
    heads: Arc<Mutex<Vec<String>>>,
}

impl ModelServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let heads = Arc::new(Mutex::new(Vec::new()));
        let (count, record) = (Arc::clone(&connections), Arc::clone(&heads));
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                count.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                record.lock().unwrap().push(head);
                let reply =
                    r#"{"object":"list","data":[{"id":"gpt-test","context_length":128000}]}"#;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        Self {
            port,
            connections,
            heads,
        }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }
}

fn rpc_error(error: ClientError) -> (Option<ErrorCode>, Value) {
    let kind = error.rpc_kind();
    match error {
        ClientError::Rpc { data, .. } => (kind, data.unwrap_or(Value::Null)),
        other => panic!("expected an RPC error, got {other}"),
    }
}

#[test]
fn provider_credentials_come_from_vault_handles_scoped_to_the_provider() {
    let secret = format!(
        "sk-test-{}",
        aidex_core::EntityId::generate("secret").unwrap().as_str()
    );
    let server = ModelServer::start();
    let root = common::temp_drive();
    let mut captured = None;
    let daemon = start(root.path(), options(), |daemon| {
        // Tiny Argon2id cost keeps debug-build tests fast.
        daemon
            .services()
            .vault()
            .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
        captured = Some(Arc::clone(daemon.services()));
    });
    let services = captured.expect("services captured before start");
    let mut client = Client::connect(root.path(), "test-models").unwrap();
    // A loopback stand-in for a cloud endpoint (forced remote so offline gates it).
    let added = client
        .call(
            PROVIDER_ADD,
            json!({
                "id": "cloud",
                "base_url": server.base_url(),
                "locality": "remote",
                "secret_name": "cloud-key",
            }),
        )
        .unwrap();
    assert_eq!(added["descriptor"]["auth"]["secret_name"], "cloud-key");

    // No vault yet: the listing fails explicitly and nothing is sent.
    let listing: ModelList = client.call_as(MODEL_LIST, Value::Null).unwrap();
    assert_eq!(listing.failures[0].kind.as_str(), "credential_unavailable");
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);

    client
        .call(INIT, json!({"passphrase": "correct horse battery staple"}))
        .unwrap();
    // A secret released only to another provider is refused for this one.
    client
        .call(
            SECRET_SET,
            json!({"name": "cloud-key", "value": secret, "consumers": ["provider:other"], "kind": "api_key"}),
        )
        .unwrap();
    let listing: ModelList = client.call_as(MODEL_LIST, Value::Null).unwrap();
    assert_eq!(listing.failures[0].kind.as_str(), "credential_unavailable");
    assert!(
        listing.failures[0].message.contains("not allowed"),
        "{:?}",
        listing.failures
    );
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);

    client
        .call(
            SECRET_SET,
            json!({"name": "cloud-key", "value": secret, "consumers": ["provider:cloud"], "kind": "api_key"}),
        )
        .unwrap();
    let listing: ModelList = client.call_as(MODEL_LIST, Value::Null).unwrap();
    assert!(listing.failures.is_empty(), "{:?}", listing.failures);
    assert_eq!(listing.models[0].model.to_string(), "cloud/gpt-test");
    assert_eq!(listing.models[0].context_window, 128_000);
    let heads = server.heads.lock().unwrap().clone();
    assert_eq!(heads.len(), 1);
    assert!(
        heads[0].contains(&format!("Bearer {secret}"))
            || heads[0].contains(&format!("bearer {secret}")),
        "{heads:?}"
    );

    // Offline mode refuses the remote provider without a connection.
    let status: NetworkStatus = client
        .call_as(NETWORK_OFFLINE, json!({"enabled": true}))
        .unwrap();
    assert!(status.offline);
    let routed = client
        .call(MODEL_ROUTE, json!({"task": "code"}))
        .unwrap_err();
    let (kind, data) = rpc_error(routed);
    assert_eq!(kind, Some(ErrorCode::ModelUnavailable));
    assert_eq!(data["failures"][0]["kind"], "offline_refused");
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);

    // Locking the vault stops credential release.
    client
        .call(NETWORK_OFFLINE, json!({"enabled": false}))
        .unwrap();
    client.call(LOCK, Value::Null).unwrap();
    let provider = services
        .models()
        .registry()
        .provider(&ProviderId::new("cloud").unwrap())
        .unwrap();
    let error = provider
        .chat(
            &aidex_providers::ChatRequest::new(
                ModelId::new("gpt-test").unwrap(),
                vec![aidex_providers::ChatMessage::user("hi")],
            ),
            &mut |_| {},
            &aidex_providers::CancelToken::new(),
        )
        .unwrap_err();
    assert_eq!(
        error,
        aidex_providers::ProviderError::Credential(aidex_providers::CredentialError::Locked)
    );
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);

    let saved = std::fs::read_to_string(services.paths().config().join("providers.toml")).unwrap();
    assert!(!saved.contains(&secret));
    drop(client);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn routing_and_network_state_are_inspectable_over_ipc() {
    let root = common::temp_drive();
    let daemon = start(root.path(), options(), |daemon| {
        let id = ProviderId::new("local").unwrap();
        let fake = FakeProvider::new(id.clone(), Locality::Local).with_model(
            ModelDescriptor::new(
                ModelRef::new(id, ModelId::new("coder").unwrap()),
                Locality::Local,
                32_768,
            )
            .with_roles([ModelRole::Code])
            .with_tool_calls(ToolCallSupport::Native),
        );
        daemon
            .services()
            .models()
            .registry()
            .register(Box::new(fake))
            .unwrap();
    });
    let mut client = Client::connect(root.path(), "test-models").unwrap();
    let status: NetworkStatus = client.call_as(NETWORK_STATUS, Value::Null).unwrap();
    assert!(!status.offline);
    let routed: RouteResult = client
        .call_as(
            MODEL_ROUTE,
            json!({"task": "code", "needs_tools": true, "privacy": "local_only"}),
        )
        .unwrap();
    assert_eq!(routed.decision.primary.to_string(), "local/coder");
    assert_eq!(routed.model.context_window, 32_768);
    assert_eq!(
        routed.model.capabilities.tool_calls,
        ToolCallSupport::Native
    );
    assert!(
        routed
            .decision
            .reasons
            .iter()
            .any(|reason| reason.contains("local_only"))
    );
    let error = client
        .call(MODEL_ROUTE, json!({"task": "code", "model": "cloud/x"}))
        .unwrap_err();
    assert_eq!(rpc_error(error).0, Some(ErrorCode::NotFound));

    // Toggling offline mode is recorded as a typed, durable event.
    client
        .call(NETWORK_OFFLINE, json!({"enabled": true}))
        .unwrap();
    let page = client
        .call(
            aidexd::methods::events::REPLAY,
            json!({"after": 0, "filter": {"kinds": ["policy.changed"]}}),
        )
        .unwrap();
    let events = page["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{page}");
    assert_eq!(events[0]["event"]["type"], "policy.changed");
    assert!(
        events[0]["event"]["data"]["summary"]
            .as_str()
            .unwrap()
            .contains("offline mode on")
    );
    drop(client);
    daemon.stop();
    daemon.wait().unwrap();
}

#[test]
fn account_metadata_refresh_and_model_preferences_over_ipc() {
    let server = ModelServer::start();
    let root = common::temp_drive();
    let daemon = start(root.path(), options(), |daemon| {
        daemon
            .services()
            .vault()
            .set_kdf_params(KdfParams::new(8, 1, 1).unwrap());
    });
    let mut client = Client::connect(root.path(), "test-models").unwrap();
    let added: ProviderEntry = client
        .call_as(
            PROVIDER_ADD,
            json!({"id": "lab", "base_url": server.base_url(), "display_name": "Lab box", "preset_id": "vllm"}),
        )
        .unwrap();
    assert_eq!(added.display_name.as_deref(), Some("Lab box"));
    assert_eq!(added.preset_id.as_deref(), Some("vllm"));

    // One provider at a time; cached until an explicit refresh.
    for (params, connections) in [
        (json!({"provider_id": "lab"}), 1),
        (json!({"provider_id": "lab"}), 1),
        (json!({"provider_id": "lab", "refresh": true}), 2),
    ] {
        let listing: ModelList = client.call_as(MODEL_LIST, params).unwrap();
        assert_eq!(listing.models[0].model.to_string(), "lab/gpt-test");
        assert_eq!(server.connections.load(Ordering::SeqCst), connections);
    }

    let disabled: ProviderEntry = client
        .call_as(PROVIDER_PATCH, json!({"id": "lab", "enabled": false}))
        .unwrap();
    assert!(!disabled.enabled);
    let listing: ModelList = client.call_as(MODEL_LIST, Value::Null).unwrap();
    assert!(listing.models.is_empty() && listing.failures.is_empty());
    let providers: ProviderList = client.call_as(MODEL_PROVIDERS, Value::Null).unwrap();
    let lab = &providers.providers[0];
    assert_eq!(lab.display_name.as_deref(), Some("Lab box"));
    assert!(!lab.enabled);
    assert_eq!(lab.catalog.as_ref().unwrap().model_count, 1);
    assert_eq!(server.connections.load(Ordering::SeqCst), 2);

    let (kind, _) = rpc_error(
        client
            .call(
                PROVIDER_OPEN_LINK,
                json!({"preset_id": "nope", "link": "docs"}),
            )
            .unwrap_err(),
    );
    assert_eq!(kind, Some(ErrorCode::InvalidParams));
    let catalog = client.call(RUNTIME_CATALOG, Value::Null).unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 10);

    // No vault yet: the vault's own error, never an empty record.
    let (kind, _) = rpc_error(client.call(MODEL_PREFERENCES_GET, Value::Null).unwrap_err());
    assert_eq!(kind, Some(ErrorCode::NotFound));
    client
        .call(INIT, json!({"passphrase": "correct horse battery staple"}))
        .unwrap();
    let saved = client
        .call(
            MODEL_PREFERENCES_SET,
            json!({"favorite_models": ["lab/gpt-test", "lab/gpt-test"]}),
        )
        .unwrap();
    assert_eq!(
        saved,
        json!({"favorite_models": ["lab/gpt-test"], "hidden_models": []})
    );
    assert_eq!(
        client.call(MODEL_PREFERENCES_GET, Value::Null).unwrap(),
        saved
    );
    assert_eq!(
        client.call(RUNTIME_LIST, Value::Null).unwrap(),
        json!({"runtimes": []})
    );
    drop(client);
    daemon.stop();
    daemon.wait().unwrap();
}
