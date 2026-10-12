//! [`EventSink`]: how domain crates emit events without owning the log.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aidex_core::Timestamp;

use crate::{Cursor, EventEnvelope, EventError, EventKind, EventScope, sanitize};

/// Emits typed events. Implemented by [`crate::EventLog`] (durable), [`MemoryEventSink`]
/// (records, for tests), and [`NullEventSink`] (discards).
///
/// Every implementation applies the same validation and redaction as the log, so a
/// scope or identifier the log would refuse is refused everywhere. Durable events
/// return their cursor; ephemeral events return the latest durable cursor.
pub trait EventSink: Send + Sync {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError>;
}

impl<S: EventSink + ?Sized> EventSink for Arc<S> {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        (**self).emit(scope, event)
    }
}

impl<S: EventSink + ?Sized> EventSink for &S {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        (**self).emit(scope, event)
    }
}

/// Validates and discards events.
///
/// Reserves the largest possible cursor when checking the encoded-envelope limit,
/// and returns [`Cursor::ZERO`] because no event is recorded.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        let event = sanitize::prepare(&scope, event)?;
        let envelope = EventEnvelope::new(Cursor::new(u64::MAX), scope, Timestamp::now(), event);
        sanitize::encode(&envelope)?;
        Ok(Cursor::ZERO)
    }
}

/// Records sanitized envelopes in memory with log-like cursors. For tests.
#[derive(Debug, Default)]
pub struct MemoryEventSink {
    recorded: Mutex<Recorded>,
}

#[derive(Debug, Default)]
struct Recorded {
    last_cursor: Cursor,
    events: Vec<EventEnvelope>,
}

impl MemoryEventSink {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Recorded> {
        self.recorded.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Everything emitted so far, in order (ephemeral events included).
    pub fn events(&self) -> Vec<EventEnvelope> {
        self.lock().events.clone()
    }

    /// Remove and return everything emitted so far.
    pub fn take(&self) -> Vec<EventEnvelope> {
        std::mem::take(&mut self.lock().events)
    }

    /// Names of the recorded events, in order.
    pub fn names(&self) -> Vec<&'static str> {
        self.lock().events.iter().map(EventEnvelope::name).collect()
    }

    pub fn len(&self) -> usize {
        self.lock().events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().events.is_empty()
    }
}

impl EventSink for MemoryEventSink {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        let event = sanitize::prepare(&scope, event)?;
        let mut recorded = self.lock();
        let cursor = if event.is_durable() {
            Cursor::new(recorded.last_cursor.get().saturating_add(1))
        } else {
            recorded.last_cursor
        };
        let envelope = EventEnvelope::new(cursor, scope, Timestamp::now(), event);
        sanitize::encode(&envelope)?;
        recorded.last_cursor = cursor;
        recorded.events.push(envelope);
        Ok(cursor)
    }
}
