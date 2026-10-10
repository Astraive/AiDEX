//! `events.*` over IPC v2: replay across restarts, push subscriptions, reconnect,
//! retention errors, slow consumers, and import of PR #40's superseded log.

mod common;

use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use aidex_events::{
    Cursor, EventEnvelope, EventKind, EventLog, EventScope, RetentionPolicy, SubscriptionItem,
};
use aidexd::{
    Client, ErrorCode, TransportConfig,
    methods::events::{
        EVENT_NOTIFICATION, LATEST, LatestCursor, REPLAY, ReplayPage, SUBSCRIBE, StreamWatermark,
        Subscribed, WATERMARK_NOTIFICATION, decode_event,
    },
    services::{LEGACY_EVENT_FILE, event_log_path, event_retention},
};
use common::{PATIENCE, Raw, options, request, start};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

fn id(value: &str) -> EntityId {
    EntityId::new(value).unwrap()
}

fn task(task: &str) -> EventKind {
    EventKind::TaskStarted { task_id: id(task) }
}

fn scope() -> EventScope {
    EventScope::session(id("ws_main"), id("ses_one"))
}

/// Start a daemon and return it with a handle to its own event log.
fn start_with_log(root: &Path, options: aidexd::DaemonOptions) -> (aidexd::DaemonHandle, EventLog) {
    let mut log = None;
    let daemon = start(root, options, |daemon| {
        log = Some(daemon.services().events().clone());
    });
    (daemon, log.unwrap())
}

fn replay(client: &mut Client, params: Value) -> ReplayPage {
    client.call_as(REPLAY, params).unwrap()
}

fn names(page: &ReplayPage) -> Vec<(u64, &'static str)> {
    page.events
        .iter()
        .map(|event| (event.cursor.get(), event.name()))
        .collect()
}

#[test]
fn replay_survives_a_daemon_restart_with_one_global_cursor() {
    let root = common::temp_drive();
    let (daemon, log) = start_with_log(root.path(), options());
    log.append(scope(), task("a")).unwrap();
    log.append(scope(), task("b")).unwrap();
    let mut client = Client::connect(root.path(), "test-events").unwrap();
    assert_eq!(
        names(&replay(&mut client, json!({}))),
        [
            (1, "daemon.started"),
            (2, "task.started"),
            (3, "task.started")
        ]
    );
    let page = replay(&mut client, json!({"after": 1, "limit": 1}));
    assert_eq!(names(&page), [(2, "task.started")]);
    assert!(page.has_more);
    drop(client);
    drop(log);
    daemon.stop();
    daemon.wait().unwrap();

    // The log lives in the drive's session directory.
    let paths = DrivePaths::open(root.path()).unwrap();
    assert!(event_log_path(&paths).is_file());

    let (restarted, _log) = start_with_log(root.path(), options());
    let mut client = Client::connect(root.path(), "test-events").unwrap();
    let page = replay(&mut client, json!({"after": 0}));
    assert_eq!(
        names(&page),
        [
            (1, "daemon.started"),
            (2, "task.started"),
            (3, "task.started"),
            (4, "daemon.stopping"),
            (5, "daemon.started"),
        ]
    );
    let workspace_only = replay(
        &mut client,
        json!({"filter": {"workspace_id": "ws_main", "kinds": ["task"]}}),
    );
    assert_eq!(
        names(&workspace_only),
        [(2, "task.started"), (3, "task.started")]
    );
    assert_eq!(workspace_only.next_cursor, Cursor::new(5));
    let latest: LatestCursor = client.call_as(LATEST, Value::Null).unwrap();
    assert_eq!(latest.latest_cursor, Cursor::new(5));
    assert_eq!(latest.oldest_cursor, Some(Cursor::new(1)));
    restarted.stop();
    restarted.wait().unwrap();
}

fn next_item(subscription: &mut aidexd::Subscription<'_>) -> SubscriptionItem {
    let deadline = Instant::now() + PATIENCE;
    loop {
        assert!(Instant::now() < deadline, "no event within {PATIENCE:?}");
        if let Some(notification) = subscription
            .next_timeout(Duration::from_millis(100))
            .unwrap()
        {
            return match notification.method.as_str() {
                EVENT_NOTIFICATION => SubscriptionItem::Event(decode_event(notification.params).unwrap()),
                WATERMARK_NOTIFICATION => {
                    let watermark: StreamWatermark = serde_json::from_value(notification.params).unwrap();
                    SubscriptionItem::Watermark(watermark.cursor)
                }
                method => panic!("unexpected stream notification: {method}"),
            };
        }
    }
}

fn next_event(subscription: &mut aidexd::Subscription<'_>) -> EventEnvelope {
    loop {
        if let SubscriptionItem::Event(event) = next_item(subscription) {
            return event;
        }
    }
}

#[test]
fn subscribe_pushes_history_then_live_events_and_resumes_without_duplicates() {
    let root = common::temp_drive();
    let (daemon, log) = start_with_log(root.path(), options());
    for index in 0..3 {
        log.append(scope(), task(&format!("t{index}"))).unwrap();
    }
    let mut client = common::client(daemon.endpoint());
    {
        let mut subscription = client.subscribe(SUBSCRIBE, json!({"after": 0})).unwrap();
        let initial: Subscribed = serde_json::from_value(subscription.initial().clone()).unwrap();
        assert_eq!(initial.latest_cursor, Cursor::new(4));
        let history: Vec<_> = (0..4)
            .map(|_| next_event(&mut subscription).cursor.get())
            .collect();
        assert_eq!(history, [1, 2, 3, 4]);
        log.append(
            scope(),
            EventKind::AgentDelta {
                text: "live only".into(),
            },
        )
        .unwrap();
        log.append(scope(), task("live")).unwrap();
        let delta = next_event(&mut subscription);
        assert_eq!(
            (delta.cursor, delta.name()),
            (Cursor::new(4), "agent.delta")
        );
        let live = next_event(&mut subscription);
        assert_eq!((live.cursor, live.name()), (Cursor::new(5), "task.started"));
        // Receive one more event but disconnect before acknowledging it.
        log.append(scope(), task("unacknowledged")).unwrap();
        assert_eq!(next_event(&mut subscription).cursor, Cursor::new(6));
        subscription.unsubscribe().unwrap();
    }
    let acknowledged = Cursor::new(5);
    log.append(scope(), task("while_away")).unwrap();
    let mut subscription = client
        .subscribe(
            SUBSCRIBE,
            json!({"after": acknowledged, "filter": {"kinds": ["task"]}}),
        )
        .unwrap();
    let resumed: Vec<_> = (0..2)
        .map(|_| next_event(&mut subscription).cursor.get())
        .collect();
    assert_eq!(resumed, [6, 7]);
    assert!(
        subscription
            .next_timeout(Duration::from_millis(200))
            .unwrap()
            .is_none()
    );
    drop(subscription);
    assert_eq!(
        client.call("daemon.ping", Value::Null).unwrap()["pong"],
        true
    );
}

#[test]
fn filtered_watermarks_follow_matches_and_survive_retention_and_restart() {
    let root = common::temp_drive();
    let (daemon, log) = start_with_log(root.path(), options());
    let first = log.append(scope(), task("first")).unwrap();
    log.append(EventScope::workspace(id("ws_other")), task("history_skip")).unwrap();
    let mut client = common::client(daemon.endpoint());
    let mut subscription = client
        .subscribe(SUBSCRIBE, json!({"filter": {"workspace_id": "ws_main"}}))
        .unwrap();
    // The initial latest_cursor is metadata, not an acknowledgement. The pending
    // matching event must arrive before the history watermark, never behind it.
    assert_eq!(next_item(&mut subscription), SubscriptionItem::Event(first.clone()));
    assert_eq!(
        next_item(&mut subscription),
        SubscriptionItem::Watermark(Cursor::new(3))
    );
    let mut acknowledged = Cursor::new(3);
    for cursor in 4..=10 {
        log.append(EventScope::workspace(id("ws_other")), task(&format!("skip{cursor}"))).unwrap();
        assert_eq!(
            next_item(&mut subscription),
            SubscriptionItem::Watermark(Cursor::new(cursor))
        );
        acknowledged = Cursor::new(cursor);
    }
    let delta = log.append(scope(), EventKind::AgentDelta { text: "partial".into() }).unwrap();
    assert_eq!(next_item(&mut subscription), SubscriptionItem::Event(delta));
    assert!(subscription.next_timeout(Duration::from_millis(250)).unwrap().is_none());
    subscription.unsubscribe().unwrap();
    drop(client);

    // Deterministic age retention: the next append removes all this consumed
    // history without needing 100 000 appends or sleeping for ninety days.
    let paths = DrivePaths::open(root.path()).unwrap();
    let age_ms = i64::try_from(event_retention().max_age.unwrap().as_millis()).unwrap() + 60_000;
    rusqlite::Connection::open(event_log_path(&paths))
        .unwrap()
        .execute("UPDATE event_log SET occurred_ms = occurred_ms - ?1", [age_ms])
        .unwrap();
    log.prune().unwrap();
    assert!(matches!(
        log.subscribe(aidex_events::EventFilter::for_workspace(id("ws_main")), first.cursor),
        Err(aidex_events::EventError::CursorExpired { .. })
    ));
    drop(log);
    daemon.stop();
    daemon.wait().unwrap();

    let (restarted, log) = start_with_log(root.path(), options());
    let mut client = common::client(restarted.endpoint());
    let mut subscription = client
        .subscribe(SUBSCRIBE, json!({
            "after": acknowledged,
            "filter": {"workspace_id": "ws_main"}
        }))
        .unwrap();
    assert_eq!(
        next_item(&mut subscription),
        SubscriptionItem::Watermark(Cursor::new(12))
    );
    let resumed = log.append(scope(), task("resumed")).unwrap();
    assert_eq!(next_item(&mut subscription), SubscriptionItem::Event(resumed));
    assert!(subscription.next_timeout(Duration::from_millis(250)).unwrap().is_none());
    drop(subscription);
    restarted.stop();
    restarted.wait().unwrap();
}

#[test]
fn invalid_and_expired_cursors_are_explicit_errors() {
    let root = common::temp_drive();
    let paths = DrivePaths::open(root.path()).unwrap();
    paths.ensure_dir(&paths.sessions()).unwrap();
    let path = event_log_path(&paths);
    {
        let log = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
        for index in 0..3 {
            log.append(scope(), task(&format!("old{index}"))).unwrap();
        }
    }
    // Age the history beyond the daemon's retention window.
    let age_ms = i64::try_from(event_retention().max_age.unwrap().as_millis()).unwrap() + 60_000;
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE event_log SET occurred_ms = occurred_ms - ?1",
            [age_ms],
        )
        .unwrap();

    let daemon = start(root.path(), options(), |_| {});
    let mut client = common::client(daemon.endpoint());
    for method in [REPLAY, SUBSCRIBE] {
        let error = client.call(method, json!({"after": 1})).unwrap_err();
        assert_eq!(error.rpc_kind(), Some(ErrorCode::NotFound), "{method}");
        assert!(error.to_string().contains("expired"), "{error}");
        let aidexd::ClientError::Rpc { data, .. } = error else {
            unreachable!()
        };
        let data = data.unwrap();
        assert_eq!(data["reason"], "cursor_expired");
        assert_eq!(data["oldest_available"], 4);
    }
    // Resuming from the newest pruned cursor is fine: nothing was missed.
    let page = replay(&mut client, json!({"after": 3}));
    assert_eq!(names(&page), [(4, "daemon.started")]);
    let ahead = client.call(REPLAY, json!({"after": 99})).unwrap_err();
    assert_eq!(ahead.rpc_kind(), Some(ErrorCode::InvalidParams));
    let typo = client
        .call(SUBSCRIBE, json!({"filter": {"kinds": ["tsk"]}}))
        .unwrap_err();
    assert_eq!(typo.rpc_kind(), Some(ErrorCode::InvalidParams));
}

#[test]
fn a_subscriber_that_stops_reading_is_dropped_without_blocking_appends() {
    let root = common::temp_drive();
    let mut options = options();
    options.transport = TransportConfig {
        max_queued_messages: 16,
        max_queued_bytes: 256 * 1024,
        write_timeout: Duration::from_secs(1),
        ..options.transport
    };
    let (daemon, log) = start_with_log(root.path(), options);
    let mut slow = Raw::authenticated(daemon.endpoint());
    slow.send(&request(1, SUBSCRIBE, json!({"after": 0})));
    assert_eq!(slow.recv()["id"], 1);
    // Never read again. Appends stay fast while the stream overflows.
    let started = Instant::now();
    let padding = "p".repeat(8 * 1024);
    for index in 0..1000 {
        log.append(
            scope(),
            EventKind::TaskFailed {
                task_id: id(&format!("t{index}")),
                reason: padding.clone(),
            },
        )
        .unwrap();
    }
    assert!(started.elapsed() < Duration::from_secs(120));
    slow.expect_closed();
    // Other clients are unaffected and can resume from any retained cursor.
    let mut other = common::client(daemon.endpoint());
    assert_eq!(
        other.call("daemon.ping", Value::Null).unwrap()["pong"],
        true
    );
    let page = replay(&mut other, json!({"after": 990}));
    assert_eq!(page.events.len(), 11);
    assert_eq!(page.events.last().unwrap().cursor, Cursor::new(1001));
}

#[test]
fn the_superseded_pr40_log_is_imported_once_and_left_untouched() {
    let root = common::temp_drive();
    let runtime = aidexd::endpoint::ensure_state_dir(root.path()).unwrap();
    let legacy = runtime.join(LEGACY_EVENT_FILE);
    {
        let connection = rusqlite::Connection::open(&legacy).unwrap();
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 CREATE TABLE IF NOT EXISTS events (
                     workspace_id TEXT NOT NULL,
                     sequence INTEGER NOT NULL,
                     session_id TEXT,
                     body TEXT NOT NULL,
                     PRIMARY KEY(workspace_id, sequence)
                 );",
            )
            .unwrap();
        for (sequence, kind) in [
            (1, r#"{"type":"session_started"}"#),
            (2, r#"{"type":"task_started","task_id":"t1"}"#),
        ] {
            connection
                .execute(
                    "INSERT INTO events VALUES ('workspace', ?1, 'session', ?2)",
                    rusqlite::params![
                        sequence,
                        format!(
                            r#"{{"version":1,"sequence":{sequence},"workspace_id":"workspace","session_id":"session","kind":{kind}}}"#
                        )
                    ],
                )
                .unwrap();
        }
    }
    let before = fs::read(&legacy).unwrap();
    let daemon = start(root.path(), options(), |_| {});
    let mut client = common::client(daemon.endpoint());
    let page = replay(&mut client, json!({}));
    assert_eq!(
        names(&page),
        [
            (1, "session.started"),
            (2, "task.started"),
            (3, "daemon.started")
        ]
    );
    assert_eq!(
        page.events[0].scope,
        EventScope::session(id("workspace"), id("session"))
    );
    drop(client);
    daemon.stop();
    daemon.wait().unwrap();
    assert_eq!(fs::read(&legacy).unwrap(), before);

    // A restart does not import again.
    let restarted = start(root.path(), options(), |_| {});
    let mut client = common::client(restarted.endpoint());
    let latest: LatestCursor = client.call_as(LATEST, Value::Null).unwrap();
    assert_eq!(latest.latest_cursor, Cursor::new(5));
}
