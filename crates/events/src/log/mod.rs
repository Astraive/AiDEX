//! The durable, drive-wide event log.

mod schema;
mod store;

pub use schema::LegacyImport;

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use aidex_core::Timestamp;
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::{
    Cursor, EventEnvelope, EventError, EventFilter, EventKind, EventScope, EventSink,
    error::storage,
    sanitize,
    subscription::{self, LiveSlot, Subscription},
};

/// Largest replay page.
pub const MAX_PAGE_SIZE: usize = 256;
/// Default retention: the newest 100 000 events, no age limit.
pub const DEFAULT_MAX_EVENTS: u64 = 100_000;

/// How much history the log keeps. Only the oldest events are ever removed, and a
/// replay from before the retained range fails with [`EventError::CursorExpired`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RetentionPolicy {
    /// Keep at most this many of the newest events.
    pub max_events: Option<u64>,
    /// Remove events older than this (by `occurred_at`).
    pub max_age: Option<Duration>,
}

impl RetentionPolicy {
    /// Keep everything.
    pub const UNBOUNDED: Self = Self {
        max_events: None,
        max_age: None,
    };

    /// Keep the newest `count` events.
    pub const fn max_events(count: u64) -> Self {
        Self {
            max_events: Some(count),
            max_age: None,
        }
    }

    pub fn validate(&self) -> Result<(), EventError> {
        if self.max_events == Some(0) {
            return Err(EventError::InvalidRetention("max_events must be positive"));
        }
        if self.max_age.is_some_and(|age| age.is_zero()) {
            return Err(EventError::InvalidRetention("max_age must be positive"));
        }
        Ok(())
    }
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self::max_events(DEFAULT_MAX_EVENTS)
    }
}

/// One bounded replay result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayPage {
    /// Matching events strictly after the requested cursor, ascending.
    pub events: Vec<EventEnvelope>,
    /// Resume strictly after this cursor. Without `has_more` it is the log's latest
    /// cursor: every event up to it was examined (filtered events are skipped too).
    pub next_cursor: Cursor,
    pub has_more: bool,
}

struct Inner {
    connection: Connection,
    /// Latest durable cursor, cached for ephemeral events.
    last_cursor: Cursor,
    subscribers: Vec<Arc<LiveSlot>>,
    next_subscriber: u64,
}

struct Shared {
    path: PathBuf,
    retention: RetentionPolicy,
    inner: Mutex<Inner>,
}

/// SQLite-backed append-only event log with one global cursor per database.
///
/// Cloning is cheap and shares the same connection, cursor, and subscribers.
/// Appends are durable (`synchronous=FULL`) before they return and before live
/// subscribers see them. All operations are blocking and thread-safe.
#[derive(Clone)]
pub struct EventLog {
    shared: Arc<Shared>,
}

impl EventLog {
    /// Open or create the database at `path` (its directory must exist).
    ///
    /// Symlinked database paths are refused. A PR #40 database is migrated in place
    /// (its rows imported in order); a newer schema is refused. Retention is applied
    /// once on open and after every append.
    pub fn open(path: impl AsRef<Path>, retention: RetentionPolicy) -> Result<Self, EventError> {
        retention.validate()?;
        let path = path.as_ref().to_path_buf();
        let mut connection = schema::open_connection(&path)?;
        schema::prepare(&mut connection)?;
        let state = {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let mut state = store::read_state(&tx)?;
            store::prune(&tx, &retention, &mut state, Timestamp::now().unix_millis())?;
            store::write_state(&tx, &state)?;
            tx.commit().map_err(storage)?;
            state
        };
        Ok(Self {
            shared: Arc::new(Shared {
                path,
                retention,
                inner: Mutex::new(Inner {
                    connection,
                    last_cursor: state.last_cursor,
                    subscribers: Vec::new(),
                    next_subscriber: 1,
                }),
            }),
        })
    }

    pub fn database_path(&self) -> &Path {
        &self.shared.path
    }

    pub fn retention(&self) -> RetentionPolicy {
        self.shared.retention
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Validate, redact, and append one event.
    ///
    /// A durable event is committed with the next cursor before this returns; a
    /// failed append assigns nothing. An ephemeral event is only offered to live
    /// subscribers. Publishers never wait for subscribers.
    pub fn append(&self, scope: EventScope, event: EventKind) -> Result<EventEnvelope, EventError> {
        let event = sanitize::prepare(&scope, event)?;
        let mut guard = self.lock();
        let inner = &mut *guard;
        let occurred_at = Timestamp::now();
        if !event.is_durable() {
            let envelope = EventEnvelope::new(inner.last_cursor, scope, occurred_at, event);
            let bytes = sanitize::encode(&envelope)?.len();
            subscription::fan_out(&inner.subscribers, &envelope, bytes);
            return Ok(envelope);
        }
        let now_ms = occurred_at.unix_millis();
        let tx = inner
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let mut state = store::read_state(&tx)?;
        let (envelope, bytes) = store::append(&tx, &mut state, scope, event, occurred_at)?;
        store::prune(&tx, &self.shared.retention, &mut state, now_ms)?;
        store::write_state(&tx, &state)?;
        tx.commit().map_err(storage)?;
        inner.last_cursor = state.last_cursor;
        subscription::fan_out(&inner.subscribers, &envelope, bytes);
        Ok(envelope)
    }

    /// Up to `limit` (1..=[`MAX_PAGE_SIZE`]) matching events strictly after `after`.
    ///
    /// Fails with [`EventError::CursorExpired`] if retention removed events after
    /// `after`, and [`EventError::CursorAhead`] if `after` was never assigned.
    pub fn replay(
        &self,
        filter: &EventFilter,
        after: Cursor,
        limit: usize,
    ) -> Result<ReplayPage, EventError> {
        self.replay_within(filter, after, limit, usize::MAX)
    }

    /// [`Self::replay`], also stopping before the encoded bodies exceed `max_bytes`
    /// (at least one event is returned when any match).
    pub fn replay_within(
        &self,
        filter: &EventFilter,
        after: Cursor,
        limit: usize,
        max_bytes: usize,
    ) -> Result<ReplayPage, EventError> {
        if limit == 0 || limit > MAX_PAGE_SIZE {
            return Err(EventError::InvalidLimit {
                requested: limit,
                max: MAX_PAGE_SIZE,
            });
        }
        filter.validate()?;
        self.page(&store::PageQuery {
            filter,
            after,
            until: None,
            limit,
            max_bytes,
        })
    }

    /// One page of history in `(after, until]` for a subscription catching up.
    pub(crate) fn catch_up(
        &self,
        filter: &EventFilter,
        after: Cursor,
        until: Cursor,
    ) -> Result<ReplayPage, EventError> {
        self.page(&store::PageQuery {
            filter,
            after,
            until: Some(until),
            limit: MAX_PAGE_SIZE,
            max_bytes: usize::MAX,
        })
    }

    fn page(&self, query: &store::PageQuery<'_>) -> Result<ReplayPage, EventError> {
        query.after.to_sql()?;
        let mut inner = self.lock();
        // One read transaction: the page and the latest cursor come from one snapshot.
        let tx = inner.connection.transaction().map_err(storage)?;
        let state = store::read_state(&tx)?;
        store::check_after(query.after, &state)?;
        let page = store::page(&tx, query, &state)?;
        tx.commit().map_err(storage)?;
        Ok(page)
    }

    /// Cursor of the newest durable event ever appended (`Cursor::ZERO` if none).
    pub fn latest_cursor(&self) -> Result<Cursor, EventError> {
        let inner = self.lock();
        Ok(store::read_state(&inner.connection)?.last_cursor)
    }

    /// Cursor of the oldest retained event, `None` when nothing is retained.
    pub fn oldest_cursor(&self) -> Result<Option<Cursor>, EventError> {
        let inner = self.lock();
        store::oldest_cursor(&inner.connection)
    }

    /// Apply the retention policy now; returns how many events were removed.
    pub fn prune(&self) -> Result<u64, EventError> {
        let mut inner = self.lock();
        let tx = inner
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let mut state = store::read_state(&tx)?;
        let removed = store::prune(
            &tx,
            &self.shared.retention,
            &mut state,
            Timestamp::now().unix_millis(),
        )?;
        store::write_state(&tx, &state)?;
        tx.commit().map_err(storage)?;
        Ok(removed)
    }

    /// Import a separate PR #40 (v1) database, read-only, into this log.
    ///
    /// Runs at most once per log and only while this log is empty; the v1 file is
    /// never modified. Events are imported in their original order with fresh
    /// cursors and the import time as `occurred_at`.
    pub fn import_legacy(&self, legacy_path: &Path) -> Result<LegacyImport, EventError> {
        let bodies = schema::read_v1_file(legacy_path)?;
        let mut inner = self.lock();
        let (outcome, state) = schema::import_file(&mut inner.connection, &bodies)?;
        inner.last_cursor = state.last_cursor;
        Ok(outcome)
    }

    /// Stream matching events and consumed durable watermarks strictly after
    /// `after`: persisted history first, then live deliveries, without duplicates
    /// or gaps. Watermarks follow the matching events they cover. Uses
    /// [`subscription::DEFAULT_CAPACITY`] for the live event queue.
    pub fn subscribe(
        &self,
        filter: EventFilter,
        after: Cursor,
    ) -> Result<Subscription, EventError> {
        self.subscribe_with_capacity(filter, after, subscription::DEFAULT_CAPACITY)
    }

    /// [`Self::subscribe`] with an explicit live-queue bound (1..=[`subscription::MAX_CAPACITY`]).
    pub fn subscribe_with_capacity(
        &self,
        filter: EventFilter,
        after: Cursor,
        capacity: usize,
    ) -> Result<Subscription, EventError> {
        if capacity == 0 || capacity > subscription::MAX_CAPACITY {
            return Err(EventError::InvalidLimit {
                requested: capacity,
                max: subscription::MAX_CAPACITY,
            });
        }
        filter.validate()?;
        let mut inner = self.lock();
        let state = store::read_state(&inner.connection)?;
        store::check_after(after, &state)?;
        // Everything up to `live_from` comes from disk; everything after it is offered
        // to the live queue, because both are decided under the same lock.
        inner.last_cursor = inner.last_cursor.max(state.last_cursor);
        let live_from = inner.last_cursor;
        let id = inner.next_subscriber;
        inner.next_subscriber = id.saturating_add(1);
        let slot = Arc::new(LiveSlot::new(id, filter.clone(), capacity));
        inner.subscribers.push(Arc::clone(&slot));
        drop(inner);
        Ok(Subscription::new(
            self.clone(),
            slot,
            filter,
            after,
            live_from,
        ))
    }

    /// Clear a lagged slot and return the cursor after which live delivery resumes.
    pub(crate) fn resume_live(&self, slot: &LiveSlot) -> Cursor {
        let inner = self.lock();
        slot.reset();
        inner.last_cursor
    }

    /// Live subscriptions currently registered.
    pub fn subscriber_count(&self) -> usize {
        self.lock().subscribers.len()
    }

    pub(crate) fn unsubscribe(&self, id: u64) {
        self.lock().subscribers.retain(|slot| slot.id() != id);
    }
}

impl EventSink for EventLog {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        self.append(scope, event).map(|envelope| envelope.cursor)
    }
}

impl fmt::Debug for EventLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventLog")
            .field("path", &self.shared.path)
            .field("retention", &self.shared.retention)
            .finish_non_exhaustive()
    }
}
