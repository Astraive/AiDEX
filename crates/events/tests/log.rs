//! Durable log behaviour: global cursor, replay, retention, restart, schema safety.

mod common;

use aidex_events::{
    Cursor, EventError, EventFilter, EventKind, EventLog, EventScope, MAX_PAGE_SIZE, MessageRole,
    RetentionPolicy,
};
use common::{db_path, id, session_scope, task_started, temp_log, ws};
use std::{sync::Arc, thread, time::Duration};

fn cursors(page: &aidex_events::ReplayPage) -> Vec<u64> {
    page.events.iter().map(|event| event.cursor.get()).collect()
}

#[test]
fn age_retention_stops_at_fresh_events_after_clock_rollback() {
    for (expired, max_events, pruned) in [
        ([false, true, false, true], None, 0),
        ([true, false, true, false], None, 1),
        ([true, false, true, false], Some(2), 3),
        ([true, true, true, true], None, 4),
    ] {
        let retention = RetentionPolicy {
            max_events,
            max_age: Some(Duration::from_secs(3600)),
        };
        let dir = tempfile::tempdir().unwrap();
        let path = db_path(dir.path());
        let log = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
        for index in 1..=4 {
            log.append(session_scope(), task_started(&format!("t{index}")))
                .unwrap();
        }
        drop(log);
        let connection = rusqlite::Connection::open(&path).unwrap();
        for (index, expired) in expired.into_iter().enumerate() {
            if expired {
                connection
                    .execute(
                        "UPDATE event_log SET occurred_ms = 0 WHERE cursor = ?1",
                        [index as i64 + 1],
                    )
                    .unwrap();
            }
        }
        drop(connection);
        let log = EventLog::open(&path, retention).unwrap();
        let page = log
            .replay(&EventFilter::all(), Cursor::new(pruned), 10)
            .unwrap();
        assert_eq!(cursors(&page), (pruned + 1..=4).collect::<Vec<_>>());
        assert_eq!(log.latest_cursor().unwrap(), Cursor::new(4));
        assert_eq!(log.prune().unwrap(), 0);
        if pruned > 0 {
            assert!(matches!(
                log.replay(&EventFilter::all(), Cursor::new(pruned - 1), 10),
                Err(EventError::CursorExpired { .. })
            ));
        }
    }
}

#[test]
fn concurrent_appenders_get_strictly_increasing_gap_free_cursors() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let log = Arc::new(fixture.log.clone());
    let threads: Vec<_> = (0..8)
        .map(|writer| {
            let log = Arc::clone(&log);
            thread::spawn(move || {
                (0..50)
                    .map(|index| {
                        log.append(
                            EventScope::workspace(id(&format!("ws_{writer}"))),
                            task_started(&format!("task_{writer}_{index}")),
                        )
                        .unwrap()
                        .cursor
                        .get()
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut all = Vec::new();
    for handle in threads {
        let mine = handle.join().unwrap();
        // Each writer observes its own appends in increasing order.
        assert!(mine.windows(2).all(|pair| pair[0] < pair[1]), "{mine:?}");
        all.extend(mine);
    }
    all.sort_unstable();
    assert_eq!(all, (1..=400).collect::<Vec<_>>());
    assert_eq!(fixture.log.latest_cursor().unwrap(), Cursor::new(400));
    // Replay order is cursor order across every scope.
    let mut after = Cursor::ZERO;
    let mut replayed = Vec::new();
    loop {
        let page = fixture
            .log
            .replay(&EventFilter::all(), after, MAX_PAGE_SIZE)
            .unwrap();
        replayed.extend(cursors(&page));
        after = page.next_cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(replayed, (1..=400).collect::<Vec<_>>());
}

#[test]
fn replay_returns_exactly_the_events_after_the_cursor() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    for index in 1..=10 {
        fixture
            .log
            .append(session_scope(), task_started(&format!("t{index}")))
            .unwrap();
    }
    let page = fixture
        .log
        .replay(&EventFilter::all(), Cursor::new(4), 3)
        .unwrap();
    assert_eq!(cursors(&page), [5, 6, 7]);
    assert!(page.has_more);
    assert_eq!(page.next_cursor, Cursor::new(7));
    let rest = fixture
        .log
        .replay(&EventFilter::all(), page.next_cursor, 100)
        .unwrap();
    assert_eq!(cursors(&rest), [8, 9, 10]);
    assert!(!rest.has_more);
    assert_eq!(rest.next_cursor, Cursor::new(10));
    let empty = fixture
        .log
        .replay(&EventFilter::all(), Cursor::new(10), 100)
        .unwrap();
    assert!(empty.events.is_empty());
    assert_eq!(empty.next_cursor, Cursor::new(10));
}

#[test]
fn filters_select_by_scope_and_kind_prefix_and_skip_ahead() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let log = &fixture.log;
    let other = EventScope::session(id("ws_other"), id("ses_two"));
    log.append(
        EventScope::global(),
        EventKind::DaemonStarted {
            pid: 1,
            version: "0".into(),
        },
    )
    .unwrap(); // 1
    log.append(session_scope(), task_started("a")).unwrap(); // 2
    log.append(other.clone(), task_started("b")).unwrap(); // 3
    log.append(
        session_scope(),
        EventKind::AgentSubagentCreated {
            child_session_id: id("ses_child"),
            role: "reviewer".into(),
        },
    )
    .unwrap(); // 4
    log.append(
        session_scope(),
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: "hi".into(),
        },
    )
    .unwrap(); // 5
    log.append(other, EventKind::TaskCompleted { task_id: id("b") })
        .unwrap(); // 6

    let replay = |filter: EventFilter| log.replay(&filter, Cursor::ZERO, 100).unwrap();
    assert_eq!(
        cursors(&replay(EventFilter::for_workspace(ws()))),
        [2, 4, 5]
    );
    assert_eq!(
        cursors(&replay(EventFilter::for_session(id("ses_two")))),
        [3, 6]
    );
    assert_eq!(
        cursors(&replay(EventFilter::all().with_kind("task"))),
        [2, 3, 6]
    );
    assert_eq!(
        cursors(&replay(EventFilter::all().with_kind("agent.subagent"))),
        [4]
    );
    assert_eq!(
        cursors(&replay(
            EventFilter::all()
                .with_kind("daemon")
                .with_kind("task.completed")
        )),
        [1, 6]
    );
    // A filtered page that found nothing more still advances to the latest cursor.
    let page = replay(EventFilter::for_workspace(ws()).with_kind("task"));
    assert_eq!(cursors(&page), [2]);
    assert_eq!(page.next_cursor, Cursor::new(6));
    // Kind prefixes match whole segments only and must exist in the catalog.
    for bad in ["tas", "task.", "", "Task", "task.*", "nope"] {
        assert!(
            matches!(
                log.replay(&EventFilter::all().with_kind(bad), Cursor::ZERO, 10),
                Err(EventError::InvalidFilter(_))
            ),
            "{bad:?}"
        );
    }
}

#[test]
fn replay_rejects_invalid_limits_and_cursors() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    fixture
        .log
        .append(session_scope(), task_started("a"))
        .unwrap();
    let all = EventFilter::all();
    assert!(matches!(
        fixture.log.replay(&all, Cursor::ZERO, 0),
        Err(EventError::InvalidLimit { .. })
    ));
    assert!(matches!(
        fixture.log.replay(&all, Cursor::ZERO, MAX_PAGE_SIZE + 1),
        Err(EventError::InvalidLimit { .. })
    ));
    assert_eq!(
        fixture.log.replay(&all, Cursor::new(u64::MAX), 1),
        Err(EventError::InvalidCursor)
    );
    // A cursor this log never assigned would silently skip future events.
    assert_eq!(
        fixture.log.replay(&all, Cursor::new(2), 1),
        Err(EventError::CursorAhead {
            requested: Cursor::new(2),
            latest: Cursor::new(1)
        })
    );
}

#[test]
fn retention_by_count_reports_expired_cursors_instead_of_gaps() {
    let fixture = temp_log(RetentionPolicy::max_events(3));
    for index in 1..=5 {
        fixture
            .log
            .append(session_scope(), task_started(&format!("t{index}")))
            .unwrap();
    }
    assert_eq!(fixture.log.oldest_cursor().unwrap(), Some(Cursor::new(3)));
    let expired = EventError::CursorExpired {
        requested: Cursor::new(1),
        oldest_available: Cursor::new(3),
    };
    assert_eq!(
        fixture.log.replay(&EventFilter::all(), Cursor::new(1), 10),
        Err(expired.clone())
    );
    assert_eq!(
        fixture
            .log
            .subscribe(EventFilter::all(), Cursor::new(1))
            .err(),
        Some(expired)
    );
    assert_eq!(
        cursors(
            &fixture
                .log
                .replay(&EventFilter::all(), Cursor::new(2), 10)
                .unwrap()
        ),
        [3, 4, 5]
    );
}

#[test]
fn retention_by_age_prunes_a_prefix_on_open_and_keeps_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    {
        let log = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
        for index in 1..=3 {
            log.append(session_scope(), task_started(&format!("t{index}")))
                .unwrap();
        }
    }
    // Age the stored events by two hours.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE event_log SET occurred_ms = occurred_ms - 7200000",
            [],
        )
        .unwrap();
    let retention = RetentionPolicy {
        max_events: None,
        max_age: Some(Duration::from_secs(3600)),
    };
    let log = EventLog::open(&path, retention).unwrap();
    assert_eq!(log.oldest_cursor().unwrap(), None);
    assert_eq!(log.latest_cursor().unwrap(), Cursor::new(3));
    assert!(matches!(
        log.replay(&EventFilter::all(), Cursor::new(2), 10),
        Err(EventError::CursorExpired { .. })
    ));
    // A consumer that had seen everything resumes without error.
    assert!(
        log.replay(&EventFilter::all(), Cursor::new(3), 10)
            .unwrap()
            .events
            .is_empty()
    );
    // Cursors never restart after everything was pruned.
    let next = log.append(session_scope(), task_started("t4")).unwrap();
    assert_eq!(next.cursor, Cursor::new(4));
    assert_eq!(log.prune().unwrap(), 0);
}

#[test]
fn retention_policy_is_validated() {
    let dir = tempfile::tempdir().unwrap();
    for retention in [
        RetentionPolicy::max_events(0),
        RetentionPolicy {
            max_events: None,
            max_age: Some(Duration::ZERO),
        },
    ] {
        assert!(matches!(
            EventLog::open(db_path(dir.path()), retention),
            Err(EventError::InvalidRetention(_))
        ));
    }
}

#[test]
fn restart_continues_the_cursor_and_replays_persisted_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    let first = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
    first.append(session_scope(), task_started("one")).unwrap();
    first.append(session_scope(), task_started("two")).unwrap();
    drop(first);
    let restarted = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
    assert_eq!(restarted.latest_cursor().unwrap(), Cursor::new(2));
    let third = restarted
        .append(session_scope(), task_started("three"))
        .unwrap();
    assert_eq!(third.cursor, Cursor::new(3));
    let page = restarted
        .replay(&EventFilter::all(), Cursor::ZERO, 10)
        .unwrap();
    assert_eq!(cursors(&page), [1, 2, 3]);
    assert_eq!(page.events[0].event, task_started("one"));
}

#[test]
fn ephemeral_events_are_never_stored_and_consume_no_cursor() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    fixture
        .log
        .append(session_scope(), task_started("a"))
        .unwrap();
    let delta = fixture
        .log
        .append(
            session_scope(),
            EventKind::AgentDelta { text: "tok".into() },
        )
        .unwrap();
    assert_eq!(delta.cursor, Cursor::new(1));
    let next = fixture
        .log
        .append(session_scope(), task_started("b"))
        .unwrap();
    assert_eq!(next.cursor, Cursor::new(2));
    let page = fixture
        .log
        .replay(&EventFilter::all(), Cursor::ZERO, 10)
        .unwrap();
    assert_eq!(cursors(&page), [1, 2]);
    assert!(page.events.iter().all(|event| event.is_durable()));
}

#[test]
fn newer_schemas_and_foreign_databases_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    drop(EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap());
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("PRAGMA journal_mode=DELETE; UPDATE schema_version SET version = 99;")
        .unwrap();
    let newer_before = std::fs::read(&path).unwrap();
    assert_eq!(
        EventLog::open(&path, RetentionPolicy::UNBOUNDED).err(),
        Some(EventError::IncompatibleSchema {
            found: 99,
            supported: 2
        })
    );
    assert_eq!(std::fs::read(&path).unwrap(), newer_before);

    let foreign = dir.path().join("other.sqlite3");
    rusqlite::Connection::open(&foreign)
        .unwrap()
        .execute_batch("CREATE TABLE kv (key TEXT, value TEXT);")
        .unwrap();
    let foreign_before = std::fs::read(&foreign).unwrap();
    assert!(matches!(
        EventLog::open(&foreign, RetentionPolicy::UNBOUNDED),
        Err(EventError::Migration(_))
    ));
    assert_eq!(std::fs::read(&foreign).unwrap(), foreign_before);
    for database in [&path, &foreign] {
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = database.as_os_str().to_os_string();
            sidecar.push(suffix);
            assert!(!std::path::Path::new(&sidecar).exists());
        }
    }
    // The foreign database was not modified.
    let tables: i64 = rusqlite::Connection::open(&foreign)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);

    assert!(matches!(
        EventLog::open(
            dir.path().join("missing").join("events.sqlite3"),
            RetentionPolicy::UNBOUNDED
        ),
        Err(EventError::Storage(_))
    ));
}

#[cfg(unix)]
#[test]
fn symlinked_database_paths_are_refused() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.sqlite3");
    drop(EventLog::open(&real, RetentionPolicy::UNBOUNDED).unwrap());
    let link = db_path(dir.path());
    symlink(&real, &link).unwrap();
    assert!(matches!(
        EventLog::open(&link, RetentionPolicy::UNBOUNDED),
        Err(EventError::UnsafePath(_))
    ));
    let sidecar_target = dir.path().join("elsewhere");
    std::fs::write(&sidecar_target, b"").unwrap();
    let fresh = dir.path().join("fresh.sqlite3");
    symlink(&sidecar_target, dir.path().join("fresh.sqlite3-wal")).unwrap();
    assert!(matches!(
        EventLog::open(&fresh, RetentionPolicy::UNBOUNDED),
        Err(EventError::UnsafePath(_))
    ));
}

#[test]
fn errors_map_to_stable_service_codes() {
    use aidex_core::ServiceErrorCode;
    assert_eq!(
        EventError::CursorExpired {
            requested: Cursor::ZERO,
            oldest_available: Cursor::new(2)
        }
        .code(),
        ServiceErrorCode::NotFound
    );
    assert_eq!(
        EventError::UnsupportedVersion { found: 9 }.code(),
        ServiceErrorCode::IncompatibleVersion
    );
    assert_eq!(
        EventError::SensitiveIdentifier.code(),
        ServiceErrorCode::InvalidArgument
    );
    assert_eq!(
        EventError::Storage("x".into()).code(),
        ServiceErrorCode::Internal
    );
}
