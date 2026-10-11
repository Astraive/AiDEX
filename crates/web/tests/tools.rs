use aidex_core::EntityId;
use aidex_drive::{DriveLayout, DrivePaths};
use aidex_events::NullEventSink;
use aidex_tools::{CancelToken, ExecutionPermit, PermitIssuer, ToolContext, ToolId, ToolRegistry};
use aidex_vault::{
    ConsumerScope, KdfParams, SecretBytes, SecretKind, SecretName, SecretPolicy, SecretString,
    VaultService, VaultServiceConfig, VaultState,
};
use aidex_web::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

const KEY: &str = "OpaqueFixtureCredentialWithoutTokenPrefix_4279";
type CapturedRequest = (String, Option<String>, Option<String>, Option<Value>);
#[derive(Default)]
struct FakeHttp {
    requests: Mutex<Vec<CapturedRequest>>,
    reply: Mutex<Option<HttpReply>>,
    on_send: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl HttpTransport for FakeHttp {
    fn send(&self, r: HttpRequest, cancel: &CancelToken) -> Result<HttpReply> {
        if cancel.is_cancelled() {
            return Err(WebError::Cancelled);
        }
        if !(r.network_allowed)() {
            return Err(WebError::Offline);
        }
        if let Some(change) = self.on_send.lock().unwrap().take() {
            change();
        }
        let auth = r.authorization.as_ref();
        self.requests.lock().unwrap().push((
            r.url,
            auth.map(|a| a.0.clone()),
            auth.map(|a| a.1.to_string()),
            r.json,
        ));
        Ok(self.reply.lock().unwrap().take().unwrap_or(HttpReply{status:200,content_type:"application/json".into(),body:serde_json::to_vec(&json!({"web":{"results":[{"title":"Source","url":"https://example.org/article","description":"useful snippet"}]}})).unwrap(),final_url:"https://example.org".into()}))
    }
}
struct Fixture {
    root: tempfile::TempDir,
    service: Arc<WebService>,
    vault: Arc<VaultService>,
    http: Arc<FakeHttp>,
    allowed: Arc<AtomicBool>,
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        VaultService::open(
            root.path().join("vault"),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap(),
    );
    vault
        .init(&SecretString::new("safe fixture passphrase".into()), false)
        .unwrap();
    let paths = DrivePaths::from_layout(root.path().to_owned(), DriveLayout::default());
    let allowed = Arc::new(AtomicBool::new(true));
    let gate = allowed.clone();
    let http = Arc::new(FakeHttp::default());
    let service = WebService::with_transport(
        &paths,
        vault.clone(),
        Arc::new(move || gate.load(Ordering::SeqCst)),
        http.clone(),
    )
    .unwrap();
    Fixture {
        root,
        service,
        vault,
        http,
        allowed,
    }
}
fn configure(f: &Fixture, provider: SearchProvider) {
    f.service
        .configure(
            SearchSettings {
                enabled: true,
                provider,
                ..Default::default()
            },
            Some(SecretBytes::from(KEY.to_owned())),
        )
        .unwrap();
}
fn context(f: &Fixture) -> ToolContext {
    ToolContext::new(
        EntityId::new("session").unwrap(),
        EntityId::new("workspace").unwrap(),
        EntityId::generate("call").unwrap(),
        Arc::new(aidex_workspace::ConfinedRoot::new(f.root.path()).unwrap()),
        Arc::new(NullEventSink),
    )
}
fn registry(f: &Fixture) -> ToolRegistry {
    let mut r = ToolRegistry::new();
    for t in f.service.tool_adapters() {
        r.register(t).unwrap();
    }
    r
}
fn input() -> SearchInput {
    SearchInput {
        query: "compiler reference".into(),
        count: 3,
        freshness: Some("pw".into()),
    }
}

#[test]
fn passive_status_and_planning_resolve_no_credentials_or_network() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    let r = registry(&f);
    let c = context(&f);
    let plan = r
        .plan(
            &c,
            &ToolId::new("web.search").unwrap(),
            serde_json::to_value(input()).unwrap(),
        )
        .unwrap();
    assert!(f.service.status().unwrap().credential_present);
    assert!(f.http.requests.lock().unwrap().is_empty());
    let intents = plan.intents();
    assert!(intents.iter().any(
        |i| i.action == aidex_policy::Action::NetworkRequest && i.attributes.network.is_some()
    ));
    assert!(intents.iter().any(|i| {
        i.action == aidex_policy::Action::SecretUse
            && i.attributes.secret.as_ref().is_some_and(|s| {
                s.name == SearchProvider::Brave.secret_name()
                    && s.consumer == "process:aidex-web-search"
            })
    }));
    assert!(!serde_json::to_string(&plan).unwrap().contains(KEY));
    assert!(
        !std::fs::read_to_string(f.root.path().join("private/config/web-search.json"))
            .unwrap()
            .contains(KEY)
    );
}
#[test]
fn brave_protocol_and_reflected_credentials_are_safe() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    *f.http.reply.lock().unwrap()=Some(HttpReply{status:200,content_type:"application/json".into(),body:serde_json::to_vec(&json!({"web":{"results":[{"title":KEY,"url":"https://example.org/page","description":KEY}]}})).unwrap(),final_url:SearchProvider::Brave.endpoint().into()});
    let out = f
        .service
        .search(
            &input(),
            &f.service.snapshot().unwrap(),
            &CancelToken::new(),
        )
        .unwrap();
    let req = f.http.requests.lock().unwrap();
    assert!(req[0].0.starts_with(SearchProvider::Brave.endpoint()));
    assert!(req[0].0.contains("freshness=pw"));
    assert_eq!(req[0].1.as_deref(), Some("X-Subscription-Token"));
    assert_eq!(req[0].2.as_deref(), Some(KEY));
    assert!(!serde_json::to_string(&out).unwrap().contains(KEY));
    assert!(out.results[0].snippet.contains("REDACTED"));
}
#[test]
fn direct_vault_rotation_rejects_old_approval_without_network_or_vault_lock() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    let r = registry(&f);
    let c = context(&f);
    let plan = r
        .plan(
            &c,
            &ToolId::new("web.search").unwrap(),
            serde_json::to_value(input()).unwrap(),
        )
        .unwrap();
    let permit = ExecutionPermit::for_plan(&plan, PermitIssuer::Development);
    f.vault
        .put_secret(
            &SecretName::new(SearchProvider::Brave.secret_name()).unwrap(),
            SecretKind::ApiKey,
            SecretBytes::from("replacement fixture credential".to_owned()),
            SecretPolicy::new([ConsumerScope::process("aidex-web-search").unwrap()]).unwrap(),
        )
        .unwrap();
    assert!(r.execute(&c, &plan, permit).is_err());
    assert!(f.http.requests.lock().unwrap().is_empty());
    assert_eq!(f.vault.status().state, VaultState::Unlocked);
}
#[test]
fn configuration_revision_and_origin_changes_do_not_reuse_credentials() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    let snapshot = f.service.snapshot().unwrap();
    assert!(
        f.service
            .configure(
                SearchSettings {
                    enabled: true,
                    provider: SearchProvider::Tavily,
                    ..Default::default()
                },
                None
            )
            .is_err()
    );
    f.service
        .configure(
            SearchSettings {
                enabled: true,
                provider: SearchProvider::Brave,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert_eq!(
        f.service
            .search(&input(), &snapshot, &CancelToken::new())
            .unwrap_err(),
        WebError::Conflict
    );
    assert!(f.http.requests.lock().unwrap().is_empty());
    configure(&f, SearchProvider::Tavily);
    *f.http.reply.lock().unwrap() = Some(HttpReply {
        status: 200,
        content_type: "application/json".into(),
        body: br#"{"results":[]}"#.to_vec(),
        final_url: SearchProvider::Tavily.endpoint().into(),
    });
    let mut query = input();
    query.freshness = None;
    f.service
        .search(&query, &f.service.snapshot().unwrap(), &CancelToken::new())
        .unwrap();
    let req = f.http.requests.lock().unwrap();
    assert_eq!(req[0].0, SearchProvider::Tavily.endpoint());
    assert_eq!(req[0].1.as_deref(), Some("Authorization"));
    assert_eq!(req[0].2.as_deref(), Some(format!("Bearer {KEY}").as_str()));
    assert!(req[0].3.as_ref().unwrap().get("api_key").is_none());
}
#[test]
fn cancelled_and_offline_search_never_reaches_transport() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    let snapshot = f.service.snapshot().unwrap();
    let cancel = CancelToken::new();
    cancel.cancel();
    assert_eq!(
        f.service.search(&input(), &snapshot, &cancel).unwrap_err(),
        WebError::Cancelled
    );
    f.allowed.store(false, Ordering::SeqCst);
    assert_eq!(
        f.service
            .search(&input(), &snapshot, &CancelToken::new())
            .unwrap_err(),
        WebError::Offline
    );
    assert!(f.http.requests.lock().unwrap().is_empty());
}
#[test]
fn rate_limits_and_malformed_responses_return_closed_errors() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    for (status, body, expected) in [
        (429, KEY.as_bytes().to_vec(), WebError::Http(429)),
        (200, b"{invalid".to_vec(), WebError::Response),
    ] {
        *f.http.reply.lock().unwrap() = Some(HttpReply {
            status,
            content_type: "application/json".into(),
            body,
            final_url: SearchProvider::Brave.endpoint().into(),
        });
        let err = f
            .service
            .search(
                &input(),
                &f.service.snapshot().unwrap(),
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(err, expected);
        assert!(!err.to_string().contains(KEY));
    }
}
#[test]
fn private_destinations_credentials_and_arbitrary_ports_fail_before_transport() {
    let f = fixture();
    for url in [
        "http://example.org",
        "https://127.0.0.1",
        "https://10.0.0.1",
        "https://169.254.169.254",
        "https://[::1]",
        "https://[::ffff:127.0.0.1]",
        "https://[fc00::1]",
        "https://localhost",
        "https://user:pass@example.org",
        "https://example.org:8443",
    ] {
        assert!(
            f.service
                .fetch(&FetchInput { url: url.into() }, &CancelToken::new())
                .is_err(),
            "{url}"
        );
    }
    assert!(f.http.requests.lock().unwrap().is_empty());
}
#[test]
fn fetch_returns_bounded_untrusted_text_and_rejects_binary_content() {
    let f = fixture();
    let input = FetchInput {
        url: "https://example.org/page".into(),
    };
    *f.http.reply.lock().unwrap() = Some(HttpReply {
        status: 200,
        content_type: "text/html; charset=utf-8".into(),
        body: b"<h1>Public page</h1><script>hidden secret</script><p>Visible &amp; useful</p>"
            .to_vec(),
        final_url: input.url.clone(),
    });
    let out = f.service.fetch(&input, &CancelToken::new()).unwrap();
    assert!(out.content.contains("Visible & useful"));
    assert!(!out.content.contains("hidden secret"));
    *f.http.reply.lock().unwrap() = Some(HttpReply {
        status: 200,
        content_type: "application/octet-stream".into(),
        body: b"binary".to_vec(),
        final_url: input.url.clone(),
    });
    assert_eq!(
        f.service.fetch(&input, &CancelToken::new()).unwrap_err(),
        WebError::Response
    );
}
#[test]
fn disconnect_revokes_only_the_selected_search_connection() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    configure(&f, SearchProvider::Tavily);
    let status = f.service.disconnect().unwrap();
    assert!(!status.settings.enabled);
    assert!(!status.credential_present);
    assert!(
        f.vault
            .secret_meta(&SecretName::new(SearchProvider::Brave.secret_name()).unwrap())
            .is_ok()
    );
}

#[test]
fn fetch_rejects_http_errors_and_bounds_utf8_output() {
    let f = fixture();
    let input = FetchInput {
        url: "https://example.org/page".into(),
    };
    *f.http.reply.lock().unwrap() = Some(HttpReply {
        status: 404,
        content_type: "text/plain".into(),
        body: b"not a source".to_vec(),
        final_url: input.url.clone(),
    });
    assert_eq!(
        f.service.fetch(&input, &CancelToken::new()).unwrap_err(),
        WebError::Http(404)
    );
    *f.http.reply.lock().unwrap() = Some(HttpReply {
        status: 200,
        content_type: "text/plain".into(),
        body: "界".repeat(24000).into_bytes(),
        final_url: input.url.clone(),
    });
    let output = f.service.fetch(&input, &CancelToken::new()).unwrap();
    assert!(output.truncated);
    assert!(output.content.len() <= 64 * 1024);
    assert!(output.content.ends_with('界'));
}

#[test]
fn search_refuses_oversized_vendor_payloads() {
    let f = fixture();
    configure(&f, SearchProvider::Brave);
    *f.http.reply.lock().unwrap() = Some(HttpReply {
        status: 200,
        content_type: "application/json".into(),
        body: vec![b' '; 1024 * 1024 + 1],
        final_url: SearchProvider::Brave.endpoint().into(),
    });
    assert_eq!(
        f.service
            .search(
                &input(),
                &f.service.snapshot().unwrap(),
                &CancelToken::new()
            )
            .unwrap_err(),
        WebError::TooLarge
    );
}

#[test]
fn disconnect_or_rotation_while_search_is_in_flight_invalidates_the_result() {
    for disconnect in [true, false] {
        let f = fixture();
        configure(&f, SearchProvider::Brave);
        let snapshot = f.service.snapshot().unwrap();
        let service = f.service.clone();
        let vault = f.vault.clone();
        *f.http.on_send.lock().unwrap() = Some(Box::new(move || {
            if disconnect {
                service.disconnect().unwrap();
            } else {
                vault
                    .put_secret(
                        &SecretName::new(SearchProvider::Brave.secret_name()).unwrap(),
                        SecretKind::ApiKey,
                        SecretBytes::from("rotated-during-request-fixture".to_owned()),
                        SecretPolicy::new([ConsumerScope::process("aidex-web-search").unwrap()])
                            .unwrap(),
                    )
                    .unwrap();
            }
        }));
        assert_eq!(
            f.service
                .search(&input(), &snapshot, &CancelToken::new())
                .unwrap_err(),
            WebError::Conflict
        );
    }
}
