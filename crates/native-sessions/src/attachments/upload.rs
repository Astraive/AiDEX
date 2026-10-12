//! In-flight chunked uploads, kept in memory only.
//!
//! An upload owns a temporary file in `<db stem>-attachments/` that receives
//! sealed segments as chunks arrive; no plaintext and no key stay in memory
//! between calls. Uploads idle for longer than the idle timeout are evicted
//! (with their file) at the start of the next upload call; `abort_all` drops
//! every upload, for example when the vault locks.
use super::model::{AttachmentSource, UPLOAD_IDLE_TIMEOUT};
use crate::StoreError;
use crate::sealed::BlobWriter;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant},
};

pub(crate) struct Upload {
    pub(crate) attachment_id: String,
    pub(crate) session_id: String,
    pub(crate) name: String,
    pub(crate) mime: Option<String>,
    pub(crate) source: Option<AttachmentSource>,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) received: u64,
    /// `None` once the upload was committed, aborted, or evicted.
    pub(crate) writer: Option<BlobWriter>,
    pub(crate) touched: Instant,
}

struct Entry {
    session_id: String,
    size: u64,
    upload: Arc<Mutex<Upload>>,
}

/// What is in flight when a new upload asks to be admitted.
pub(crate) struct InFlight {
    /// Uploads in flight in the daemon.
    pub(crate) count: usize,
    /// Declared bytes in flight in the daemon.
    pub(crate) bytes: u64,
    /// Uploads in flight for the new upload's session.
    pub(crate) session: usize,
}

struct Table {
    entries: HashMap<String, Entry>,
    idle_timeout: Duration,
}

pub(crate) struct Uploads {
    table: Mutex<Table>,
}

pub(crate) fn lock_upload(upload: &Mutex<Upload>) -> MutexGuard<'_, Upload> {
    // A panic mid-chunk leaves a file that may be incomplete; the writer is
    // still owned, so the upload can be aborted or will fail its commit checks.
    upload
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Uploads {
    pub(crate) fn new() -> Self {
        Self {
            table: Mutex::new(Table {
                entries: HashMap::new(),
                idle_timeout: UPLOAD_IDLE_TIMEOUT,
            }),
        }
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn set_idle_timeout(&self, timeout: Duration) {
        self.table().idle_timeout = timeout;
    }

    /// Drop the writers (and so the files) of removed uploads, outside the table lock.
    fn close(removed: Vec<Arc<Mutex<Upload>>>) -> usize {
        let mut closed = 0;
        for upload in removed {
            if lock_upload(&upload).writer.take().is_some() {
                closed += 1;
            }
        }
        closed
    }

    /// Evict uploads idle for longer than the idle timeout.
    pub(crate) fn evict_expired(&self) {
        let removed = {
            let mut table = self.table();
            let timeout = table.idle_timeout;
            let expired: Vec<String> = table
                .entries
                .iter()
                .filter(|(_, entry)| match entry.upload.try_lock() {
                    Ok(upload) => upload.touched.elapsed() >= timeout,
                    Err(TryLockError::Poisoned(_)) => true,
                    Err(TryLockError::WouldBlock) => false,
                })
                .map(|(id, _)| id.clone())
                .collect();
            expired
                .iter()
                .filter_map(|id| table.entries.remove(id))
                .map(|entry| entry.upload)
                .collect()
        };
        Self::close(removed);
    }

    /// Add an upload after `check` accepts what is in flight (called with the
    /// table locked, so admissions cannot race). A refused upload is dropped
    /// with its file.
    pub(crate) fn admit(
        &self,
        upload_id: String,
        upload: Upload,
        check: impl FnOnce(InFlight) -> Result<(), StoreError>,
    ) -> Result<(), StoreError> {
        let mut table = self.table();
        let in_flight = InFlight {
            count: table.entries.len(),
            bytes: table.entries.values().map(|e| e.size).sum(),
            session: table
                .entries
                .values()
                .filter(|e| e.session_id == upload.session_id)
                .count(),
        };
        check(in_flight)?;
        table.entries.insert(
            upload_id,
            Entry {
                session_id: upload.session_id.clone(),
                size: upload.size,
                upload: Arc::new(Mutex::new(upload)),
            },
        );
        Ok(())
    }

    /// Counts for a session's staged limit when bytes arrive outside an upload.
    pub(crate) fn in_flight(&self, session_id: &str) -> InFlight {
        let table = self.table();
        InFlight {
            count: table.entries.len(),
            bytes: table.entries.values().map(|e| e.size).sum(),
            session: table
                .entries
                .values()
                .filter(|e| e.session_id == session_id)
                .count(),
        }
    }

    pub(crate) fn get(&self, upload_id: &str) -> Option<Arc<Mutex<Upload>>> {
        self.table()
            .entries
            .get(upload_id)
            .map(|entry| Arc::clone(&entry.upload))
    }

    pub(crate) fn remove(&self, upload_id: &str) -> Option<Arc<Mutex<Upload>>> {
        self.table()
            .entries
            .remove(upload_id)
            .map(|entry| entry.upload)
    }

    /// Abort one upload; `false` when it is unknown or already finished.
    pub(crate) fn abort(&self, upload_id: &str) -> bool {
        let removed = self.remove(upload_id).into_iter().collect();
        Self::close(removed) > 0
    }

    /// Abort every upload of a session.
    pub(crate) fn abort_session(&self, session_id: &str) -> usize {
        let removed = {
            let mut table = self.table();
            let ids: Vec<String> = table
                .entries
                .iter()
                .filter(|(_, e)| e.session_id == session_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter()
                .filter_map(|id| table.entries.remove(id))
                .map(|e| e.upload)
                .collect()
        };
        Self::close(removed)
    }

    /// Abort every upload.
    pub(crate) fn abort_all(&self) -> usize {
        let removed = self
            .table()
            .entries
            .drain()
            .map(|(_, entry)| entry.upload)
            .collect();
        Self::close(removed)
    }
}
