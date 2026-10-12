//! Stdio round trips against the Python fixture server, launched through the
//! sandbox with a cleared environment and a vault-style secret.

mod support;

use std::{
    fs,
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use aidex_core::CancelToken;
use aidex_mcp::{
    EnvSecret, LATEST_PROTOCOL_VERSION, LogBuffer, McpClient, McpConfigStore, McpErrorKind,
    McpHost, McpSource, McpStatus, McpSupervisor, Scrubber, Timeouts,
};
use serde_json::{Value, json};
use support::{StaticSecrets, fixture_script, host, python, sentinel, stdio_config, stdio_spec};

fn cancel() -> CancelToken {
    CancelToken::new().with_timeout(Duration::from_secs(60))
}

fn log_text(logs: &LogBuffer) -> String {
    logs.tail(1000)
        .lines
        .iter()
        .map(|line| line.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_client_speaks_mcp_with_a_sandboxed_stdio_server() {
    let scratch = tempfile::tempdir().unwrap();
    let secret = sentinel();
    let mut config = stdio_config("fixture", "noisy");
    config.env_allowlist = vec!["CARGO_PKG_NAME".into()];
    config.env_from_secrets = vec![EnvSecret {
        env: "FIXTURE_TOKEN".into(),
        secret_name: "fixture-token".into(),
    }];
    config.validate().unwrap();
    let host = host(
        scratch.path(),
        StaticSecrets::default().with("fixture-token", "mcp:fixture", &secret),
    );
    let logs = Arc::new(LogBuffer::new());
    let launched = host.launch_stdio(&config, &logs).unwrap();
    assert!(!format!("{launched:?}").contains(&secret));
    let mut client = McpClient::stdio(
        launched.process,
        Arc::clone(&logs),
        Scrubber::new(launched.secrets),
        Timeouts::default(),
    )
    .unwrap();

    let identity = client.initialize(&cancel()).unwrap();
    assert_eq!(identity.name, "fixture");
    assert_eq!(identity.version, "1.0.0");
    assert_eq!(identity.protocol_version, LATEST_PROTOCOL_VERSION);
    assert!(identity.tools);

    // Seven tools over four pages; the schema without a type is normalized.
    let listing = client.list_tools(&cancel()).unwrap();
    let names: Vec<&str> = listing.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        ["echo", "add", "fail", "env", "image", "slow", "crash"]
    );
    assert_eq!(listing.tools[1].input_schema["type"], "object");
    assert_eq!(listing.tools[0].description, "Echo the given text");

    let echo = client
        .call_tool("echo", json!({"text": "hello"}), &cancel())
        .unwrap();
    assert_eq!(echo.content_text, "hello");
    assert!(!echo.is_error && !echo.truncated);
    let sum = client
        .call_tool("add", json!({"a": 2, "b": 3}), &cancel())
        .unwrap();
    assert_eq!(sum.content_text, "5");
    let failed = client.call_tool("fail", Value::Null, &cancel()).unwrap();
    assert!(failed.is_error);
    assert_eq!(failed.content_text, "the fixture tool failed");
    let image = client.call_tool("image", json!({}), &cancel()).unwrap();
    assert!(image.content_text.starts_with("[image: image/png"));
    let unknown = client
        .call_tool("missing", json!({}), &cancel())
        .unwrap_err();
    assert_eq!(unknown.kind, McpErrorKind::ServerError);
    assert!(unknown.message.contains("-32602"), "{}", unknown.message);
    let refused = client.call_tool("echo", json!([1]), &cancel()).unwrap_err();
    assert_eq!(refused.kind, McpErrorKind::InvalidRequest);
    client.ping(&cancel()).unwrap();

    // The environment is cleared: allowlisted names pass, others do not, and
    // the secret reaches the server but never comes back.
    let env = client
        .call_tool(
            "env",
            json!({"names": ["CARGO_PKG_NAME", "CARGO_MANIFEST_DIR", "FIXTURE_TOKEN"]}),
            &cancel(),
        )
        .unwrap();
    assert!(!env.content_text.contains(&secret));
    let reported: Value = serde_json::from_str(&env.content_text).unwrap();
    assert_eq!(reported["CARGO_PKG_NAME"], "aidex-mcp");
    assert_eq!(reported["CARGO_MANIFEST_DIR"], Value::Null);
    assert_eq!(reported["FIXTURE_TOKEN"], "[REDACTED]");

    // A request that times out is abandoned; its late answer is ignored.
    let started = Instant::now();
    let slow = client
        .call_tool(
            "slow",
            json!({"seconds": 1.5}),
            &CancelToken::new().with_timeout(Duration::from_millis(300)),
        )
        .unwrap_err();
    assert_eq!(slow.kind, McpErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(1));
    let after = client
        .call_tool("echo", json!({"text": "after"}), &cancel())
        .unwrap();
    assert_eq!(after.content_text, "after");

    // The server exiting ends the connection with its exit code and last words.
    let crashed = client.call_tool("crash", json!({}), &cancel()).unwrap_err();
    assert_eq!(crashed.kind, McpErrorKind::Disconnected);
    assert!(crashed.message.contains("code 3"), "{}", crashed.message);
    assert!(!client.is_alive());
    client.close();

    let text = log_text(&logs);
    assert!(text.contains("fixture starting (noisy)"), "{text}");
    assert!(text.contains("ignored a non-JSON line"), "{text}");
    assert!(text.contains("info: listing from 0"), "{text}");
    assert!(
        text.contains("declined the server's `roots/list` request"),
        "{text}"
    );
    assert!(text.contains("fixture initialized"), "{text}");
    assert!(!text.contains(&secret));
}

fn supervisor(
    scratch: &std::path::Path,
    secrets: StaticSecrets,
    timeouts: Timeouts,
) -> McpSupervisor {
    let store = McpConfigStore::in_dir(scratch.join("mcp"));
    McpSupervisor::with_timeouts(
        store,
        Arc::new(host(&scratch.join("scratch"), secrets)),
        timeouts,
    )
}

#[test]
fn the_supervisor_starts_on_demand_restarts_and_stops() {
    let scratch = tempfile::tempdir().unwrap();
    let supervisor = supervisor(
        scratch.path(),
        StaticSecrets::default(),
        Timeouts::default(),
    );
    supervisor
        .store()
        .add(&stdio_config("fixture", "normal"))
        .unwrap();
    assert_eq!(supervisor.state("fixture").status, McpStatus::Stopped);

    // The first tool listing starts the server.
    let tools = supervisor.list_tools("fixture", &cancel()).unwrap();
    assert_eq!(tools.len(), 7);
    let state = supervisor.state("fixture");
    assert_eq!(state.status, McpStatus::Running);
    assert_eq!(state.server.as_ref().unwrap().name, "fixture");
    assert!(state.process_id.as_deref().unwrap().starts_with("proc_"));
    assert!(state.started_at.is_some());
    assert!(supervisor.has_active());

    let echo = supervisor
        .call_tool("fixture", "echo", json!({"text": "hi"}), &cancel())
        .unwrap();
    assert_eq!(echo.content_text, "hi");
    let unknown = supervisor
        .call_tool("fixture", "nope", json!({}), &cancel())
        .unwrap_err();
    assert_eq!(unknown.kind, McpErrorKind::InvalidRequest);
    supervisor.ping("fixture", &cancel()).unwrap();

    let toolsets = supervisor.running_toolsets(Some("ws_any")).unwrap();
    assert_eq!(toolsets.len(), 1);
    assert_eq!(toolsets[0].server_id, "fixture");
    assert_eq!(toolsets[0].tools.len(), 7);

    // A crash marks the server failed; the next call starts it again.
    let crashed = supervisor
        .call_tool("fixture", "crash", json!({}), &cancel())
        .unwrap_err();
    assert_eq!(crashed.kind, McpErrorKind::Disconnected);
    let state = supervisor.state("fixture");
    assert_eq!(state.status, McpStatus::Failed);
    assert_eq!(state.error.unwrap().reason, McpErrorKind::Disconnected);
    assert!(supervisor.running_toolsets(None).unwrap().is_empty());
    let again = supervisor
        .call_tool("fixture", "add", json!({"a": 1, "b": 1}), &cancel())
        .unwrap();
    assert_eq!(again.content_text, "2");

    let restarted = supervisor.restart("fixture", &cancel()).unwrap();
    assert_eq!(restarted.status, McpStatus::Running);
    let stopped = supervisor.stop("fixture");
    assert_eq!(stopped.status, McpStatus::Stopped);
    assert!(stopped.tools.is_empty());
    assert!(!supervisor.has_active());
    let logs = supervisor.logs("fixture", 1000);
    let text: Vec<&str> = logs.lines.iter().map(|l| l.text.as_str()).collect();
    assert!(text.contains(&"stopped"), "{text:?}");

    // Disabled servers neither start nor answer.
    supervisor.store().set_enabled("fixture", false).unwrap();
    let disabled = supervisor.start("fixture", &cancel()).unwrap_err();
    assert_eq!(disabled.kind, McpErrorKind::Disabled);
    let disabled = supervisor
        .call_tool("fixture", "echo", json!({"text": "x"}), &cancel())
        .unwrap_err();
    assert_eq!(disabled.kind, McpErrorKind::Disabled);

    supervisor.store().set_enabled("fixture", true).unwrap();
    supervisor.start("fixture", &cancel()).unwrap();
    assert_eq!(supervisor.stop_all("vault locked"), 1);
    assert_eq!(supervisor.state("fixture").status, McpStatus::Stopped);
    assert_eq!(supervisor.stop_all("again"), 0);
}

#[test]
fn start_failures_set_the_server_status() {
    let scratch = tempfile::tempdir().unwrap();
    let supervisor = supervisor(
        scratch.path(),
        StaticSecrets::default(),
        Timeouts {
            initialize: Duration::from_millis(1500),
            ..Timeouts::default()
        },
    );
    let store = supervisor.store();

    let mut needs_secret = stdio_config("needs-secret", "normal");
    needs_secret.env_from_secrets = vec![EnvSecret {
        env: "FIXTURE_TOKEN".into(),
        secret_name: "missing-token".into(),
    }];
    store.add(&needs_secret).unwrap();
    let error = supervisor.start("needs-secret", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::NeedsAuth);
    let state = supervisor.state("needs-secret");
    assert_eq!(state.status, McpStatus::NeedsAuth);
    assert!(state.process_id.is_none());

    store.add(&stdio_config("old", "old-version")).unwrap();
    let error = supervisor.start("old", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::Protocol);
    assert_eq!(supervisor.state("old").status, McpStatus::Failed);

    store.add(&stdio_config("exits", "exit")).unwrap();
    let error = supervisor.start("exits", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::Disconnected);
    assert!(error.message.contains("code 2"), "{}", error.message);
    assert!(
        error.message.contains("fixture refusing to start"),
        "{}",
        error.message
    );

    store.add(&stdio_config("silent", "silent")).unwrap();
    let error = supervisor.start("silent", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::Timeout);
    assert!(error.message.contains("initialize"), "{}", error.message);
    assert!(error.message.contains("within 1 s"), "{}", error.message);
    // The server's own time limit ran out: it failed.
    assert_eq!(supervisor.state("silent").status, McpStatus::Failed);

    let mut missing = stdio_config("missing", "normal");
    missing.command = Some("aidex-no-such-mcp-server".into());
    store.add(&missing).unwrap();
    let error = supervisor.start("missing", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::LaunchFailed);

    let error = supervisor.start("unknown", &cancel()).unwrap_err();
    assert_eq!(error.kind, McpErrorKind::NotFound);
    assert!(!supervisor.has_active());
}

#[test]
fn stop_interrupts_a_call_in_flight() {
    let scratch = tempfile::tempdir().unwrap();
    let supervisor = Arc::new(supervisor(
        scratch.path(),
        StaticSecrets::default(),
        Timeouts::default(),
    ));
    supervisor
        .store()
        .add(&stdio_config("fixture", "normal"))
        .unwrap();
    supervisor.start("fixture", &cancel()).unwrap();
    let calling = {
        let supervisor = Arc::clone(&supervisor);
        thread::spawn(move || {
            supervisor.call_tool("fixture", "slow", json!({"seconds": 20}), &cancel())
        })
    };
    thread::sleep(Duration::from_millis(400));
    let started = Instant::now();
    let stopped = supervisor.stop("fixture");
    assert_eq!(stopped.status, McpStatus::Stopped);
    let outcome = calling.join().unwrap().unwrap_err();
    assert_eq!(outcome.kind, McpErrorKind::Cancelled);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "stop took {:?}",
        started.elapsed()
    );
    supervisor.forget("fixture");
    assert!(supervisor.logs("fixture", 10).lines.is_empty());
}

#[test]
fn a_start_the_caller_gives_up_on_leaves_the_server_stopped() {
    let scratch = tempfile::tempdir().unwrap();
    let supervisor = supervisor(
        scratch.path(),
        StaticSecrets::default(),
        Timeouts::default(),
    );
    supervisor
        .store()
        .add(&stdio_config("silent", "silent"))
        .unwrap();

    // The caller's deadline comes before the server's 60 s `initialize`.
    let error = supervisor
        .start(
            "silent",
            &CancelToken::new().with_timeout(Duration::from_millis(800)),
        )
        .unwrap_err();
    assert_eq!(error.kind, McpErrorKind::Timeout);
    assert!(error.message.contains("caller"), "{}", error.message);
    let state = supervisor.state("silent");
    assert_eq!(state.status, McpStatus::Stopped);
    assert!(state.error.is_none());

    // Later callers do not each wait out their own budget on it again: on-demand
    // starts are refused for a while.
    let on_demand = || {
        let started = Instant::now();
        let outcome = supervisor.list_tools(
            "silent",
            &CancelToken::new().with_timeout(Duration::from_millis(600)),
        );
        (outcome.unwrap_err(), started.elapsed())
    };
    let (refused, took) = on_demand();
    assert_eq!(refused.kind, McpErrorKind::Unavailable);
    assert!(refused.message.contains("start it now"), "{refused:?}");
    assert!(took < Duration::from_millis(300), "{took:?}");
    let refused = supervisor
        .call_tool("silent", "echo", json!({"text": "x"}), &cancel())
        .unwrap_err();
    assert_eq!(refused.kind, McpErrorKind::Unavailable);
    assert_eq!(supervisor.state("silent").status, McpStatus::Stopped);

    // A run stopped while the server starts (an explicit start retries at once).
    let stop = CancelToken::new();
    let stopping = {
        let stop = stop.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            stop.cancel();
        })
    };
    let error = supervisor.start("silent", &stop).unwrap_err();
    stopping.join().unwrap();
    assert_eq!(error.kind, McpErrorKind::Cancelled);
    assert_eq!(supervisor.state("silent").status, McpStatus::Stopped);
    // A Stop is no reason to wait: the next use starts it again, and that
    // start's deadline holds it back once more.
    let (tried, took) = on_demand();
    assert_eq!(tried.kind, McpErrorKind::Timeout);
    assert!(took >= Duration::from_millis(500), "{took:?}");
    assert_eq!(on_demand().0.kind, McpErrorKind::Unavailable);
    let logs = supervisor.logs("silent", 1000);
    assert!(
        logs.lines
            .iter()
            .any(|line| line.text.starts_with("start abandoned")),
        "{logs:?}"
    );
    assert!(
        logs.lines
            .iter()
            .any(|line| line.text.starts_with("not started on demand again")),
        "{logs:?}"
    );
    assert!(!supervisor.has_active());
}

#[test]
fn a_start_on_a_server_that_stopped_reading_its_input_ends() {
    let scratch = tempfile::tempdir().unwrap();
    let supervisor = Arc::new(supervisor(
        scratch.path(),
        StaticSecrets::default(),
        Timeouts {
            initialize: Duration::from_secs(2),
            request: Duration::from_secs(2),
            ..Timeouts::default()
        },
    ));
    supervisor
        .store()
        .add(&stdio_config("flood", "flood-init"))
        .unwrap();
    let start = |supervisor: &Arc<McpSupervisor>| {
        let (done, finished) = mpsc::channel();
        let supervisor = Arc::clone(supervisor);
        thread::spawn(move || {
            let _ = done.send(supervisor.start("flood", &cancel()));
        });
        finished
    };

    // The server pings instead of answering `initialize` and stops reading, so
    // neither the answers nor `notifications/cancelled` can be written. The
    // start still ends at its own `initialize` limit.
    let started = Instant::now();
    let error = start(&supervisor)
        .recv_timeout(Duration::from_secs(30))
        .expect("start did not return")
        .unwrap_err();
    assert_eq!(error.kind, McpErrorKind::Timeout);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "start took {:?}",
        started.elapsed()
    );
    assert_eq!(supervisor.state("flood").status, McpStatus::Failed);
    assert!(
        log_text_of(&supervisor, "flood").contains("dropped answers to the server's requests"),
        "{}",
        log_text_of(&supervisor, "flood")
    );

    // A stop while such a start is giving up returns at once.
    let starting = start(&supervisor);
    thread::sleep(Duration::from_millis(2500));
    let (done, stopped) = mpsc::channel();
    let began = Instant::now();
    {
        let supervisor = Arc::clone(&supervisor);
        thread::spawn(move || {
            let _ = done.send(supervisor.stop("flood"));
        });
    }
    let state = stopped
        .recv_timeout(Duration::from_secs(30))
        .expect("stop did not return");
    assert_eq!(state.status, McpStatus::Stopped);
    assert!(
        began.elapsed() < Duration::from_secs(10),
        "stop took {:?}",
        began.elapsed()
    );
    let outcome = starting
        .recv_timeout(Duration::from_secs(30))
        .expect("start did not return");
    assert!(outcome.is_err(), "{outcome:?}");
    assert!(!supervisor.has_active());
}

fn log_text_of(supervisor: &McpSupervisor, id: &str) -> String {
    supervisor
        .logs(id, 1000)
        .lines
        .iter()
        .map(|line| line.text.clone())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_busy_connection_serves_cached_tools_and_honours_cancellation() {
    let scratch = tempfile::tempdir().unwrap();
    let supervisor = Arc::new(supervisor(
        scratch.path(),
        StaticSecrets::default(),
        Timeouts::default(),
    ));
    supervisor
        .store()
        .add(&stdio_config("fixture", "normal"))
        .unwrap();
    supervisor.start("fixture", &cancel()).unwrap();
    let calling = {
        let supervisor = Arc::clone(&supervisor);
        thread::spawn(move || {
            supervisor.call_tool("fixture", "slow", json!({"seconds": 4}), &cancel())
        })
    };
    thread::sleep(Duration::from_millis(400));

    // Listing tools answers from the cache, without waiting for the call.
    let started = Instant::now();
    let tools = supervisor
        .list_tools("fixture", &CancelToken::new())
        .unwrap();
    assert_eq!(tools.len(), 7);
    assert!(started.elapsed() < Duration::from_secs(1));

    // A request queued behind the call gives up with its own token.
    let started = Instant::now();
    let late = supervisor
        .call_tool(
            "fixture",
            "echo",
            json!({"text": "queued"}),
            &CancelToken::new().with_timeout(Duration::from_millis(300)),
        )
        .unwrap_err();
    assert_eq!(late.kind, McpErrorKind::Timeout);
    let stopped = CancelToken::new();
    stopped.cancel();
    let cancelled = supervisor.ping("fixture", &stopped).unwrap_err();
    assert_eq!(cancelled.kind, McpErrorKind::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(2));

    let slow = calling.join().unwrap().unwrap();
    assert_eq!(slow.content_text, "late");
    // Giving up on a busy connection is no failure of the server.
    assert_eq!(supervisor.state("fixture").status, McpStatus::Running);
    supervisor.stop("fixture");
}

#[test]
fn stop_kills_a_server_that_stopped_reading_its_input() {
    let scratch = tempfile::tempdir().unwrap();
    let supervisor = Arc::new(supervisor(
        scratch.path(),
        StaticSecrets::default(),
        Timeouts::default(),
    ));
    supervisor
        .store()
        .add(&stdio_config("fixture", "chatty"))
        .unwrap();
    // The server pings far more than its input pipe holds answers to, then
    // stops reading, so the client's answer write blocks.
    let flooding = supervisor
        .call_tool(
            "fixture",
            "flood",
            json!({"pings": 20000, "seconds": 60}),
            &cancel(),
        )
        .unwrap();
    assert_eq!(flooding.content_text, "flooding");
    thread::sleep(Duration::from_millis(1500));

    // A request behind the answers that cannot be written gives up at its own
    // deadline (and the notice that it was cancelled at its own).
    let began = Instant::now();
    let late = supervisor
        .ping(
            "fixture",
            &CancelToken::new().with_timeout(Duration::from_secs(1)),
        )
        .unwrap_err();
    assert_eq!(late.kind, McpErrorKind::Timeout);
    assert!(
        began.elapsed() < Duration::from_secs(8),
        "ping took {:?}",
        began.elapsed()
    );

    let (done, stopped) = mpsc::channel();
    let started = Instant::now();
    {
        let supervisor = Arc::clone(&supervisor);
        thread::spawn(move || {
            let _ = done.send(supervisor.stop("fixture"));
        });
    }
    let state = stopped
        .recv_timeout(Duration::from_secs(30))
        .expect("stop did not return");
    assert_eq!(state.status, McpStatus::Stopped);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "stop took {:?}",
        started.elapsed()
    );
    assert!(!supervisor.has_active());
}

#[test]
fn responses_to_no_request_in_flight_are_dropped() {
    let scratch = tempfile::tempdir().unwrap();
    let host = host(scratch.path(), StaticSecrets::default());
    let logs = Arc::new(LogBuffer::new());
    let launched = host
        .launch_stdio(&stdio_config("fixture", "normal"), &logs)
        .unwrap();
    let mut client = McpClient::stdio(
        launched.process,
        Arc::clone(&logs),
        Scrubber::new(launched.secrets),
        Timeouts::default(),
    )
    .unwrap();
    client.initialize(&cancel()).unwrap();
    let sent = client
        .call_tool(
            "unsolicited",
            json!({"count": 60, "bytes": 20000}),
            &cancel(),
        )
        .unwrap();
    assert_eq!(sent.content_text, "sent");
    thread::sleep(Duration::from_millis(500));
    // The connection still answers, and its next request gets its own answer.
    client.ping(&cancel()).unwrap();
    let echo = client
        .call_tool("echo", json!({"text": "mine"}), &cancel())
        .unwrap();
    assert_eq!(echo.content_text, "mine");
    let ignored = logs
        .tail(1000)
        .lines
        .iter()
        .filter(|line| line.text == "ignored a response to no request in flight")
        .count();
    assert!(ignored >= 60, "{ignored}");
    client.close();
}

#[test]
fn plugin_and_workspace_servers_find_their_own_files() {
    let scratch = tempfile::tempdir().unwrap();
    let plugins = scratch.path().join("plugins");
    fs::create_dir_all(plugins.join("kit/servers")).unwrap();
    // The server imports a module of the plugin, which Python would cache.
    fs::copy(
        fixture_script(),
        plugins.join("kit/servers/fixture_server.py"),
    )
    .unwrap();
    fs::write(
        plugins.join("kit/servers/echo.py"),
        "import fixture_server  # noqa: F401 (runs the server)\n",
    )
    .unwrap();
    let workspace = scratch.path().join("project");
    fs::create_dir_all(workspace.join("scripts")).unwrap();
    fs::copy(fixture_script(), workspace.join("scripts/mcp.py")).unwrap();
    let known = workspace.clone();
    let host = host(&scratch.path().join("scratch"), StaticSecrets::default())
        .with_plugins_dir(&plugins)
        .with_workspace_roots(move |id| (id == "ws_1").then(|| known.clone()));
    let supervisor = McpSupervisor::new(
        McpConfigStore::in_dir(scratch.path().join("mcp")),
        Arc::new(host),
    );
    let store = supervisor.store();

    // A plugin's `.mcp.json` as Claude Code plugins write it.
    let mcp_json = plugins.join("kit/.mcp.json");
    let server = json!({
        "command": python(),
        "args": ["-u", "${CLAUDE_PLUGIN_ROOT}/servers/echo.py", "normal"],
    });
    fs::write(
        &mcp_json,
        json!({"mcpServers": {"echo": server}}).to_string(),
    )
    .unwrap();
    let mut config = McpConfigStore::import_mcp_json(&mcp_json, McpSource::plugin("kit"))
        .unwrap()
        .remove(0)
        .config
        .unwrap();
    config.id = "kit-echo".into();
    config.enabled = true;
    store.add(&config).unwrap();

    let tools = supervisor.list_tools("kit-echo", &cancel()).unwrap();
    assert_eq!(tools.len(), 7);
    let env = supervisor
        .call_tool(
            "kit-echo",
            "env",
            json!({"names": ["CLAUDE_PLUGIN_ROOT", "AIDEX_PLUGIN_ROOT", "PYTHONDONTWRITEBYTECODE"]}),
            &cancel(),
        )
        .unwrap();
    let reported: Value = serde_json::from_str(&env.content_text).unwrap();
    let root = reported["CLAUDE_PLUGIN_ROOT"].as_str().unwrap();
    assert!(Path::new(root).join("servers/echo.py").is_file(), "{root}");
    assert_eq!(reported["AIDEX_PLUGIN_ROOT"], root);
    // Nothing is written into the plugin's trusted (hashed) files.
    assert_eq!(reported["PYTHONDONTWRITEBYTECODE"], "1");
    assert!(!plugins.join("kit/servers/__pycache__").exists());
    // The stored configuration keeps the portable form.
    assert_eq!(
        store.get("kit-echo").unwrap().args[1],
        "${CLAUDE_PLUGIN_ROOT}/servers/echo.py"
    );

    // A bundled command that climbs out of the plugin is refused.
    let mut escape = config.clone();
    escape.id = "kit-escape".into();
    escape.command = Some("${CLAUDE_PLUGIN_ROOT}/../outside".into());
    store.add(&escape).unwrap();
    let refused = supervisor.start("kit-escape", &cancel()).unwrap_err();
    assert_eq!(refused.kind, McpErrorKind::InvalidConfig);
    assert_eq!(supervisor.state("kit-escape").status, McpStatus::Failed);

    // A workspace server runs in its workspace root.
    let mut spec = stdio_spec("project-mcp", "normal");
    spec.args = vec!["-u".into(), "scripts/mcp.py".into(), "normal".into()];
    store
        .add(&spec.into_config(McpSource::workspace("ws_1")))
        .unwrap();
    let tools = supervisor.list_tools("project-mcp", &cancel()).unwrap();
    assert_eq!(tools.len(), 7);

    // Removing a server deletes its scratch home.
    let home = scratch.path().join("scratch/kit-echo");
    assert!(home.is_dir());
    supervisor.forget("kit-echo");
    assert!(!home.exists());
    supervisor.forget("project-mcp");
    assert!(!scratch.path().join("scratch/project-mcp").exists());
}
