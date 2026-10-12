//! Per-path write locks: writers to the same path (or to a path and a directory
//! containing it) serialize; everything else, including reads, runs concurrently.

use std::{
    collections::HashSet,
    sync::{Condvar, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use crate::{CancelToken, ToolError};

/// How often a waiting writer re-checks cancellation.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// A lock key: a workspace root and a workspace-relative path within it.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct LockKey {
    root: String,
    path: String,
}

impl LockKey {
    pub(crate) fn new(root: &str, path: &str) -> Self {
        // Windows file systems are case-insensitive by default.
        let fold = |text: &str| {
            if cfg!(windows) {
                text.to_lowercase()
            } else {
                text.to_owned()
            }
        };
        Self {
            root: fold(root),
            path: fold(path),
        }
    }

    /// Same root and one path equals or contains the other.
    fn overlaps(&self, other: &Self) -> bool {
        if self.root != other.root {
            return false;
        }
        let within = |inner: &str, outer: &str| {
            outer.is_empty()
                || inner == outer
                || (inner.len() > outer.len()
                    && inner.starts_with(outer)
                    && inner.as_bytes()[outer.len()] == b'/')
        };
        within(&self.path, &other.path) || within(&other.path, &self.path)
    }
}

#[derive(Debug, Default)]
pub(crate) struct WriteLocks {
    held: Mutex<HashSet<LockKey>>,
    released: Condvar,
}

impl WriteLocks {
    fn lock(&self) -> MutexGuard<'_, HashSet<LockKey>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Acquire every key at once (all-or-nothing, so no lock-order deadlocks),
    /// waiting while any overlapping key is held.
    pub(crate) fn acquire(
        &self,
        keys: Vec<LockKey>,
        cancel: &CancelToken,
    ) -> Result<LockGuard<'_>, ToolError> {
        let mut held = self.lock();
        loop {
            crate::context::check_cancel(cancel)?;
            let busy = keys
                .iter()
                .any(|key| held.iter().any(|other| key.overlaps(other)));
            if !busy {
                held.extend(keys.iter().cloned());
                return Ok(LockGuard { locks: self, keys });
            }
            held = self
                .released
                .wait_timeout(held, CANCEL_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Releases its keys on drop.
pub(crate) struct LockGuard<'a> {
    locks: &'a WriteLocks,
    keys: Vec<LockKey>,
}

impl Drop for LockGuard<'_> {
    fn drop(&mut self) {
        let mut held = self.locks.lock();
        for key in &self.keys {
            held.remove(key);
        }
        drop(held);
        self.locks.released.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };

    use super::*;

    #[test]
    fn overlap_is_component_wise() {
        let key = |path| LockKey::new("/w", path);
        assert!(key("a/b").overlaps(&key("a/b")));
        assert!(key("a").overlaps(&key("a/b")));
        assert!(key("a/b/c").overlaps(&key("a")));
        assert!(!key("a/b").overlaps(&key("a/bc")));
        assert!(!key("a").overlaps(&key("b")));
        assert!(key("").overlaps(&key("x")));
        assert!(!LockKey::new("/w", "a").overlaps(&LockKey::new("/v", "a")));
    }

    #[test]
    fn overlapping_writers_wait_and_disjoint_writers_do_not() {
        let locks = Arc::new(WriteLocks::default());
        let cancel = CancelToken::new();
        let guard = locks
            .acquire(vec![LockKey::new("/w", "dir")], &cancel)
            .unwrap();
        // A disjoint key is granted immediately.
        drop(
            locks
                .acquire(vec![LockKey::new("/w", "other")], &cancel)
                .unwrap(),
        );
        let entered = Arc::new(AtomicBool::new(false));
        let waiter = {
            let (locks, entered, cancel) = (locks.clone(), entered.clone(), cancel.clone());
            thread::spawn(move || {
                let _guard = locks
                    .acquire(vec![LockKey::new("/w", "dir/file")], &cancel)
                    .unwrap();
                entered.store(true, Ordering::SeqCst);
            })
        };
        thread::sleep(Duration::from_millis(100));
        assert!(!entered.load(Ordering::SeqCst), "nested key must wait");
        drop(guard);
        waiter.join().unwrap();
        assert!(entered.load(Ordering::SeqCst));
    }

    #[test]
    fn waiting_is_cancellable() {
        let locks = WriteLocks::default();
        let cancel = CancelToken::new();
        let _guard = locks
            .acquire(vec![LockKey::new("/w", "a")], &cancel)
            .unwrap();
        let other = CancelToken::new();
        other.cancel();
        assert!(matches!(
            locks.acquire(vec![LockKey::new("/w", "a")], &other),
            Err(ToolError::Cancelled)
        ));
    }
}
