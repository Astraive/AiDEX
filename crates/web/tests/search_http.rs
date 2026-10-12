//! Search providers against a local fake server: request shape, key
//! placement, response parsing, and first-class errors.

mod support;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use aidex_core::CancelToken;
use aidex_web::{
    CredentialProblem, EgressPurpose, SearchProvider, WebClient, WebError, WebOptions, WebSettings,
};
use support::{CredentialLog, EgressLog, FakeServer, Reply, no_dns, sentinel_secret};

fn settings(provider: SearchProvider, secret_name: Option<&str>) -> WebSettings {
    WebSettings {
        enabled: true,
        provider,
        secret_name: secret_name.map(str::to_owned),
        ..WebSettings::default()
    }
}

/// A client for `provider` whose searches go to `path` on `server`.
fn client(
    server: &FakeServer,
    path: &str,
    settings: WebSettings,
    key: &str,
) -> (WebClient, CredentialLog, EgressLog) {
    let credentials = CredentialLog::default();
    let egress = EgressLog::default();
    let client = WebClient::new(
        settings,
        credentials.release(Ok(key.to_owned())),
        egress.allow_all(),
    )
    .unwrap()
    .with_search_url(&server.url(path))
    .unwrap();
    (client, credentials, egress)
}

fn assert_secret_absent(text: &str, secret: &str) {
    assert!(!text.contains(secret), "the key leaked into: {text}");
}

#[test]
fn brave_sends_the_subscription_token_and_parses_web_results() {
    let server = FakeServer::start(vec![Reply::json(
        200,
        r#"{"type":"search","query":{"original":"rust web"},"web":{"results":[
            {"title":"The <strong>Rust</strong> Programming Language","url":"https://www.rust-lang.org/","description":"A language empowering <strong>everyone</strong> &amp; more.","page_age":"2024-05-01T00:00:00","age":"May 1, 2024"},
            {"title":"Rust by Example","url":"https://doc.rust-lang.org/rust-by-example/","description":"Learn Rust with examples.","age":"3 days ago"},
            {"title":"Ad-free duplicate","url":"https://www.rust-lang.org/","description":"dup"},
            {"title":"Bad link","url":"javascript:alert(1)","description":"x"},
            {"title":"Third","url":"https://crates.io/","description":"Registry"},
            {"title":"Fourth","url":"https://docs.rs/","description":"Docs"}
        ]}}"#,
    )]);
    let key = sentinel_secret();
    let (client, credentials, egress) = client(
        &server,
        "/res/v1/web/search",
        settings(SearchProvider::Brave, Some("brave-key")),
        &key,
    );
    let results = client
        .search("  rust   web ", Some(3), &CancelToken::new())
        .unwrap();

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/res/v1/web/search?q=rust+web&count=3");
    assert_eq!(request.header("X-Subscription-Token"), Some(key.as_str()));
    assert_eq!(request.header("Accept"), Some("application/json"));
    assert!(request.header("User-Agent").unwrap().starts_with("aidex/"));
    assert_eq!(credentials.requests(), vec!["brave:brave-key".to_owned()]);
    let calls = egress.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].purpose,
        EgressPurpose::Search(SearchProvider::Brave)
    );
    assert_eq!(
        (calls[0].host.as_str(), calls[0].port, calls[0].hop),
        ("127.0.0.1", server.port(), 0)
    );

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].title, "The Rust Programming Language");
    assert_eq!(results[0].url, "https://www.rust-lang.org/");
    assert_eq!(results[0].snippet, "A language empowering everyone & more.");
    assert_eq!(results[0].published.as_deref(), Some("2024-05-01T00:00:00"));
    assert_eq!(results[0].source, SearchProvider::Brave);
    assert_eq!(results[1].published.as_deref(), Some("3 days ago"));
    assert_eq!(results[2].url, "https://crates.io/");
    let shown = serde_json::to_string(&results).unwrap();
    assert!(shown.contains(r#""source":"brave""#), "{shown}");
    assert_secret_absent(&shown, &key);
}

#[test]
fn tavily_posts_the_key_in_the_json_body() {
    let server = FakeServer::start(vec![Reply::json(
        200,
        r#"{"query":"rust","results":[
            {"title":"Rust","url":"https://www.rust-lang.org/","content":"Fast, reliable, productive.","score":0.9},
            {"title":"News","url":"https://blog.rust-lang.org/","content":"Release notes","published_date":"Thu, 01 Aug 2024 00:00:00 GMT"}
        ],"response_time":0.4}"#,
    )]);
    let key = sentinel_secret();
    let (client, _, _) = client(
        &server,
        "/search",
        settings(SearchProvider::Tavily, Some("tavily-key")),
        &key,
    );
    let results = client.search("rust", None, &CancelToken::new()).unwrap();
    let request = &server.requests()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/search");
    assert_eq!(request.header("Content-Type"), Some("application/json"));
    let body = request.json();
    assert_eq!(body["api_key"], key.as_str());
    assert_eq!(body["query"], "rust");
    assert_eq!(body["max_results"], 5);
    assert_eq!(body["search_depth"], "basic");
    assert!(request.header("Authorization").is_none());
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].snippet, "Fast, reliable, productive.");
    assert_eq!(
        results[1].published.as_deref(),
        Some("Thu, 01 Aug 2024 00:00:00 GMT")
    );
}

#[test]
fn exa_sends_the_key_header_and_reads_highlights() {
    let server = FakeServer::start(vec![Reply::json(
        200,
        r#"{"requestId":"r1","results":[
            {"title":"Exa result","url":"https://example.com/a","publishedDate":"2024-01-02T00:00:00.000Z","highlights":["First highlight.","Second one."]},
            {"title":"Summary only","url":"https://example.com/b","summary":"A summary."},
            {"title":"Text only","url":"https://example.com/c","text":"Page text."},
            {"title":null,"url":"https://example.com/d"}
        ]}"#,
    )]);
    let key = sentinel_secret();
    let (client, _, _) = client(
        &server,
        "/search",
        settings(SearchProvider::Exa, Some("exa-key")),
        &key,
    );
    let results = client
        .search("neural search", Some(10), &CancelToken::new())
        .unwrap();
    let request = &server.requests()[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.header("x-api-key"), Some(key.as_str()));
    let body = request.json();
    assert_eq!(body["query"], "neural search");
    assert_eq!(body["numResults"], 10);
    assert_secret_absent(&String::from_utf8_lossy(&request.body), &key);
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].snippet, "First highlight. \u{2026} Second one.");
    assert_eq!(
        results[0].published.as_deref(),
        Some("2024-01-02T00:00:00.000Z")
    );
    assert_eq!(results[1].snippet, "A summary.");
    assert_eq!(results[2].snippet, "Page text.");
    // A missing title falls back to the host.
    assert_eq!(results[3].title, "example.com");
}

#[test]
fn searxng_uses_the_configured_endpoint_and_an_optional_bearer_key() {
    let body = r#"{"query":"rust","number_of_results":0,"results":[
        {"title":"Rust","url":"https://www.rust-lang.org/","content":"Language","engine":"duckduckgo","publishedDate":null},
        {"title":"Wiki","url":"https://en.wikipedia.org/wiki/Rust_(programming_language)","content":"Encyclopedia","publishedDate":"2024-02-03T00:00:00"}
    ]}"#;
    let server = FakeServer::start(vec![Reply::json(200, body), Reply::json(200, body)]);
    let mut settings = settings(SearchProvider::Searxng, None);
    settings.endpoint = Some(server.url("/searx"));
    let egress = EgressLog::default();
    let credentials = CredentialLog::default();
    let anonymous = WebClient::new(
        settings.clone(),
        credentials.release(Err(WebError::Cancelled)),
        egress.allow_all(),
    )
    .unwrap();
    let results = anonymous
        .search("rust", Some(2), &CancelToken::new())
        .unwrap();
    assert!(credentials.requests().is_empty(), "no key is configured");
    let request = &server.requests()[0];
    assert_eq!(request.path, "/searx/search?q=rust&format=json&pageno=1");
    assert!(request.header("Authorization").is_none());
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].published, None);
    assert_eq!(
        results[1].url,
        "https://en.wikipedia.org/wiki/Rust_(programming_language)"
    );

    let key = sentinel_secret();
    settings.secret_name = Some("searx-token".into());
    let keyed = WebClient::new(
        settings,
        credentials.release(Ok(key.clone())),
        egress.allow_all(),
    )
    .unwrap();
    keyed.search("rust", Some(1), &CancelToken::new()).unwrap();
    let request = &server.requests()[1];
    assert_eq!(
        request.header("Authorization"),
        Some(format!("Bearer {key}").as_str())
    );
    assert_eq!(
        credentials.requests(),
        vec!["searxng:searx-token".to_owned()]
    );
}

#[test]
fn duckduckgo_html_results_are_parsed_from_a_fixture() {
    let html = include_str!("fixtures/duckduckgo.html");
    let server = FakeServer::start(vec![Reply::html(html)]);
    let credentials = CredentialLog::default();
    let client = WebClient::new(
        settings(SearchProvider::Duckduckgo, None),
        credentials.release(Err(WebError::Cancelled)),
        EgressLog::default().allow_all(),
    )
    .unwrap()
    .with_search_url(&server.url("/html/"))
    .unwrap();
    let results = client
        .search("rust web", Some(10), &CancelToken::new())
        .unwrap();
    assert!(credentials.requests().is_empty());
    let request = &server.requests()[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/html/?q=rust+web");
    assert_eq!(request.header("Accept"), Some("text/html"));

    let summary: Vec<(&str, &str, &str)> = results
        .iter()
        .map(|result| {
            (
                result.title.as_str(),
                result.url.as_str(),
                result.snippet.as_str(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "Rust Programming Language",
                "https://www.rust-lang.org/",
                "A language empowering everyone to build reliable and efficient software."
            ),
            (
                "The Rust Book \u{2014} Getting Started",
                "https://doc.rust-lang.org/book/ch01-00-getting-started.html?q=1&r=2",
                "Install Rust, write Hello, world! & meet Cargo."
            ),
            ("crates.io: Rust Package Registry", "https://crates.io/", ""),
            (
                "Docs.rs",
                "https://docs.rs/",
                "Documentation for every crate."
            ),
        ]
    );
    assert!(
        results
            .iter()
            .all(|result| result.source == SearchProvider::Duckduckgo)
    );
}

#[test]
fn provider_failures_are_first_class_errors() {
    let key = sentinel_secret();
    type Expectation = fn(&WebError) -> bool;
    let cases: Vec<(Reply, Expectation)> = vec![
        (Reply::json(401, r#"{"error":"bad key"}"#), |error| {
            matches!(error, WebError::Unauthorized(_))
        }),
        (Reply::json(403, "{}"), |error| {
            matches!(error, WebError::Unauthorized(_))
        }),
        (Reply::json(429, "{}"), |error| {
            matches!(error, WebError::RateLimited(_))
        }),
        (Reply::json(503, "{}"), |error| {
            matches!(error, WebError::Http { status: 503, .. })
        }),
        (
            Reply::redirect(302, "https://elsewhere.example/"),
            |error| matches!(error, WebError::Protocol(_)),
        ),
        (
            Reply::new(200, "text/html", "<html>not json</html>"),
            |error| matches!(error, WebError::Protocol(_)),
        ),
        (Reply::json(200, r#"{"web":{"results":"nope"}}"#), |error| {
            matches!(error, WebError::Protocol(_))
        }),
    ];
    for (reply, expected) in cases {
        let server = FakeServer::start(vec![reply.clone()]);
        let (client, _, _) = client(
            &server,
            "/search",
            settings(SearchProvider::Brave, Some("brave-key")),
            &key,
        );
        let error = client
            .search("rust", None, &CancelToken::new())
            .unwrap_err();
        assert!(expected(&error), "{reply:?} -> {error:?}");
        assert_secret_absent(&format!("{error} {error:?}"), &key);
        assert!(
            !error.to_string().contains("bad key"),
            "bodies are not quoted"
        );
    }

    // An oversized API response fails instead of being half-parsed.
    let server = FakeServer::start(vec![Reply::json(
        200,
        &format!(r#"{{"results":[],"pad":"{}"}}"#, "x".repeat(4096)),
    )]);
    let (client, _, _) = client(
        &server,
        "/search",
        settings(SearchProvider::Tavily, Some("tavily-key")),
        &key,
    );
    let client = client.with_options(WebOptions {
        max_raw_bytes: 1024,
        ..WebOptions::default()
    });
    assert!(matches!(
        client.search("rust", None, &CancelToken::new()),
        Err(WebError::Protocol(_))
    ));

    // DuckDuckGo's bot check (HTTP 202) is reported as rate limiting.
    let server = FakeServer::start(vec![Reply::new(202, "text/html", "<html>challenge</html>")]);
    let client = WebClient::new(
        settings(SearchProvider::Duckduckgo, None),
        CredentialLog::default().release(Err(WebError::Cancelled)),
        EgressLog::default().allow_all(),
    )
    .unwrap()
    .with_search_url(&server.url("/html/"))
    .unwrap();
    assert!(matches!(
        client.search("rust", None, &CancelToken::new()),
        Err(WebError::RateLimited(_))
    ));
}

#[test]
fn keys_are_released_only_after_the_egress_check_approves() {
    let server = FakeServer::start(vec![Reply::json(200, r#"{"results":[]}"#)]);
    let credentials = CredentialLog::default();
    let egress = EgressLog::default();
    let dns = Arc::new(AtomicUsize::new(0));
    let client = WebClient::new(
        settings(SearchProvider::Tavily, Some("tavily-key")),
        credentials.release(Ok(sentinel_secret())),
        egress.check(|_| Err(WebError::Offline)),
    )
    .unwrap()
    .with_search_url(&server.url("/search"))
    .unwrap()
    .with_dns_resolver(no_dns(Arc::clone(&dns)));
    assert_eq!(
        client
            .search("rust", None, &CancelToken::new())
            .unwrap_err(),
        WebError::Offline
    );
    assert_eq!(egress.calls().len(), 1);
    assert!(credentials.requests().is_empty(), "no key was released");
    assert_eq!(server.connections(), 0, "no socket was opened");
    assert_eq!(dns.load(Ordering::SeqCst), 0);
}

#[test]
fn built_in_providers_ask_egress_for_their_fixed_https_host() {
    for (provider, host) in [
        (SearchProvider::Brave, "api.search.brave.com"),
        (SearchProvider::Tavily, "api.tavily.com"),
        (SearchProvider::Exa, "api.exa.ai"),
        (SearchProvider::Duckduckgo, "html.duckduckgo.com"),
    ] {
        let egress = EgressLog::default();
        let dns = Arc::new(AtomicUsize::new(0));
        let secret = provider.requires_key().then_some("key-name");
        let client = WebClient::new(
            settings(provider, secret),
            CredentialLog::default().release(Ok(sentinel_secret())),
            egress.check(|_| Err(WebError::EgressDenied("test policy".into()))),
        )
        .unwrap()
        .with_dns_resolver(no_dns(Arc::clone(&dns)));
        assert!(matches!(
            client.search("rust", None, &CancelToken::new()),
            Err(WebError::EgressDenied(_))
        ));
        let calls = egress.calls();
        assert_eq!(calls.len(), 1, "{provider}");
        assert_eq!(
            (
                calls[0].scheme.as_str(),
                calls[0].host.as_str(),
                calls[0].port
            ),
            ("https", host, 443),
            "{provider}"
        );
        assert_eq!(dns.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn missing_configuration_and_credentials_fail_before_any_request() {
    let server = FakeServer::start(Vec::new());
    // A key provider without a secret name.
    let (client, credentials, egress) = client(
        &server,
        "/search",
        settings(SearchProvider::Brave, None),
        "unused",
    );
    assert!(matches!(
        client.search("rust", None, &CancelToken::new()),
        Err(WebError::NotConfigured(_))
    ));
    assert!(credentials.requests().is_empty());
    assert!(egress.calls().is_empty());

    // SearXNG without an endpoint.
    let client = WebClient::new(
        settings(SearchProvider::Searxng, None),
        CredentialLog::default().release(Err(WebError::Cancelled)),
        EgressLog::default().allow_all(),
    )
    .unwrap();
    assert!(matches!(
        client.search("rust", None, &CancelToken::new()),
        Err(WebError::NotConfigured(_))
    ));

    // A locked vault.
    let egress = EgressLog::default();
    let client = WebClient::new(
        settings(SearchProvider::Exa, Some("exa-key")),
        CredentialLog::default().release(Err(WebError::Credential {
            problem: CredentialProblem::Locked,
            message: "the vault is locked".into(),
        })),
        egress.allow_all(),
    )
    .unwrap()
    .with_search_url(&server.url("/search"))
    .unwrap();
    let error = client
        .search("rust", None, &CancelToken::new())
        .unwrap_err();
    assert_eq!(error.reason(), "vault_locked");
    assert_eq!(server.connections(), 0);

    // Bad queries and counts.
    for (query, count) in [
        ("", None),
        ("   ", None),
        ("rust", Some(0)),
        ("rust", Some(11)),
    ] {
        assert!(matches!(
            client.search(query, count, &CancelToken::new()),
            Err(WebError::InvalidRequest(_))
        ));
    }
    assert_eq!(server.connections(), 0);
}

#[test]
fn a_cancelled_search_returns_promptly() {
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server = FakeServer::start(vec![
        Reply::json(200, r#"{"results":[]}"#).held(Arc::clone(&release)),
    ]);
    let (client, _, _) = client(
        &server,
        "/search",
        settings(SearchProvider::Tavily, Some("tavily-key")),
        &sentinel_secret(),
    );
    let cancel = CancelToken::new();
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            cancel.cancel();
        })
    };
    let started = std::time::Instant::now();
    assert_eq!(
        client.search("rust", None, &cancel).unwrap_err(),
        WebError::Cancelled
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    canceller.join().unwrap();
    release.store(true, Ordering::SeqCst);
}
