//! Guarded fetch against a local fake server: SSRF refusals before any
//! socket, redirects re-validated per hop, size caps, content types, and the
//! egress check.

mod support;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use aidex_core::CancelToken;
use aidex_web::{
    EgressPurpose, LocalTargets, MAX_TEXT_BYTES, WebClient, WebError, WebOptions, WebSettings,
};
use support::{CredentialLog, EgressLog, FakeServer, Reply, fixed_dns, no_dns, open_client};

fn loopback_options() -> WebOptions {
    WebOptions {
        local_targets: LocalTargets::Loopback,
        ..WebOptions::default()
    }
}

fn client_with(egress: &EgressLog, options: WebOptions) -> WebClient {
    WebClient::new(
        WebSettings::default(),
        CredentialLog::default().release(Err(WebError::Cancelled)),
        egress.allow_all(),
    )
    .unwrap()
    .with_options(options)
}

#[test]
fn a_loopback_page_is_fetched_as_readable_text() {
    let server = FakeServer::start(vec![Reply::html(
        r#"<html><head><title>Fixture &amp; Page</title><script>steal()</script></head>
        <body><h2>Section</h2><p>Hello <a href="/next?x=1">next page</a>.</p>
        <ul><li>one</li><li>two</li></ul></body></html>"#,
    )]);
    let egress = EgressLog::default();
    let client = client_with(&egress, loopback_options());
    let url = server.url("/page#section");
    let page = client.fetch(&url, &CancelToken::new()).unwrap();
    assert_eq!(page.url, server.url("/page"));
    assert_eq!(page.final_url, server.url("/page"));
    assert_eq!(page.status, 200);
    assert_eq!(page.content_type, "text/html");
    assert_eq!(page.title.as_deref(), Some("Fixture & Page"));
    assert!(!page.truncated);
    assert_eq!(
        page.text,
        format!(
            "## Section\n\nHello [next page]({}).\n\n- one\n- two",
            server.url("/next?x=1")
        )
    );
    let request = &server.requests()[0];
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/page");
    assert!(request.header("Accept").unwrap().starts_with("text/html"));
    assert!(request.header("Cookie").is_none());
    let calls = egress.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].purpose, EgressPurpose::Fetch);
    assert_eq!(
        (
            calls[0].scheme.as_str(),
            calls[0].host.as_str(),
            calls[0].port,
            calls[0].hop
        ),
        ("http", "127.0.0.1", server.port(), 0)
    );
}

#[test]
fn json_and_plain_text_bodies_are_returned_as_text() {
    let server = FakeServer::start(vec![
        Reply::json(200, r#"{"ok":true}"#),
        Reply::new(404, "text/plain; charset=utf-8", "not here\r\n"),
    ]);
    let client = client_with(&EgressLog::default(), loopback_options());
    let page = client
        .fetch(&server.url("/api"), &CancelToken::new())
        .unwrap();
    assert_eq!(
        (page.content_type.as_str(), page.text.as_str(), page.title),
        ("application/json", r#"{"ok":true}"#, None)
    );
    let page = client
        .fetch(&server.url("/missing"), &CancelToken::new())
        .unwrap();
    assert_eq!((page.status, page.text.as_str()), (404, "not here\n"));
}

#[test]
fn ssrf_targets_are_refused_before_any_socket_or_lookup() {
    let server = FakeServer::start(Vec::new());
    let egress = EgressLog::default();
    let dns = Arc::new(AtomicUsize::new(0));
    let client =
        client_with(&egress, WebOptions::default()).with_dns_resolver(no_dns(Arc::clone(&dns)));
    let refused_targets = [
        server.url("/"),
        format!("http://localhost:{}/", server.port()),
        format!("http://[::1]:{}/", server.port()),
        "https://127.0.0.1/".to_owned(),
        "https://10.0.0.1/".to_owned(),
        "https://10.255.1.2:8443/admin".to_owned(),
        "https://172.16.0.1/".to_owned(),
        "https://192.168.1.1/".to_owned(),
        "https://169.254.169.254/latest/meta-data/".to_owned(),
        "https://[fe80::1]/".to_owned(),
        "https://[::ffff:127.0.0.1]/".to_owned(),
        "https://[::ffff:169.254.169.254]/".to_owned(),
        "https://0.0.0.0/".to_owned(),
        "https://[::]/".to_owned(),
        "https://0x7f.1/".to_owned(),
        "https://2130706433/".to_owned(),
    ];
    for url in &refused_targets {
        let error = client.fetch(url, &CancelToken::new()).unwrap_err();
        assert!(
            matches!(error, WebError::TargetRefused(_)),
            "{url}: {error:?}"
        );
        assert_eq!(error.reason(), "target_refused");
    }
    let invalid_urls = [
        "https://user:pass@example.com/",
        "https://user@example.com/",
        format!("http://user@127.0.0.1:{}/", server.port()).leak() as &str,
        "ftp://example.com/file",
        "file:///etc/passwd",
        "gopher://example.com/",
        "javascript:alert(1)",
        "data:text/html,<p>hi</p>",
        "http://example.com/",
        "http://10.0.0.1/",
        "example.com/no-scheme",
        "",
    ];
    for url in invalid_urls {
        let error = client.fetch(url, &CancelToken::new()).unwrap_err();
        assert!(matches!(error, WebError::InvalidUrl(_)), "{url}: {error:?}");
    }
    assert!(egress.calls().is_empty(), "refusals happen before egress");
    assert_eq!(dns.load(Ordering::SeqCst), 0);
    assert_eq!(server.connections(), 0);
}

#[test]
fn explicit_local_targets_need_the_matching_allowance() {
    let server = FakeServer::start(vec![Reply::html("<p>local</p>")]);
    let client = client_with(&EgressLog::default(), loopback_options());
    // Loopback is allowed, private ranges and link-local are not.
    for url in [
        "https://10.0.0.1/",
        "https://192.168.0.10/",
        "https://169.254.169.254/",
    ] {
        assert!(matches!(
            client.fetch(url, &CancelToken::new()),
            Err(WebError::TargetRefused(_))
        ));
    }
    let page = client.fetch(&server.url("/"), &CancelToken::new()).unwrap();
    assert_eq!(page.text, "local");

    let private = client_with(
        &EgressLog::default(),
        WebOptions {
            local_targets: LocalTargets::LoopbackAndPrivate,
            ..WebOptions::default()
        },
    )
    .with_dns_resolver(no_dns(Arc::new(AtomicUsize::new(0))));
    // Link-local (cloud metadata) stays refused at every setting.
    assert!(matches!(
        private.fetch("https://169.254.169.254/", &CancelToken::new()),
        Err(WebError::TargetRefused(_))
    ));
}

#[test]
fn names_that_resolve_to_non_public_addresses_are_refused() {
    let egress = EgressLog::default();
    for addresses in [
        &["127.0.0.1"][..],
        &["10.1.2.3"][..],
        &["169.254.169.254"][..],
        &["::1"][..],
        &["fd00::1"][..],
        &["93.184.216.34", "192.168.0.1"][..],
    ] {
        let addresses: &'static [&'static str] = Box::leak(addresses.to_vec().into_boxed_slice());
        let client = client_with(
            &egress,
            WebOptions {
                local_targets: LocalTargets::LoopbackAndPrivate,
                ..WebOptions::default()
            },
        )
        .with_dns_resolver(fixed_dns(addresses));
        let error = client
            .fetch("https://rebind.example/secret", &CancelToken::new())
            .unwrap_err();
        assert!(
            matches!(error, WebError::TargetRefused(_)),
            "{addresses:?}: {error:?}"
        );
    }
    // The egress check saw the name (before DNS), every time.
    assert!(
        egress
            .calls()
            .iter()
            .all(|call| call.host == "rebind.example" && call.port == 443)
    );
}

#[test]
fn redirects_are_followed_and_every_hop_is_rechecked() {
    let server = FakeServer::start(Vec::new());
    server.push(Reply::redirect(302, "/middle"));
    server.push(Reply::redirect(
        301,
        &format!("http://localhost:{}/end", server.port()),
    ));
    server.push(Reply::new(200, "text/plain", "arrived"));
    let egress = EgressLog::default();
    let client = client_with(&egress, loopback_options());
    let page = client
        .fetch(&server.url("/start"), &CancelToken::new())
        .unwrap();
    assert_eq!(page.text, "arrived");
    assert_eq!(page.url, server.url("/start"));
    assert_eq!(
        page.final_url,
        format!("http://localhost:{}/end", server.port())
    );
    let hops: Vec<(String, usize)> = egress
        .calls()
        .into_iter()
        .map(|call| (call.host, call.hop))
        .collect();
    assert_eq!(
        hops,
        vec![
            ("127.0.0.1".to_owned(), 0),
            ("127.0.0.1".to_owned(), 1),
            ("localhost".to_owned(), 2),
        ]
    );
    let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
    assert_eq!(paths, vec!["/start", "/middle", "/end"]);
}

#[test]
fn redirects_into_refused_targets_stop_before_connecting() {
    for (location, expected) in [
        (
            "https://169.254.169.254/latest/meta-data/",
            "target_refused",
        ),
        ("https://10.0.0.7/internal", "target_refused"),
        ("https://[::ffff:10.0.0.7]/", "target_refused"),
        ("http://10.0.0.7/", "invalid_url"),
        ("http://example.com/downgrade", "invalid_url"),
        ("ftp://example.com/", "invalid_url"),
        ("file:///etc/passwd", "invalid_url"),
        ("https://user:pw@example.com/", "invalid_url"),
    ] {
        let server = FakeServer::start(vec![Reply::redirect(302, location)]);
        let egress = EgressLog::default();
        let client = client_with(&egress, loopback_options())
            .with_dns_resolver(no_dns(Arc::new(AtomicUsize::new(0))));
        let error = client
            .fetch(&server.url("/go"), &CancelToken::new())
            .unwrap_err();
        assert_eq!(error.reason(), expected, "{location}: {error:?}");
        assert_eq!(server.connections(), 1, "{location}");
        assert_eq!(egress.calls().len(), 1, "{location}: only the first hop");
    }
    // A redirect without a Location header is a protocol error.
    let server = FakeServer::start(vec![Reply::without_content_type(302, "")]);
    let client = client_with(&EgressLog::default(), loopback_options());
    assert!(matches!(
        client.fetch(&server.url("/"), &CancelToken::new()),
        Err(WebError::Protocol(_))
    ));
}

#[test]
fn more_than_five_redirects_fail() {
    let server = FakeServer::start(Vec::new());
    for hop in 0..6 {
        server.push(Reply::redirect(302, &format!("/hop{hop}")));
    }
    server.push(Reply::new(200, "text/plain", "never reached"));
    let client = client_with(&EgressLog::default(), loopback_options());
    assert_eq!(
        client
            .fetch(&server.url("/"), &CancelToken::new())
            .unwrap_err(),
        WebError::TooManyRedirects(5)
    );
    assert_eq!(server.connections(), 6);

    // Exactly five redirects are fine.
    let server = FakeServer::start(Vec::new());
    for hop in 0..5 {
        server.push(Reply::redirect(307, &format!("/hop{hop}")));
    }
    server.push(Reply::new(200, "text/plain", "made it"));
    let page = client.fetch(&server.url("/"), &CancelToken::new()).unwrap();
    assert_eq!(page.text, "made it");
}

#[test]
fn the_egress_check_can_refuse_a_redirect_hop() {
    let server = FakeServer::start(vec![
        Reply::redirect(302, "/second"),
        Reply::new(200, "text/plain", "unreachable"),
    ]);
    let egress = EgressLog::default();
    let client = WebClient::new(
        WebSettings::default(),
        CredentialLog::default().release(Err(WebError::Cancelled)),
        egress.check(|call| {
            if call.hop == 0 {
                Ok(())
            } else {
                Err(WebError::EgressDenied("hop refused by the test".into()))
            }
        }),
    )
    .unwrap()
    .with_options(loopback_options());
    assert!(matches!(
        client.fetch(&server.url("/first"), &CancelToken::new()),
        Err(WebError::EgressDenied(_))
    ));
    assert_eq!(server.connections(), 1);
}

#[test]
fn offline_refusal_opens_no_socket() {
    let server = FakeServer::start(vec![Reply::html("<p>never</p>")]);
    let egress = EgressLog::default();
    let dns = Arc::new(AtomicUsize::new(0));
    let client = WebClient::new(
        WebSettings::default(),
        CredentialLog::default().release(Err(WebError::Cancelled)),
        egress.check(|_| Err(WebError::Offline)),
    )
    .unwrap()
    .with_options(loopback_options())
    .with_dns_resolver(no_dns(Arc::clone(&dns)));
    for url in [server.url("/"), "https://example.com/".to_owned()] {
        assert_eq!(
            client.fetch(&url, &CancelToken::new()).unwrap_err(),
            WebError::Offline
        );
    }
    assert_eq!(egress.calls().len(), 2);
    assert_eq!(server.connections(), 0);
    assert_eq!(dns.load(Ordering::SeqCst), 0);
}

#[test]
fn bodies_and_extracted_text_are_capped() {
    // A body over 2 MiB: the raw read stops at the cap, the text at 256 KiB.
    let body = "a".repeat(2 * 1024 * 1024 + 4096);
    let server = FakeServer::start(vec![Reply::new(200, "text/plain", body)]);
    let client = client_with(&EgressLog::default(), loopback_options());
    let page = client
        .fetch(&server.url("/big"), &CancelToken::new())
        .unwrap();
    assert!(page.truncated);
    assert_eq!(page.text.len(), MAX_TEXT_BYTES);

    // HTML text over a small text cap is cut and marked truncated.
    let html = format!("<p>{}</p>", "word ".repeat(400));
    let server = FakeServer::start(vec![Reply::html(&html), Reply::html(&html)]);
    let small_text = client_with(
        &EgressLog::default(),
        WebOptions {
            max_text_bytes: 100,
            ..loopback_options()
        },
    );
    let page = small_text
        .fetch(&server.url("/"), &CancelToken::new())
        .unwrap();
    assert!(page.truncated);
    assert!(page.text.len() <= 100);

    // A small raw cap truncates the body before extraction.
    let small_raw = client_with(
        &EgressLog::default(),
        WebOptions {
            max_raw_bytes: 64,
            ..loopback_options()
        },
    );
    let page = small_raw
        .fetch(&server.url("/"), &CancelToken::new())
        .unwrap();
    assert!(page.truncated);
    assert!(page.text.len() < 64);
}

#[test]
fn unreadable_content_types_are_refused() {
    let server = FakeServer::start(vec![
        Reply::new(200, "image/png", "\u{89}PNG"),
        Reply::new(200, "application/octet-stream", "binary"),
        Reply::new(200, "application/pdf", "%PDF"),
        Reply::without_content_type(200, "unknown"),
    ]);
    let client = client_with(&EgressLog::default(), loopback_options());
    for expected in [
        "image/png",
        "application/octet-stream",
        "application/pdf",
        "(none)",
    ] {
        let error = client
            .fetch(&server.url("/"), &CancelToken::new())
            .unwrap_err();
        assert_eq!(error, WebError::UnsupportedContentType(expected.to_owned()));
    }
}

#[test]
fn a_cancelled_fetch_returns_promptly() {
    let release = Arc::new(AtomicBool::new(false));
    let server = FakeServer::start(vec![Reply::html("<p>late</p>").held(Arc::clone(&release))]);
    let client = open_client(WebSettings::default()).with_options(loopback_options());
    let cancel = CancelToken::new();
    let canceller = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancel.cancel();
        })
    };
    let started = Instant::now();
    assert_eq!(
        client.fetch(&server.url("/"), &cancel).unwrap_err(),
        WebError::Cancelled
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    canceller.join().unwrap();
    release.store(true, Ordering::SeqCst);

    // A request deadline reports a timeout.
    let release = Arc::new(AtomicBool::new(false));
    let server = FakeServer::start(vec![Reply::html("<p>late</p>").held(Arc::clone(&release))]);
    let client = open_client(WebSettings::default()).with_options(WebOptions {
        request_timeout: Duration::from_millis(200),
        ..loopback_options()
    });
    assert_eq!(
        client
            .fetch(&server.url("/"), &CancelToken::new())
            .unwrap_err(),
        WebError::Timeout
    );
    release.store(true, Ordering::SeqCst);
}
