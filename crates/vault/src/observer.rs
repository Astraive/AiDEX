//! Lock notices: how the owner of a [`VaultService`](crate::VaultService) learns
//! that its unlocked session ended.
//!
//! The service ends its session on an explicit lock, after the idle timeout,
//! after a `Conflict` or an uncertain commit of its own header, and after a panic
//! while the vault was in use. Each ending queues exactly one [`LockNotice`] while
//! the session lock is held, so notices keep the order in which sessions ended.
//! Observers run only after that lock is released, one notice at a time: an
//! explicit lock delivers on the calling thread before it returns, and every other
//! ending is delivered by the service's background thread, so a caller that holds
//! its own locks while using the vault is never re-entered by an observer. A new
//! session begins (unlock, init) only once every earlier ending has been delivered,
//! so an observer's cleanup never reaches the session that follows.
//!
//! One replacement is not an ending: unlocking while a session is open whose
//! `vault.json` changed on disk swaps in a session for the new header without a
//! notice, because the vault never locks in between.

use crate::service::lock_or_recover;
use aidex_core::EntityId;
use std::{
    cell::Cell,
    collections::VecDeque,
    panic::{self, AssertUnwindSafe},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

/// Why an unlocked session ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockReason {
    /// [`VaultService::lock`](crate::VaultService::lock).
    Requested,
    /// No secret operation within the idle timeout.
    Idle,
    /// `vault.json` changed on disk since the session loaded it.
    Conflict,
    /// A commit of the vault's own header may or may not have reached the disk.
    CommitUncertain,
    /// A panic while the vault was in use (the service fails closed).
    Panic,
}

/// One ended session, delivered once to every lock observer. Carries no secret
/// material.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockNotice {
    pub vault_id: EntityId,
    pub reason: LockReason,
    /// Handles that were still live and are now unusable.
    pub revoked_handles: usize,
}

pub(crate) type LockObserver = Arc<dyn Fn(&LockNotice) + Send + Sync>;

thread_local! {
    /// Set while this thread runs observers, so an observer that locks the vault
    /// does not wait for its own delivery (the running loop picks the notice up).
    static DELIVERING: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct Queue {
    notices: VecDeque<LockNotice>,
    /// Notices queued or still being delivered.
    undelivered: usize,
    /// The service is gone: the background thread stops waiting.
    closed: bool,
}

/// Registered observers and the notices not yet delivered to them.
#[derive(Default)]
pub(crate) struct LockNotices {
    observers: Mutex<Vec<LockObserver>>,
    queue: Mutex<Queue>,
    queued: Condvar,
    /// Held while observers run, so notices are delivered one at a time, in order.
    delivery: Mutex<()>,
}

impl LockNotices {
    pub(crate) fn observe(&self, observer: LockObserver) {
        lock_or_recover(&self.observers).push(observer);
    }

    /// Queue a notice (with the session lock held) and wake the background thread.
    pub(crate) fn push(&self, notice: LockNotice) {
        {
            let mut queue = lock_or_recover(&self.queue);
            queue.notices.push_back(notice);
            queue.undelivered += 1;
        }
        self.queued.notify_all();
    }

    /// Deliver every queued notice on this thread. Returns once the queue is empty
    /// and no other thread is still running observers for a notice it took.
    pub(crate) fn deliver(&self) {
        if DELIVERING.get() {
            return;
        }
        let _delivery = lock_or_recover(&self.delivery);
        DELIVERING.set(true);
        loop {
            let next = lock_or_recover(&self.queue).notices.pop_front();
            let Some(notice) = next else {
                break;
            };
            let observers = lock_or_recover(&self.observers).clone();
            for observer in observers {
                // A panicking observer must not skip the others or end the
                // background thread that delivers later notices.
                let _ = panic::catch_unwind(AssertUnwindSafe(|| observer(&notice)));
            }
            let mut queue = lock_or_recover(&self.queue);
            queue.undelivered = queue.undelivered.saturating_sub(1);
        }
        DELIVERING.set(false);
    }

    /// Whether every notice queued so far has been delivered (observers included).
    /// Always true on a thread that is running observers: it cannot wait for the
    /// notice it is delivering.
    pub(crate) fn settled(&self) -> bool {
        DELIVERING.get() || lock_or_recover(&self.queue).undelivered == 0
    }

    /// Wait until a notice is queued, the service is gone, or `timeout` passes.
    pub(crate) fn wait(&self, timeout: Duration) {
        let queue = lock_or_recover(&self.queue);
        let _ = self.queued.wait_timeout_while(queue, timeout, |queue| {
            queue.notices.is_empty() && !queue.closed
        });
    }

    /// Wake the background thread for good: the service was dropped.
    pub(crate) fn close(&self) {
        lock_or_recover(&self.queue).closed = true;
        self.queued.notify_all();
    }
}
