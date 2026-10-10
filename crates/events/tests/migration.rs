//! Migration of PR #40 (event log v1) databases.

mod common;

use aidex_events::{
    Cursor, EventFilter, EventKind, EventLog, EventScope, LegacyApprovalDecision, LegacyImport,
    LegacyRecord, Outcome, RetentionPolicy, SubscriptionItem,
};
use common::{db_path, id, write_v1_database};
use std::fs;

/// One row of every PR #40 kind, across two workspaces, in write order.
const V1_ROWS: &[(&str, Option<&str>, &str)] = &[
    ("ws_a", Some("ses_1"), r#"{"type":"session_started"}"#),
    (
        "ws_a",
        Some("ses_1"),
        r#"{"type":"task_started","task_id":"t1"}"#,
    ),
    (
        "ws_b",
        None,
        r#"{"type":"task_finished","task_id":"t2","outcome":"succeeded"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"task_finished","task_id":"t3","outcome":"cancelled"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"task_finished","task_id":"t4","outcome":"failed"}"#,
    ),
    (
        "ws_a",
        Some("ses_1"),
        r#"{"type":"agent_started","agent_id":"omp"}"#,
    ),
    (
        "ws_a",
        Some("ses_1"),
        r#"{"type":"agent_finished","agent_id":"omp","outcome":"failed"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"tool_started","tool_id":"fs.read"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"tool_finished","tool_id":"fs.read","outcome":"succeeded"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"approval_requested","approval_id":"apr_1"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"approval_resolved","approval_id":"apr_1","decision":"approved"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"approval_resolved","approval_id":"apr_2","decision":"denied"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"approval_resolved","approval_id":"apr_3","decision":"expired"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"process_started","process_id":"p1"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"process_finished","process_id":"p1","exit_code":3}"#,
    ),
    ("ws_a", None, r#"{"type":"model_started","model_id":"m1"}"#),
    (
        "ws_a",
        None,
        r#"{"type":"model_finished","model_id":"m1","outcome":"succeeded"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"cleanup_started","cleanup_id":"c1"}"#,
    ),
    (
        "ws_a",
        None,
        r#"{"type":"cleanup_finished","cleanup_id":"c1","outcome":"succeeded"}"#,
    ),
    ("ws_b", Some("ses_9"), r#"{"type":"session_finished"}"#),
];

fn legacy(record: LegacyRecord, subject: Option<&str>) -> EventKind {
    EventKind::LegacyV1 {
        record,
        subject_id: subject.map(id),
        outcome: None,
        decision: None,
        exit_code: None,
    }
}

fn legacy_outcome(record: LegacyRecord, subject: &str, outcome: Outcome) -> EventKind {
    EventKind::LegacyV1 {
        record,
        subject_id: Some(id(subject)),
        outcome: Some(outcome),
        decision: None,
        exit_code: None,
    }
}

fn expected() -> Vec<(EventScope, EventKind)> {
    let a = || EventScope::workspace(id("ws_a"));
    let a1 = || EventScope::session(id("ws_a"), id("ses_1"));
    vec![
        (a1(), EventKind::SessionStarted {}),
        (a1(), EventKind::TaskStarted { task_id: id("t1") }),
        (
            EventScope::workspace(id("ws_b")),
            EventKind::TaskCompleted { task_id: id("t2") },
        ),
        (a(), EventKind::TaskCancelled { task_id: id("t3") }),
        (
            a(),
            legacy_outcome(LegacyRecord::TaskFinished, "t4", Outcome::Failed),
        ),
        (
            a1(),
            EventKind::AgentStarted {
                agent_id: id("omp"),
                model: None,
            },
        ),
        (
            a1(),
            EventKind::AgentFinished {
                agent_id: id("omp"),
                outcome: Outcome::Failed,
            },
        ),
        (a(), legacy(LegacyRecord::ToolStarted, Some("fs.read"))),
        (
            a(),
            legacy_outcome(LegacyRecord::ToolFinished, "fs.read", Outcome::Succeeded),
        ),
        (a(), legacy(LegacyRecord::ApprovalRequested, Some("apr_1"))),
        (
            a(),
            EventKind::LegacyV1 {
                record: LegacyRecord::ApprovalResolved,
                subject_id: Some(id("apr_1")),
                outcome: None,
                decision: Some(LegacyApprovalDecision::Approved),
                exit_code: None,
            },
        ),
        (
            a(),
            EventKind::ApprovalDenied {
                approval_id: id("apr_2"),
                reason: None,
            },
        ),
        (
            a(),
            EventKind::ApprovalExpired {
                approval_id: id("apr_3"),
            },
        ),
        (a(), legacy(LegacyRecord::ProcessStarted, Some("p1"))),
        (
            a(),
            EventKind::ProcessExited {
                process_id: id("p1"),
                exit_code: Some(3),
                signal: None,
            },
        ),
        (a(), legacy(LegacyRecord::ModelStarted, Some("m1"))),
        (
            a(),
            legacy_outcome(LegacyRecord::ModelFinished, "m1", Outcome::Succeeded),
        ),
        (
            a(),
            EventKind::CleanupStarted {
                cleanup_id: id("c1"),
            },
        ),
        (
            a(),
            legacy_outcome(LegacyRecord::CleanupFinished, "c1", Outcome::Succeeded),
        ),
        (
            EventScope::session(id("ws_b"), id("ses_9")),
            legacy(LegacyRecord::SessionFinished, None),
        ),
    ]
}

fn replay_all(log: &EventLog) -> Vec<(u64, EventScope, EventKind)> {
    log.replay(&EventFilter::all(), Cursor::ZERO, 256)
        .unwrap()
        .events
        .into_iter()
        .map(|event| (event.cursor.get(), event.scope, event.event))
        .collect()
}

fn assert_imported(log: &EventLog) {
    let replayed = replay_all(log);
    let expected = expected();
    assert_eq!(replayed.len(), expected.len());
    for (index, ((cursor, scope, kind), (want_scope, want_kind))) in
        replayed.into_iter().zip(expected).enumerate()
    {
        assert_eq!(cursor, index as u64 + 1);
        assert_eq!((scope, kind), (want_scope, want_kind), "row {cursor}");
    }
}

#[test]
fn a_v1_database_is_migrated_in_place_preserving_write_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    write_v1_database(&path, V1_ROWS);
    let log = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
    assert_imported(&log);
    assert_eq!(
        log.latest_cursor().unwrap(),
        Cursor::new(V1_ROWS.len() as u64)
    );
    let next = log
        .append(
            EventScope::workspace(id("ws_a")),
            common::task_started("t5"),
        )
        .unwrap();
    assert_eq!(next.cursor, Cursor::new(V1_ROWS.len() as u64 + 1));
    drop(log);
    // The v1 table is gone and reopening does not import twice.
    let tables: Vec<String> = rusqlite::Connection::open(&path)
        .unwrap()
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!tables.contains(&"events".to_owned()), "{tables:?}");
    let reopened = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
    assert_eq!(
        reopened.latest_cursor().unwrap(),
        Cursor::new(V1_ROWS.len() as u64 + 1)
    );
}

#[test]
fn a_corrupt_v1_row_fails_the_migration_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = db_path(dir.path());
    write_v1_database(
        &path,
        &[
            ("ws_a", None, r#"{"type":"task_started","task_id":"t1"}"#),
            (
                "ws_a",
                None,
                r#"{"type":"task_started","task_id":"bad id"}"#,
            ),
        ],
    );
    assert!(matches!(
        EventLog::open(&path, RetentionPolicy::UNBOUNDED),
        Err(aidex_events::EventError::Migration(_))
    ));
    let rows: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 2);
}

#[test]
fn a_separate_v1_file_is_imported_once_read_only() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_path = dir.path().join("legacy.sqlite");
    write_v1_database(&legacy_path, V1_ROWS);
    let before = fs::read(&legacy_path).unwrap();

    let log = EventLog::open(db_path(dir.path()), RetentionPolicy::UNBOUNDED).unwrap();
    assert_eq!(
        log.import_legacy(&legacy_path).unwrap(),
        LegacyImport::Imported {
            events: V1_ROWS.len() as u64
        }
    );
    assert_imported(&log);
    assert_eq!(
        log.import_legacy(&legacy_path).unwrap(),
        LegacyImport::AlreadyImported
    );
    assert_eq!(
        fs::read(&legacy_path).unwrap(),
        before,
        "v1 file was modified"
    );

    // A log that already has its own history never imports, to keep cursor order honest.
    let other =
        EventLog::open(dir.path().join("other.sqlite3"), RetentionPolicy::UNBOUNDED).unwrap();
    other
        .append(EventScope::global(), EventKind::DriveSafeToRemove {})
        .unwrap();
    assert_eq!(
        other.import_legacy(&legacy_path).unwrap(),
        LegacyImport::SkippedNonEmpty
    );
    assert_eq!(other.latest_cursor().unwrap(), Cursor::new(1));

    // An empty target still refuses a source that is not a v1 log.
    let empty =
        EventLog::open(dir.path().join("empty.sqlite3"), RetentionPolicy::UNBOUNDED).unwrap();
    assert!(matches!(
        empty.import_legacy(&db_path(dir.path())),
        Err(aidex_events::EventError::Migration(_))
    ));
}

#[test]
fn skipped_imports_do_not_open_the_legacy_source() {
    for rows in [V1_ROWS, &[][..]] {
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir.path().join("legacy.sqlite");
        write_v1_database(&legacy_path, rows);
        let path = db_path(dir.path());
        let log = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
        assert_eq!(
            log.import_legacy(&legacy_path).unwrap(),
            LegacyImport::Imported { events: rows.len() as u64 }
        );
        // Neither an invalid source nor a missing source is opened after the marker,
        // including when the first import was empty and last_cursor is still zero.
        fs::write(&legacy_path, b"no longer a database").unwrap();
        assert_eq!(
            log.import_legacy(&legacy_path).unwrap(),
            LegacyImport::AlreadyImported
        );
        fs::remove_file(&legacy_path).unwrap();
        drop(log);
        let reopened = EventLog::open(&path, RetentionPolicy::UNBOUNDED).unwrap();
        assert_eq!(
            reopened.import_legacy(&legacy_path).unwrap(),
            LegacyImport::AlreadyImported
        );
        let nonempty =
            EventLog::open(dir.path().join("nonempty.sqlite3"), RetentionPolicy::UNBOUNDED)
                .unwrap();
        nonempty
            .append(EventScope::global(), EventKind::DriveSafeToRemove {})
            .unwrap();
        assert_eq!(
            nonempty.import_legacy(&legacy_path).unwrap(),
            LegacyImport::SkippedNonEmpty
        );
    }
}

#[test]
fn active_subscribers_catch_up_imported_history_before_later_appends() {
    for become_live in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let legacy_path = dir.path().join("legacy.sqlite");
        let rows: Vec<_> = (0..600)
            .map(|index| {
                (
                    if index % 2 == 0 { "ws_a" } else { "ws_b" },
                    None,
                    r#"{"type":"task_started","task_id":"legacy_task"}"#,
                )
            })
            .collect();
        write_v1_database(&legacy_path, &rows);
        let log = EventLog::open(db_path(dir.path()), RetentionPolicy::UNBOUNDED).unwrap();
        let mut all = log
            .subscribe_with_capacity(EventFilter::all(), Cursor::ZERO, 1)
            .unwrap();
        let mut filtered = log
            .subscribe_with_capacity(EventFilter::for_workspace(id("ws_a")), Cursor::ZERO, 1)
            .unwrap();
        if become_live {
            assert_eq!(all.try_next().unwrap(), None);
            assert_eq!(filtered.try_next().unwrap(), None);
        }
        log.import_legacy(&legacy_path).unwrap();
        log.append(EventScope::global(), EventKind::DriveSafeToRemove {})
            .unwrap();
        let mut seen = Vec::new();
        while let Some(item) = all.try_next().unwrap() {
            if let SubscriptionItem::Event(event) = item {
                seen.push(event.cursor.get());
            }
        }
        assert_eq!(seen, (1..=601).collect::<Vec<_>>());
        let mut seen_filtered = Vec::new();
        while let Some(item) = filtered.try_next().unwrap() {
            if let SubscriptionItem::Event(event) = item {
                seen_filtered.push(event.cursor.get());
            }
        }
        assert_eq!(seen_filtered, (1..=599).step_by(2).collect::<Vec<_>>());
        // Both rejoin live delivery after multiple disk pages, without duplicates.
        let next = log
            .append(EventScope::workspace(id("ws_a")), common::task_started("live_task"))
            .unwrap();
        assert_eq!(next.cursor, Cursor::new(602));
        assert!(matches!(
            all.try_next().unwrap(),
            Some(SubscriptionItem::Event(event)) if event == next
        ));
        assert!(matches!(
            filtered.try_next().unwrap(),
            Some(SubscriptionItem::Event(event)) if event == next
        ));
        assert_eq!(all.try_next().unwrap(), None);
        assert_eq!(filtered.try_next().unwrap(), None);
    }
}

#[test]
fn subscribers_report_expired_import_history_instead_of_skipping_it() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_path = dir.path().join("legacy.sqlite");
    write_v1_database(&legacy_path, V1_ROWS);
    let log = EventLog::open(db_path(dir.path()), RetentionPolicy::max_events(2)).unwrap();
    let mut subscription = log.subscribe(EventFilter::all(), Cursor::ZERO).unwrap();
    assert_eq!(subscription.try_next().unwrap(), None);
    log.import_legacy(&legacy_path).unwrap();
    log.append(EventScope::global(), EventKind::DriveSafeToRemove {})
        .unwrap();
    assert!(matches!(
        subscription.try_next(),
        Err(aidex_events::EventError::CursorExpired { requested: Cursor::ZERO, .. })
    ));
}

#[test]
fn separate_import_applies_retention_before_returning() {
    let dir = tempfile::tempdir().unwrap();
    let legacy_path = dir.path().join("legacy.sqlite");
    write_v1_database(&legacy_path, V1_ROWS);
    let path = db_path(dir.path());
    let log = EventLog::open(&path, RetentionPolicy::max_events(3)).unwrap();
    let imported = V1_ROWS.len() as u64;
    assert_eq!(
        log.import_legacy(&legacy_path).unwrap(),
        LegacyImport::Imported { events: imported }
    );
    assert_eq!(log.latest_cursor().unwrap(), Cursor::new(imported));
    assert_eq!(log.oldest_cursor().unwrap(), Some(Cursor::new(imported - 2)));
    assert!(matches!(
        log.replay(&EventFilter::all(), Cursor::ZERO, 256),
        Err(aidex_events::EventError::CursorExpired { .. })
    ));
    let page = log
        .replay(&EventFilter::all(), Cursor::new(imported - 3), 256)
        .unwrap();
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.cursor.get())
            .collect::<Vec<_>>(),
        [imported - 2, imported - 1, imported]
    );
    // Retention and its cursor boundary are already visible to another connection.
    let connection = rusqlite::Connection::open(&path).unwrap();
    let (count, pruned): (u64, u64) = connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM event_log), pruned_through FROM event_log_state",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((count, pruned), (3, imported - 3));
    let recorded: u64 = connection
        .query_row(
            "SELECT events FROM event_migrations WHERE name = 'v1-file-import'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(recorded, imported);
}
