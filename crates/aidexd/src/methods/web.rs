//! `web.*`: web search settings, the provider catalog, a test search, and
//! opening result links in the system browser.
//!
//! Search keys never travel through these methods: the settings hold only the
//! vault secret *name*, and the value is stored with `vault.secret.set` for
//! the consumer `process:web`. Searches pass the offline switch and the
//! broker's network policy (see `services/web.rs`).

use crate::{
    errors::{ErrorCode, RpcError},
    router::{CallContext, Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_auth::AuthError;
use aidex_core::CancelToken;
use aidex_web::{
    CredentialProblem, ProviderInfo, SearchProvider, SearchResult, WebError, WebSettings,
    WebSettingsPatch,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{sync::Arc, time::Instant};

pub const WEB_SETTINGS_GET: &str = "web.settings.get";
pub const WEB_SETTINGS_SET: &str = "web.settings.set";
pub const WEB_PROVIDERS: &str = "web.providers";
pub const WEB_TEST: &str = "web.test";
pub const WEB_OPEN: &str = "web.open";

/// The vault consumer provider keys must be stored for.
const CREDENTIAL_CONSUMER: &str = "process:web";

/// `web.settings.get` / `web.settings.set` result.
#[derive(Debug, Serialize)]
pub struct WebSettingsView {
    #[serde(flatten)]
    pub settings: WebSettings,
    /// Whether the settings name everything a search needs (the key itself
    /// is checked only when a search runs).
    pub configured: bool,
    /// What is still missing when `configured` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
    /// The vault consumer the provider key must be stored for.
    pub credential_consumer: &'static str,
}

impl WebSettingsView {
    fn new(settings: WebSettings) -> Self {
        let problem = settings.missing().map(|error| error.to_string());
        Self {
            configured: problem.is_none(),
            problem,
            settings,
            credential_consumer: CREDENTIAL_CONSUMER,
        }
    }
}

#[derive(Serialize)]
struct ProviderCatalog {
    providers: Vec<ProviderInfo>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestParams {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
    /// Unsaved changes to test with (the stored settings are not changed).
    #[serde(default)]
    settings: Option<WebSettingsPatch>,
}

#[derive(Debug, Serialize)]
struct TestResult {
    provider: SearchProvider,
    results: Vec<SearchResult>,
    elapsed_ms: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenParams {
    url: String,
}

/// A web error on the wire: a stable code plus `data.reason`. Messages name
/// hosts and providers, never a URL query, a header, or a key.
pub(crate) fn web_error(error: &WebError) -> RpcError {
    let code = match error {
        WebError::Offline | WebError::EgressDenied(_) | WebError::TargetRefused(_) => {
            ErrorCode::PolicyDenied
        }
        WebError::Credential {
            problem: CredentialProblem::Locked,
            ..
        } => ErrorCode::VaultLocked,
        WebError::Cancelled => ErrorCode::Cancelled,
        WebError::InvalidRequest(_) | WebError::InvalidSettings(_) | WebError::InvalidUrl(_) => {
            ErrorCode::InvalidParams
        }
        other => ErrorCode::from(other.code()),
    };
    RpcError::new(code, error.to_string()).with_data(json!({ "reason": error.reason() }))
}

fn open_error(error: AuthError) -> RpcError {
    let (code, reason, message) = match error {
        AuthError::NetworkOffline => (
            ErrorCode::PolicyDenied,
            "offline_refused",
            "Offline mode blocks opening web links.",
        ),
        AuthError::Invalid => (
            ErrorCode::InvalidArgument,
            "https_required",
            "Only https:// links can be opened in the system browser.",
        ),
        _ => (
            ErrorCode::Unavailable,
            "browser_unavailable",
            "The system browser could not be opened.",
        ),
    };
    RpcError::new(code, message).with_data(json!({ "reason": reason }))
}

/// Run `operation` with a token cancelled when the connection closes or the
/// daemon stops.
fn with_cancel<T>(context: &CallContext, operation: impl FnOnce(&CancelToken) -> T) -> T {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Finished<'a>(&'a AtomicBool);
    impl Drop for Finished<'_> {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let token = CancelToken::new();
    let done = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if context.is_cancelled() {
                    token.cancel();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        });
        let _finished = Finished(&done);
        operation(&token)
    })
}

fn test_search(
    services: &Services,
    context: &CallContext,
    params: TestParams,
) -> Result<TestResult, RpcError> {
    let web = services.web();
    let stored = web.settings().map_err(|error| web_error(&error))?;
    let settings = match &params.settings {
        Some(patch) => patch.apply(&stored).map_err(|error| web_error(&error))?,
        None => stored,
    };
    let provider = settings.provider;
    let client = web
        .client_for(settings)
        .map_err(|error| web_error(&error))?;
    let started = Instant::now();
    let results = with_cancel(context, |cancel| {
        client.search(&params.query, params.max_results, cancel)
    })
    .map_err(|error| web_error(&error))?;
    Ok(TestResult {
        provider,
        results,
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(WEB_SETTINGS_GET, move |_context, params| {
        expect_no_params(&params)?;
        let settings = shared.web().settings().map_err(|error| web_error(&error))?;
        to_result(&WebSettingsView::new(settings))
    })?;
    let shared = Arc::clone(services);
    router.register(WEB_SETTINGS_SET, move |_context, params| {
        let patch: WebSettingsPatch = parse_params(params)?;
        let settings = shared
            .web()
            .update(&patch)
            .map_err(|error| web_error(&error))?;
        to_result(&WebSettingsView::new(settings))
    })?;
    router.register(WEB_PROVIDERS, move |_context, params| {
        expect_no_params(&params)?;
        to_result(&ProviderCatalog {
            providers: aidex_web::catalog(),
        })
    })?;
    let shared = Arc::clone(services);
    router.register(WEB_TEST, move |context, params| {
        let params: TestParams = parse_params(params)?;
        if context.is_cancelled() {
            return Err(RpcError::new(ErrorCode::Cancelled, "request was cancelled"));
        }
        to_result(&test_search(&shared, context, params)?)
    })?;
    let shared = Arc::clone(services);
    router.register(WEB_OPEN, move |_context, params| {
        let params: OpenParams = parse_params(params)?;
        let url = aidex_web::validate_open_url(&params.url).map_err(|error| web_error(&error))?;
        let web = Arc::clone(shared.web());
        aidex_auth::open_in_browser(url.as_str(), &move || !web.is_offline())
            .map_err(open_error)?;
        Ok(json!({ "opened": true }))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lifecycle::ShutdownSignal, router::ClientInfo};
    use aidex_events::EventFilter;
    use aidex_vault::{
        ConsumerScope, SecretBytes, SecretKind, SecretName, SecretPolicy, SecretString,
    };
    use serde_json::Value;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    fn fixture() -> (tempfile::TempDir, Arc<Services>, Router, CallContext) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("web-test", "1"), shutdown);
        (root, services, router, context)
    }

    fn reason(error: &RpcError) -> Option<String> {
        serde_json::to_value(error).ok()?["data"]["reason"]
            .as_str()
            .map(str::to_owned)
    }

    /// A loopback HTTP server answering every connection with `body`
    /// (SearXNG JSON), recording request heads.
    struct Searx {
        port: u16,
        connections: Arc<AtomicUsize>,
        heads: Arc<Mutex<Vec<String>>>,
    }

    impl Searx {
        fn start(body: &'static str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let connections = Arc::new(AtomicUsize::new(0));
            let heads = Arc::new(Mutex::new(Vec::new()));
            let (count, recorded) = (Arc::clone(&connections), Arc::clone(&heads));
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    count.fetch_add(1, Ordering::SeqCst);
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut head = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        head.push_str(&line);
                    }
                    recorded.lock().unwrap().push(head);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.flush();
                }
            });
            Self {
                port,
                connections,
                heads,
            }
        }

        fn endpoint(&self) -> String {
            format!("http://127.0.0.1:{}/searx", self.port)
        }

        fn connections(&self) -> usize {
            self.connections.load(Ordering::SeqCst)
        }

        fn heads(&self) -> Vec<String> {
            self.heads.lock().unwrap().clone()
        }
    }

    const SEARX_BODY: &str = r#"{"query":"rust","results":[
        {"title":"Rust","url":"https://www.rust-lang.org/","content":"A <b>language</b>","publishedDate":null},
        {"title":"Crates","url":"https://crates.io/","content":"Registry"}
    ]}"#;

    #[test]
    fn settings_round_trip_through_the_closed_web_file() {
        let (_root, services, router, context) = fixture();
        let defaults = router
            .call(&context, WEB_SETTINGS_GET, Value::Null)
            .unwrap();
        assert_eq!(
            defaults,
            json!({
                "version": 1, "enabled": false, "provider": "duckduckgo", "endpoint": null,
                "secret_name": null, "max_results": 5, "fetch_enabled": true,
                "configured": true, "credential_consumer": "process:web"
            })
        );
        let path = services.paths().config().join(aidex_web::SETTINGS_FILE);
        assert!(!path.exists(), "reading never creates the file");

        let saved = router
            .call(
                &context,
                WEB_SETTINGS_SET,
                json!({"enabled": true, "provider": "brave", "secret_name": "brave-key", "max_results": 3}),
            )
            .unwrap();
        assert_eq!(saved["provider"], "brave");
        assert_eq!(saved["secret_name"], "brave-key");
        assert_eq!(saved["configured"], true);
        assert_eq!(
            router
                .call(&context, WEB_SETTINGS_GET, Value::Null)
                .unwrap(),
            saved
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("provider = \"brave\""), "{text}");

        // A key value is never accepted here, and nothing is written.
        let refused = router
            .call(
                &context,
                WEB_SETTINGS_SET,
                json!({"api_key": "sk-pasted-secret-value"}),
            )
            .unwrap_err();
        assert_eq!(refused.kind(), Some(ErrorCode::InvalidParams));
        assert!(!refused.to_string().contains("sk-pasted-secret-value"));
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("sk-pasted")
        );

        for (patch, reason_text) in [
            (json!({"max_results": 0}), "invalid_settings"),
            (json!({"provider": "google"}), ""),
            (
                json!({"provider": "searxng", "endpoint": "http://searx.example"}),
                "invalid_settings",
            ),
            (
                json!({"provider": "duckduckgo", "secret_name": "k"}),
                "invalid_settings",
            ),
        ] {
            let error = router
                .call(&context, WEB_SETTINGS_SET, patch.clone())
                .unwrap_err();
            assert_eq!(error.kind(), Some(ErrorCode::InvalidParams), "{patch}");
            if !reason_text.is_empty() {
                assert_eq!(reason(&error).as_deref(), Some(reason_text), "{patch}");
            }
        }

        // SearXNG without an endpoint saves but reports what is missing.
        let incomplete = router
            .call(&context, WEB_SETTINGS_SET, json!({"provider": "searxng"}))
            .unwrap();
        assert_eq!(incomplete["configured"], false);
        assert_eq!(
            incomplete["secret_name"],
            Value::Null,
            "the Brave key name does not follow the provider change"
        );
        assert!(incomplete["problem"].as_str().unwrap().contains("endpoint"));
        let named = router
            .call(
                &context,
                WEB_SETTINGS_SET,
                json!({"secret_name": "searx-token"}),
            )
            .unwrap();
        assert_eq!(named["secret_name"], "searx-token");
        let cleared = router
            .call(&context, WEB_SETTINGS_SET, json!({"secret_name": null}))
            .unwrap();
        assert_eq!(cleared["secret_name"], Value::Null);
    }

    #[test]
    fn providers_list_the_catalog_with_icons_and_key_links() {
        let (_root, _services, router, context) = fixture();
        let catalog = router.call(&context, WEB_PROVIDERS, Value::Null).unwrap();
        let providers = catalog["providers"].as_array().unwrap();
        let ids: Vec<&str> = providers
            .iter()
            .map(|provider| provider["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["brave", "tavily", "exa", "searxng", "duckduckgo"]);
        for provider in providers {
            assert_eq!(provider["icon"], provider["id"]);
            assert_eq!(
                provider["key_url"].is_string(),
                provider["requires_key"].as_bool().unwrap()
            );
        }
        assert!(
            router
                .call(&context, WEB_PROVIDERS, json!({"x": 1}))
                .is_err()
        );
    }

    #[test]
    fn test_searches_pass_the_policy_gate_and_record_the_decision() {
        let (_root, services, router, context) = fixture();
        let searx = Searx::start(SEARX_BODY);
        let before = services.events().latest_cursor().unwrap();
        // Unsaved settings can be tested without changing the stored ones.
        let result = router
            .call(
                &context,
                WEB_TEST,
                json!({"query": "rust", "max_results": 2,
                       "settings": {"provider": "searxng", "endpoint": searx.endpoint()}}),
            )
            .unwrap();
        assert_eq!(result["provider"], "searxng");
        let results = result["results"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["title"], "Rust");
        assert_eq!(results[0]["snippet"], "A language");
        assert_eq!(results[0]["source"], "searxng");
        assert!(result["elapsed_ms"].is_u64());
        assert!(searx.heads()[0].starts_with("GET /searx/search?q=rust&format=json&pageno=1 "));
        let stored = router
            .call(&context, WEB_SETTINGS_GET, Value::Null)
            .unwrap();
        assert_eq!(stored["provider"], "duckduckgo");

        let decisions: Vec<_> = services
            .events()
            .replay(&EventFilter::all(), before, 100)
            .unwrap()
            .events
            .into_iter()
            .filter(|event| event.event.name() == "policy.decision")
            .collect();
        assert_eq!(decisions.len(), 1, "one decision per connection");

        for (params, expected) in [
            (json!({"query": ""}), ErrorCode::InvalidParams),
            (
                json!({"query": "rust", "max_results": 11}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"query": "rust", "unknown": true}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"query": "rust", "settings": {"api_key": "sk-x"}}),
                ErrorCode::InvalidParams,
            ),
        ] {
            let error = router.call(&context, WEB_TEST, params.clone()).unwrap_err();
            assert_eq!(error.kind(), Some(expected), "{params}");
        }
    }

    #[test]
    fn offline_mode_and_network_policy_refuse_before_any_socket() {
        let (_root, services, router, context) = fixture();
        let searx = Searx::start(SEARX_BODY);
        router
            .call(
                &context,
                WEB_SETTINGS_SET,
                json!({"provider": "searxng", "endpoint": searx.endpoint()}),
            )
            .unwrap();
        services.models().offline().set_offline(true).unwrap();
        let error = router
            .call(&context, WEB_TEST, json!({"query": "rust"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::PolicyDenied));
        assert_eq!(reason(&error).as_deref(), Some("offline_refused"));
        services.models().offline().set_offline(false).unwrap();

        // Air-gapped privacy denies every destination, loopback included.
        services
            .broker()
            .change_policy(
                aidex_broker::PolicyChange::Privacy(aidex_policy::PrivacyMode::AirGapped),
                &aidex_broker::Principal::LocalUser,
            )
            .unwrap();
        let error = router
            .call(&context, WEB_TEST, json!({"query": "rust"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::PolicyDenied));
        assert_eq!(reason(&error).as_deref(), Some("policy_denied"));
        assert_eq!(searx.connections(), 0, "no socket was opened");
    }

    #[test]
    fn provider_keys_come_from_the_vault_for_process_web() {
        let (_root, services, router, context) = fixture();
        let searx = Searx::start(SEARX_BODY);
        router
            .call(
                &context,
                WEB_SETTINGS_SET,
                json!({"provider": "searxng", "endpoint": searx.endpoint(), "secret_name": "searx-token"}),
            )
            .unwrap();
        // No vault yet.
        let error = router
            .call(&context, WEB_TEST, json!({"query": "rust"}))
            .unwrap_err();
        assert_eq!(reason(&error).as_deref(), Some("credential_unavailable"));

        let vault = services.vault();
        let passphrase = SecretString::new("fixture passphrase only".into());
        vault.init(&passphrase, false).unwrap();
        let key = format!(
            "tok-{}",
            aidex_core::EntityId::generate("secret").unwrap().as_str()
        );
        let error = router
            .call(&context, WEB_TEST, json!({"query": "rust"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::NotFound));
        assert_eq!(reason(&error).as_deref(), Some("secret_not_found"));

        // A secret for another consumer is not released to web search.
        vault
            .put_secret(
                &SecretName::new("searx-token").unwrap(),
                SecretKind::ApiKey,
                SecretBytes::new(key.clone().into_bytes()),
                SecretPolicy::new([ConsumerScope::provider("openai").unwrap()]).unwrap(),
            )
            .unwrap();
        let error = router
            .call(&context, WEB_TEST, json!({"query": "rust"}))
            .unwrap_err();
        assert_eq!(reason(&error).as_deref(), Some("secret_denied"));
        assert_eq!(searx.connections(), 0);

        vault
            .put_secret(
                &SecretName::new("searx-token").unwrap(),
                SecretKind::ApiKey,
                SecretBytes::new(key.clone().into_bytes()),
                SecretPolicy::new([ConsumerScope::process("web").unwrap()]).unwrap(),
            )
            .unwrap();
        let result = router
            .call(&context, WEB_TEST, json!({"query": "rust"}))
            .unwrap();
        assert!(!result.to_string().contains(&key), "the key never returns");
        let head = &searx.heads()[0];
        assert!(
            head.contains(&format!("Bearer {key}")) || head.contains(&format!("bearer {key}")),
            "{head}"
        );

        vault.lock();
        let error = router
            .call(&context, WEB_TEST, json!({"query": "rust"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::VaultLocked));
        assert!(!error.to_string().contains(&key));
    }

    #[test]
    fn links_are_validated_before_the_browser_is_asked() {
        let (_root, services, router, context) = fixture();
        for url in [
            "javascript:alert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "https://user:pw@example.com/",
            "https://example.com/a b",
            "ms-settings:privacy",
            "",
        ] {
            let error = router
                .call(&context, WEB_OPEN, json!({ "url": url }))
                .unwrap_err();
            assert_eq!(error.kind(), Some(ErrorCode::InvalidParams), "{url}");
            assert_eq!(reason(&error).as_deref(), Some("invalid_url"), "{url}");
        }
        // Plain http passes URL validation but the system opener takes only
        // https, and refuses before launching anything.
        let error = router
            .call(&context, WEB_OPEN, json!({ "url": "http://example.com/" }))
            .unwrap_err();
        assert_eq!(reason(&error).as_deref(), Some("https_required"));
        // Offline mode refuses before anything is launched.
        services.models().offline().set_offline(true).unwrap();
        let error = router
            .call(
                &context,
                WEB_OPEN,
                json!({ "url": "https://www.rust-lang.org/" }),
            )
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::PolicyDenied));
        assert_eq!(reason(&error).as_deref(), Some("offline_refused"));
        assert!(
            router
                .call(
                    &context,
                    WEB_OPEN,
                    json!({ "url": "https://x/", "extra": 1 })
                )
                .is_err()
        );
    }
}
