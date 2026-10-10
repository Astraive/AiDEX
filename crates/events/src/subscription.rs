//! Subscriptions: persisted catch-up, then bounded live delivery with lag recovery.
//!
//! A subscription is registered at a "live-from" cursor, decided under the log lock:
//! every durable event up to it is paged from disk, and every event after it (durable
//! or ephemeral) is offered to the subscriber's bounded live queue in cursor order.
//! Publishers only ever try to enqueue. When a queue is full (by count or bytes) the
//! subscriber is marked lagged and its queue is dropped, so publishers never block and
//! memory per subscriber stays bounded. A lagged subscriber picks a new live-from
//! cursor, pages the missed durable events back from disk starting after the last
//! one it consumed, and rejoins the live queue. Ordered watermarks expose durable
//! events consumed by filtering without overtaking matching events. Ephemeral
//! events dropped while lagging are lost; they were never persisted. Live delivery
//! covers appends made through this process's `EventLog` handles.

use std::{
    collections::VecDeque,
    fmt,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use crate::{Cursor, EventEnvelope, EventError, EventFilter, EventLog};

/// Default live-queue bound, in events.
pub const DEFAULT_CAPACITY: usize = 1024;
/// Largest live-queue bound a caller may request, in events.
pub const MAX_CAPACITY: usize = 16 * 1024;
/// Live-queue bound in encoded bytes, regardless of the event bound.
pub const MAX_QUEUED_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct Queue {
    items: VecDeque<(EventEnvelope, usize)>,
    bytes: usize,
    lagged: bool,
    /// Latest durable cursor offered, including filtered-out events. Kept outside
    /// the event queue so arbitrarily many nonmatches consume constant space.
    watermark: Cursor,
    lag_count: u64,
}

impl Queue {
    fn mark_lagged(&mut self) {
        if !self.lagged {
            self.lag_count += 1;
        }
        self.lagged = true;
        self.items.clear();
        self.bytes = 0;
        self.watermark = Cursor::ZERO;
    }
}

/// The publisher-facing half of a subscription.
pub(crate) struct LiveSlot {
    id: u64,
    filter: EventFilter,
    capacity: usize,
    queue: Mutex<Queue>,
    ready: Condvar,
}

impl LiveSlot {
    pub(crate) fn new(id: u64, filter: EventFilter, capacity: usize) -> Self {
        Self {
            id,
            filter,
            capacity,
            queue: Mutex::new(Queue::default()),
            ready: Condvar::new(),
        }
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Enqueue without blocking; on overflow mark lagged and drop the queue.
    fn offer(&self, envelope: &EventEnvelope, bytes: usize) {
        let matches = self.filter.matches(envelope);
        if !matches && !envelope.is_durable() {
            return;
        }
        let mut queue = self.lock();
        if queue.lagged {
            return;
        }
        if envelope.is_durable() {
            queue.watermark = envelope.cursor;
        }
        if matches {
            if queue.items.len() >= self.capacity || queue.bytes + bytes > MAX_QUEUED_BYTES {
                queue.mark_lagged();
            } else {
                queue.items.push_back((envelope.clone(), bytes));
                queue.bytes += bytes;
            }
        }
        drop(queue);
        self.ready.notify_all();
    }

    /// Invalidate live delivery after a durable write that bypassed fan-out.
    /// Called under the log lock, just like ordinary queue overflow.
    pub(crate) fn mark_lagged(&self) {
        self.lock().mark_lagged();
        self.ready.notify_all();
    }

    /// Leave the lagged state with an empty queue. Called with the log locked, so no
    /// event can be offered between the reset and the caller's new live-from cursor.
    pub(crate) fn reset(&self) {
        let mut queue = self.lock();
        queue.lagged = false;
        queue.items.clear();
        queue.bytes = 0;
        queue.watermark = Cursor::ZERO;
    }
}

/// Offer `envelope` to every live subscriber (called with the log locked, in cursor order).
pub(crate) fn fan_out(slots: &[Arc<LiveSlot>], envelope: &EventEnvelope, bytes: usize) {
    for slot in slots {
        slot.offer(envelope, bytes);
    }
}

/// The live queue overflowed; resume from disk.
struct Lagged;

/// An ordered subscription delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubscriptionItem {
    /// A matching event. A durable cursor is safe to acknowledge after processing.
    Event(EventEnvelope),
    /// All durable events through this cursor were delivered earlier or filtered
    /// out. Safe to acknowledge after processing all preceding stream items.
    /// This is progress, not an event or a periodic heartbeat.
    Watermark(Cursor),
}

/// An ordered event stream from [`EventLog::subscribe`]. Dropping it unregisters it.
pub struct Subscription {
    log: EventLog,
    slot: Arc<LiveSlot>,
    filter: EventFilter,
    /// Every durable event up to here was delivered or filtered out.
    scanned: Cursor,
    /// Last cursor exposed by a durable event or watermark; never ahead of pending
    /// matching history, even when a page scan has already reached further.
    consumed: Cursor,
    /// Durable events up to here come from disk; later ones from the live queue.
    live_from: Cursor,
    pending: VecDeque<EventEnvelope>,
    live: bool,
}

impl Subscription {
    pub(crate) fn new(
        log: EventLog,
        slot: Arc<LiveSlot>,
        filter: EventFilter,
        after: Cursor,
        live_from: Cursor,
    ) -> Self {
        Self {
            log,
            slot,
            filter,
            scanned: after,
            consumed: after,
            live_from,
            pending: VecDeque::new(),
            live: false,
        }
    }

    pub fn filter(&self) -> &EventFilter {
        &self.filter
    }

    /// The next event or consumed-cursor watermark, waiting up to `timeout`;
    /// `Ok(None)` when neither arrived. Watermarks never overtake matching durable
    /// events and do not advance on ephemeral events.
    ///
    /// Fails with [`EventError::CursorExpired`] if retention removed events this
    /// subscription had not yet consumed (the stream cannot continue without a gap).
    pub fn next_timeout(&mut self, timeout: Duration) -> Result<Option<SubscriptionItem>, EventError> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if let Some(envelope) = self.pending.pop_front() {
                self.consumed = envelope.cursor;
                return Ok(Some(SubscriptionItem::Event(envelope)));
            }
            if self.consumed < self.scanned {
                self.consumed = self.scanned;
                return Ok(Some(SubscriptionItem::Watermark(self.consumed)));
            }
            if !self.live {
                if self.scanned >= self.live_from {
                    self.live = true;
                    continue;
                }
                let page = self
                    .log
                    .catch_up(&self.filter, self.scanned, self.live_from)?;
                self.scanned = page.next_cursor;
                self.live = !page.has_more;
                self.pending.extend(page.events);
                continue;
            }
            match self.next_live(deadline) {
                Ok(next) => return Ok(next),
                Err(Lagged) => {
                    self.live_from = self.log.resume_live(&self.slot);
                    self.live = false;
                }
            }
        }
    }

    /// [`Self::next_timeout`] without waiting.
    pub fn try_next(&mut self) -> Result<Option<SubscriptionItem>, EventError> {
        self.next_timeout(Duration::ZERO)
    }

    /// The next live delivery, `Ok(None)` at the deadline.
    fn next_live(&mut self, deadline: Option<Instant>) -> Result<Option<SubscriptionItem>, Lagged> {
        let mut queue = self.slot.lock();
        loop {
            if queue.lagged {
                return Err(Lagged);
            }
            // A live ephemeral event carries the latest durable cursor. Emit the
            // independently consumed durable prefix first, without acknowledging
            // the ephemeral event itself or dropping ephemeral events at equality.
            if let Some((envelope, _)) = queue.items.front() {
                if !envelope.is_durable() && envelope.cursor > self.scanned {
                    self.scanned = envelope.cursor;
                    self.consumed = self.scanned;
                    return Ok(Some(SubscriptionItem::Watermark(self.consumed)));
                }
            }
            if let Some((envelope, bytes)) = queue.items.pop_front() {
                queue.bytes -= bytes;
                if envelope.is_durable() {
                    if envelope.cursor <= self.scanned {
                        continue;
                    }
                    self.scanned = envelope.cursor;
                    self.consumed = self.scanned;
                } else if envelope.cursor < self.scanned {
                    continue;
                }
                return Ok(Some(SubscriptionItem::Event(envelope)));
            }
            if queue.watermark > self.scanned {
                self.scanned = queue.watermark;
                self.consumed = self.scanned;
                return Ok(Some(SubscriptionItem::Watermark(self.consumed)));
            }
            let remaining = deadline.map_or(Duration::from_secs(3600), |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            if remaining.is_zero() {
                return Ok(None);
            }
            queue = self
                .slot
                .ready
                .wait_timeout(queue, remaining)
                .map_or_else(|poisoned| poisoned.into_inner().0, |(queue, _)| queue);
        }
    }

    /// Times this subscriber overflowed its live queue and fell back to disk.
    pub fn lag_count(&self) -> u64 {
        self.slot.lock().lag_count
    }

    /// Events currently held in the live queue (never more than its capacity).
    pub fn queued(&self) -> usize {
        self.slot.lock().items.len()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.log.unsubscribe(self.slot.id());
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("filter", &self.filter)
            .field("scanned", &self.scanned)
            .field("live_from", &self.live_from)
            .field("live", &self.live)
            .finish_non_exhaustive()
    }
}
