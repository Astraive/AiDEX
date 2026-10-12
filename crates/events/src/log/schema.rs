//! Database creation, schema versioning, and migration from the PR #40 (v1) layout.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use aidex_core::Timestamp;
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};

use super::{
    RetentionPolicy,
    store::{self, State},
};
use crate::{EventError, catalog::import_v1_body, error::storage, sanitize};

/// Current schema. Version 1 is PR #40's `events(workspace_id, sequence, session_id, body)`.
pub(crate) const SCHEMA_VERSION: i64 = 2;
/// Marker for a v1 database upgraded in place by [`prepare`].
const IN_PLACE_MIGRATION: &str = "v1-in-place";
/// Marker for a separate v1 file imported by `EventLog::import_legacy`.
pub(crate) const FILE_IMPORT_MIGRATION: &str = "v1-file-import";

const CREATE: &str = "
CREATE TABLE schema_version (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    version INTEGER NOT NULL
);
CREATE TABLE event_log_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    last_cursor INTEGER NOT NULL CHECK (last_cursor >= 0),
    pruned_through INTEGER NOT NULL CHECK (pruned_through >= 0)
);
CREATE TABLE event_log (
    cursor INTEGER PRIMARY KEY CHECK (cursor > 0),
    workspace_id TEXT,
    session_id TEXT,
    kind TEXT NOT NULL,
    occurred_ms INTEGER NOT NULL,
    body TEXT NOT NULL
);
CREATE INDEX event_log_by_workspace ON event_log(workspace_id, cursor);
CREATE INDEX event_log_by_session ON event_log(session_id, cursor);
CREATE INDEX event_log_by_kind ON event_log(kind, cursor);
CREATE INDEX event_log_by_time ON event_log(occurred_ms);
CREATE TABLE event_migrations (
    name TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL,
    events INTEGER NOT NULL
);
INSERT INTO event_log_state(id, last_cursor, pruned_through) VALUES (1, 0, 0);
";

const V1_COLUMNS: [&str; 4] = ["workspace_id", "sequence", "session_id", "body"];

/// Refuse a database (or SQLite sidecar) path that is a symbolic link.
pub(crate) fn refuse_symlinks(path: &Path) -> Result<(), EventError> {
    let sidecars = ["-wal", "-shm", "-journal"].map(|suffix| {
        let mut name = OsString::from(path.as_os_str());
        name.push(suffix);
        PathBuf::from(name)
    });
    for candidate in std::iter::once(path.to_path_buf()).chain(sidecars) {
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(EventError::UnsafePath(candidate));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(storage(error)),
        }
    }
    Ok(())
}

/// Open, prepare the schema, then enable WAL with full synchronous commits.
pub(crate) fn open_connection(path: &Path) -> Result<Connection, EventError> {
    refuse_symlinks(path)?;
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() && !parent.is_dir() => {
            return Err(EventError::Storage(format!(
                "event database directory does not exist: {}",
                parent.display()
            )));
        }
        _ => {}
    }
    let mut connection = Connection::open(path).map_err(storage)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage)?;
    prepare(&mut connection)?;
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .map_err(storage)?;
    Ok(connection)
}

fn tables(connection: &Connection) -> Result<BTreeSet<String>, EventError> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .map_err(storage)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage)?;
    names.map(|name| name.map_err(storage)).collect()
}

fn is_v1_layout(connection: &Connection, tables: &BTreeSet<String>) -> Result<bool, EventError> {
    if tables.len() != 1 || !tables.contains("events") {
        return Ok(false);
    }
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('events') ORDER BY cid")
        .map_err(storage)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    Ok(columns == V1_COLUMNS)
}

/// v1 bodies in insertion (rowid) order, which is the global order they were written in.
fn v1_bodies(connection: &Connection) -> Result<Vec<String>, EventError> {
    let mut statement = connection
        .prepare("SELECT body FROM events ORDER BY rowid")
        .map_err(storage)?;
    let bodies = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(storage)?;
    bodies.map(|body| body.map_err(storage)).collect()
}

/// Create, verify, or migrate the schema atomically.
pub(crate) fn prepare(connection: &mut Connection) -> Result<(), EventError> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let existing = tables(&tx)?;
    if existing.contains("schema_version") {
        let found: i64 = tx
            .query_row(
                "SELECT version FROM schema_version WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if found != SCHEMA_VERSION {
            return Err(EventError::IncompatibleSchema {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        return Ok(());
    }
    if existing.is_empty() {
        create(&tx)?;
    } else if is_v1_layout(&tx, &existing)? {
        let bodies = v1_bodies(&tx)?;
        create(&tx)?;
        let imported = import_v1(&tx, &bodies)?;
        record_migration(&tx, IN_PLACE_MIGRATION, imported)?;
        tx.execute_batch("DROP TABLE events;").map_err(storage)?;
    } else {
        return Err(EventError::Migration(format!(
            "database is not an AiDEX event log (tables: {})",
            existing.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    tx.commit().map_err(storage)
}

fn create(tx: &Transaction<'_>) -> Result<(), EventError> {
    tx.execute_batch(CREATE).map_err(storage)?;
    tx.execute(
        "INSERT INTO schema_version(id, version) VALUES (1, ?1)",
        [SCHEMA_VERSION],
    )
    .map_err(storage)?;
    Ok(())
}

/// Append v1 bodies in order with fresh, contiguous cursors. Imported events carry
/// the import time as `occurred_at`; v1 records had no timestamp.
fn import_v1(tx: &Transaction<'_>, bodies: &[String]) -> Result<u64, EventError> {
    let mut state = store::read_state(tx)?;
    let imported_at = Timestamp::now();
    for body in bodies {
        let (scope, kind) = import_v1_body(body)?;
        let kind = sanitize::prepare(&scope, kind)?;
        store::append(tx, &mut state, scope, kind, imported_at.clone())?;
    }
    store::write_state(tx, &state)?;
    Ok(u64::try_from(bodies.len()).unwrap_or(u64::MAX))
}

fn record_migration(tx: &Transaction<'_>, name: &str, events: u64) -> Result<(), EventError> {
    tx.execute(
        "INSERT INTO event_migrations(name, applied_at, events) VALUES (?1, ?2, ?3)",
        params![
            name,
            Timestamp::now().as_str(),
            i64::try_from(events).unwrap_or(i64::MAX)
        ],
    )
    .map_err(storage)?;
    Ok(())
}

fn migration_applied(tx: &Transaction<'_>, name: &str) -> Result<bool, EventError> {
    tx.query_row(
        "SELECT COUNT(*) FROM event_migrations WHERE name = ?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .map_err(storage)
}

/// Read every v1 body from a separate, read-only PR #40 database.
pub(crate) fn read_v1_file(path: &Path) -> Result<Vec<String>, EventError> {
    refuse_symlinks(path)?;
    let legacy = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| EventError::Migration(format!("cannot open v1 event log: {error}")))?;
    legacy
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage)?;
    let existing = tables(&legacy)?;
    if !is_v1_layout(&legacy, &existing)? {
        return Err(EventError::Migration(format!(
            "{} is not a v1 event log",
            path.display()
        )));
    }
    v1_bodies(&legacy)
}

/// Outcome of importing a separate v1 database file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyImport {
    /// The v1 events were appended with cursors `1..=events`.
    Imported { events: u64 },
    /// A previous import already brought these events in.
    AlreadyImported,
    /// This log already holds events, so importing would misorder history; the v1
    /// file was left alone.
    SkippedNonEmpty,
}

/// Check the target state before reading and importing a separate v1 file.
pub(crate) fn import_file(
    connection: &mut Connection,
    legacy_path: &Path,
    retention: &RetentionPolicy,
) -> Result<(LegacyImport, State), EventError> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let state = store::read_state(&tx)?;
    if migration_applied(&tx, FILE_IMPORT_MIGRATION)? {
        return Ok((LegacyImport::AlreadyImported, state));
    }
    if state.last_cursor.get() > 0 {
        return Ok((LegacyImport::SkippedNonEmpty, state));
    }
    let bodies = read_v1_file(legacy_path)?;
    let events = import_v1(&tx, &bodies)?;
    record_migration(&tx, FILE_IMPORT_MIGRATION, events)?;
    let mut state = store::read_state(&tx)?;
    store::prune(&tx, retention, &mut state, Timestamp::now().unix_millis())?;
    store::write_state(&tx, &state)?;
    tx.commit().map_err(storage)?;
    Ok((LegacyImport::Imported { events }, state))
}
