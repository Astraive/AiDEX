//! SQL operations on an open event database. Callers hold the log lock and, for
//! writes, an immediate transaction.

use std::time::Duration;

use aidex_core::Timestamp;
use rusqlite::{Connection, params, params_from_iter, types::Value as SqlValue};

use super::{ReplayPage, RetentionPolicy};
use crate::{
    Cursor, EventEnvelope, EventError, EventFilter, EventKind, EventScope, error::storage, sanitize,
};

/// The log's persistent counters.
#[derive(Clone, Copy, Debug)]
pub(crate) struct State {
    /// Cursor of the newest durable event ever appended (`ZERO` when none).
    pub(crate) last_cursor: Cursor,
    /// Every cursor up to and including this one has been removed by retention.
    pub(crate) pruned_through: Cursor,
}

pub(crate) fn read_state(connection: &Connection) -> Result<State, EventError> {
    let (last, pruned): (i64, i64) = connection
        .query_row(
            "SELECT last_cursor, pruned_through FROM event_log_state WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(storage)?;
    Ok(State {
        last_cursor: Cursor::from_sql(last)?,
        pruned_through: Cursor::from_sql(pruned)?,
    })
}

pub(crate) fn write_state(connection: &Connection, state: &State) -> Result<(), EventError> {
    connection
        .execute(
            "UPDATE event_log_state SET last_cursor = ?1, pruned_through = ?2 WHERE id = 1",
            params![state.last_cursor.to_sql()?, state.pruned_through.to_sql()?],
        )
        .map_err(storage)?;
    Ok(())
}

/// A replay or subscription may start strictly after `after` only if nothing after it
/// was pruned and the log has actually reached it.
pub(crate) fn check_after(after: Cursor, state: &State) -> Result<(), EventError> {
    after.to_sql()?;
    if after < state.pruned_through {
        return Err(EventError::CursorExpired {
            requested: after,
            oldest_available: Cursor::new(state.pruned_through.get().saturating_add(1)),
        });
    }
    if after > state.last_cursor {
        return Err(EventError::CursorAhead {
            requested: after,
            latest: state.last_cursor,
        });
    }
    Ok(())
}

/// Assign the next cursor to an already-sanitized durable event and insert it.
/// Returns the envelope and its encoded size.
pub(crate) fn append(
    connection: &Connection,
    state: &mut State,
    scope: EventScope,
    event: EventKind,
    occurred_at: Timestamp,
) -> Result<(EventEnvelope, usize), EventError> {
    let next = state
        .last_cursor
        .get()
        .checked_add(1)
        .filter(|next| i64::try_from(*next).is_ok())
        .ok_or_else(|| EventError::Storage("event cursor space exhausted".into()))?;
    let cursor = Cursor::new(next);
    let occurred_ms = occurred_at.unix_millis();
    let envelope = EventEnvelope::new(cursor, scope, occurred_at, event);
    let body = sanitize::encode(&envelope)?;
    connection
        .execute(
            "INSERT INTO event_log(cursor, workspace_id, session_id, kind, occurred_ms, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                cursor.to_sql()?,
                envelope.scope.workspace_id.as_ref().map(|id| id.as_str()),
                envelope.scope.session_id.as_ref().map(|id| id.as_str()),
                envelope.name(),
                occurred_ms,
                body,
            ],
        )
        .map_err(storage)?;
    state.last_cursor = cursor;
    Ok((envelope, body.len()))
}

/// Remove the oldest events beyond the retention policy. Only a prefix of the log is
/// ever removed, so retained cursors stay contiguous and gaps are always detectable.
pub(crate) fn prune(
    connection: &Connection,
    retention: &RetentionPolicy,
    state: &mut State,
    now_ms: i64,
) -> Result<u64, EventError> {
    let mut boundary = state.pruned_through.get();
    if let Some(max_events) = retention.max_events {
        boundary = boundary.max(state.last_cursor.get().saturating_sub(max_events));
    }
    if let Some(max_age) = retention.max_age {
        let cutoff = now_ms.saturating_sub(duration_ms(max_age));
        // Count retention may already remove a fresh row. Beyond that boundary,
        // age retention must stop at the first fresh row, regardless of clock order.
        let first_fresh: Option<i64> = connection
            .query_row(
                "SELECT MIN(cursor) FROM event_log WHERE cursor > ?2 AND occurred_ms >= ?1",
                params![cutoff, Cursor::new(boundary).to_sql()?],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let age_boundary = match first_fresh {
            Some(cursor) => Cursor::from_sql(cursor)?.get() - 1,
            None => state.last_cursor.get(),
        };
        boundary = boundary.max(age_boundary);
    }
    if boundary <= state.pruned_through.get() {
        return Ok(0);
    }
    let boundary = Cursor::new(boundary);
    let removed = connection
        .execute(
            "DELETE FROM event_log WHERE cursor <= ?1",
            [boundary.to_sql()?],
        )
        .map_err(storage)?;
    state.pruned_through = boundary;
    Ok(u64::try_from(removed).unwrap_or(u64::MAX))
}

fn duration_ms(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Bounds of one page query.
#[derive(Clone, Copy)]
pub(crate) struct PageQuery<'a> {
    pub(crate) filter: &'a EventFilter,
    pub(crate) after: Cursor,
    /// Inclusive upper cursor bound (subscription catch-up); `None` reads to the end.
    pub(crate) until: Option<Cursor>,
    pub(crate) limit: usize,
    pub(crate) max_bytes: usize,
}

/// Events matching the filter strictly after `after` (and up to `until`), in cursor
/// order, at most `limit` of them and (beyond the first) at most `max_bytes` of
/// encoded bodies.
pub(crate) fn page(
    connection: &Connection,
    query: &PageQuery<'_>,
    state: &State,
) -> Result<ReplayPage, EventError> {
    let PageQuery {
        filter,
        after,
        until,
        limit,
        max_bytes,
    } = *query;
    let mut sql = String::from("SELECT cursor, body FROM event_log WHERE cursor > ?");
    let mut args = vec![SqlValue::Integer(after.to_sql()?)];
    if let Some(until) = until {
        sql.push_str(" AND cursor <= ?");
        args.push(SqlValue::Integer(until.to_sql()?));
    }
    if let Some(workspace) = &filter.workspace_id {
        sql.push_str(" AND workspace_id = ?");
        args.push(SqlValue::Text(workspace.as_str().to_owned()));
    }
    if let Some(session) = &filter.session_id {
        sql.push_str(" AND session_id = ?");
        args.push(SqlValue::Text(session.as_str().to_owned()));
    }
    if !filter.kinds.is_empty() {
        // Prefixes are validated to `[a-z0-9_.]`, so they carry no GLOB metacharacters.
        let clauses = vec!["kind = ? OR kind GLOB ?"; filter.kinds.len()].join(" OR ");
        sql.push_str(&format!(" AND ({clauses})"));
        for prefix in &filter.kinds {
            args.push(SqlValue::Text(prefix.clone()));
            args.push(SqlValue::Text(format!("{prefix}.*")));
        }
    }
    sql.push_str(" ORDER BY cursor LIMIT ?");
    args.push(SqlValue::Integer(
        i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX),
    ));

    let mut statement = connection.prepare_cached(&sql).map_err(storage)?;
    let mut rows = statement
        .query(params_from_iter(args.iter()))
        .map_err(storage)?;
    let mut events: Vec<EventEnvelope> = Vec::new();
    let mut bytes = 0usize;
    let mut has_more = false;
    while let Some(row) = rows.next().map_err(storage)? {
        let cursor: i64 = row.get(0).map_err(storage)?;
        let body: String = row.get(1).map_err(storage)?;
        if events.len() >= limit || (!events.is_empty() && bytes + body.len() > max_bytes) {
            has_more = true;
            break;
        }
        let envelope = EventEnvelope::decode(&body)?;
        if envelope.cursor != Cursor::from_sql(cursor)? {
            return Err(EventError::Storage(format!(
                "stored event {cursor} carries cursor {}",
                envelope.cursor
            )));
        }
        bytes += body.len();
        events.push(envelope);
    }
    let next_cursor = if has_more {
        events.last().map_or(after, |envelope| envelope.cursor)
    } else {
        until
            .map_or(state.last_cursor, |until| until.min(state.last_cursor))
            .max(after)
    };
    Ok(ReplayPage {
        events,
        next_cursor,
        has_more,
    })
}

pub(crate) fn oldest_cursor(connection: &Connection) -> Result<Option<Cursor>, EventError> {
    let oldest: Option<i64> = connection
        .query_row("SELECT MIN(cursor) FROM event_log", [], |row| row.get(0))
        .map_err(storage)?;
    oldest.map(Cursor::from_sql).transpose()
}
