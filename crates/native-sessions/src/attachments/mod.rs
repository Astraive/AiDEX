//! Encrypted chat attachments of native sessions: images, PDFs, and text files
//! a user sends with a message.
//!
//! The `attachments` table is additive (`CREATE TABLE IF NOT EXISTS`), so
//! daemons that predate it open the same database and never read it. A row
//! keeps in plaintext only the attachment id, its session id, its lifecycle
//! (`state`, `created_at`, `size`, which the store needs without the key: the
//! id already encodes the creation time and the sealed length reveals the
//! size), and for content above [`INLINE_ATTACHMENT_BYTES`] the content
//! address of its sealed blob in `<db stem>-attachments/`. Name, kind, MIME
//! type, hash, message link and bytes are sealed with the store's record key
//! under their own AAD domain, so attachments are unavailable while the vault
//! is locked. The table is the authoritative record of which message an
//! attachment belongs to; `Run::attachments` is only a cache.
mod model;
mod sniff;
mod upload;

pub use model::*;
pub use sniff::{Sniffed, sniff_attachment};
pub(crate) use upload::Uploads;

use crate::sealed::{
    self, BlobDir, Content, Domain, PageError, Stored, sealed_len, segment_count, segment_span,
};
use crate::{SessionDetail, SessionStore, StoreError, digest, id, now};
use chacha20poly1305::XChaCha20Poly1305;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};
use upload::{InFlight, Upload, lock_upload};
use zeroize::Zeroizing;

const DOMAIN: Domain = Domain::Attachment;

/// Schema migration run on every open; older daemons ignore the table.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS attachments(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,state TEXT NOT NULL,created_at INTEGER NOT NULL,size INTEGER NOT NULL,meta BLOB NOT NULL,content BLOB,blob TEXT); CREATE INDEX IF NOT EXISTS attachments_by_session ON attachments(session_id);";

/// Rows of sessions deleted by a daemon that predates the table.
pub(crate) const PURGE_ORPHANS: &str =
    "DELETE FROM attachments WHERE session_id NOT IN (SELECT id FROM sessions)";

/// Open-time maintenance; needs no key, so it also runs with the vault locked:
/// orphaned rows, then staged rows older than [`STAGED_TTL`], then the blob
/// sweep of `<db stem>-attachments/` (temporary files and unreferenced blobs).
pub(crate) fn open_maintenance(db: &Connection, blobs: &BlobDir) -> Result<(), StoreError> {
    db.execute_batch(SCHEMA).map_err(sql)?;
    db.execute(PURGE_ORPHANS, []).map_err(sql)?;
    for address in purge_expired_staged(db)? {
        blobs.remove(&address);
    }
    blobs.sweep(|address| {
        db.query_row(
            "SELECT EXISTS(SELECT 1 FROM attachments WHERE blob=?)",
            [address],
            |r| r.get(0),
        )
        .map_err(sql)
    });
    Ok(())
}

/// Delete staged rows older than [`STAGED_TTL`] (no key needed) and return
/// the blob addresses to remove once they are gone.
fn purge_expired_staged(db: &Connection) -> Result<Vec<String>, StoreError> {
    let cutoff = now().saturating_sub(STAGED_TTL.as_millis() as u64);
    let cutoff = i64::try_from(cutoff).unwrap_or(i64::MAX);
    let expired = {
        let mut stmt = db
            .prepare("SELECT blob FROM attachments WHERE state='staged' AND created_at<? AND blob IS NOT NULL")
            .map_err(sql)?;
        stmt.query_map([cutoff], |r| r.get::<_, String>(0))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?
    };
    db.execute(
        "DELETE FROM attachments WHERE state='staged' AND created_at<?",
        [cutoff],
    )
    .map_err(sql)?;
    Ok(expired)
}

/// When a running daemon last deleted expired staged attachments, so later
/// calls repeat it at most once per interval (default
/// [`STAGED_EXPIRY_INTERVAL`]). Open counts as the first time.
pub(crate) struct StagedExpiry(Mutex<(Instant, Duration)>);

impl StagedExpiry {
    pub(crate) fn new() -> Self {
        Self(Mutex::new((Instant::now(), STAGED_EXPIRY_INTERVAL)))
    }

    fn state(&self) -> MutexGuard<'_, (Instant, Duration)> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether an expiry is due; claims it when it is.
    fn claim(&self) -> bool {
        let mut state = self.state();
        let (last, interval) = &mut *state;
        if last.elapsed() < *interval {
            return false;
        }
        *last = Instant::now();
        true
    }
}

fn sql(error: rusqlite::Error) -> StoreError {
    error.to_string()
}

fn not_found() -> StoreError {
    "attachment not found".into()
}

fn upload_not_found() -> StoreError {
    "attachment upload not found".into()
}

/// The plaintext columns of one row.
struct Row {
    id: String,
    session_id: String,
    state: String,
    created_at: i64,
    size: i64,
    meta: Vec<u8>,
    blob: Option<String>,
}

const COLUMNS: &str = "a.id,a.session_id,a.state,a.created_at,a.size,a.meta,a.blob";

fn read_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: r.get(0)?,
        session_id: r.get(1)?,
        state: r.get(2)?,
        created_at: r.get(3)?,
        size: r.get(4)?,
        meta: r.get(5)?,
        blob: r.get(6)?,
    })
}

/// An attachment row whose session still exists.
fn row(db: &Connection, id: &str) -> Result<Option<Row>, StoreError> {
    db.query_row(
        &format!(
            "SELECT {COLUMNS} FROM attachments a JOIN sessions s ON s.id=a.session_id WHERE a.id=?"
        ),
        [id],
        read_row,
    )
    .optional()
    .map_err(sql)
}

/// Rows of one session matching an optional state, newest first.
fn session_rows(
    db: &Connection,
    session_id: &str,
    state: Option<AttachmentState>,
    limit: usize,
    offset: usize,
) -> Result<Vec<Row>, StoreError> {
    let mut stmt = db
        .prepare(&format!(
            "SELECT {COLUMNS} FROM attachments a WHERE a.session_id=?1 AND (?2 IS NULL OR a.state=?2) ORDER BY a.created_at DESC, a.rowid DESC LIMIT ?3 OFFSET ?4"
        ))
        .map_err(sql)?;
    stmt.query_map(
        params![
            session_id,
            state.map(AttachmentState::as_str),
            i64::try_from(limit).unwrap_or(i64::MAX),
            i64::try_from(offset).unwrap_or(i64::MAX)
        ],
        read_row,
    )
    .map_err(sql)?
    .collect::<Result<Vec<_>, _>>()
    .map_err(sql)
}

fn session_exists(db: &Connection, session_id: &str) -> Result<bool, StoreError> {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?)",
        [session_id],
        |r| r.get(0),
    )
    .map_err(sql)
}

fn check_session(db: &Connection, session_id: &str) -> Result<(), StoreError> {
    if session_exists(db, session_id)? {
        Ok(())
    } else {
        Err("session not found".into())
    }
}

fn staged_count(db: &Connection, session_id: &str) -> Result<usize, StoreError> {
    let n: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM attachments WHERE session_id=? AND state='staged'",
            [session_id],
            |r| r.get(0),
        )
        .map_err(sql)?;
    Ok(usize::try_from(n).unwrap_or(usize::MAX))
}

fn staged_bytes(db: &Connection) -> Result<u64, StoreError> {
    let n: i64 = db
        .query_row(
            "SELECT COALESCE(SUM(size),0) FROM attachments WHERE state='staged'",
            [],
            |r| r.get(0),
        )
        .map_err(sql)?;
    Ok(u64::try_from(n).unwrap_or(u64::MAX))
}

/// The staged limits for one more attachment of `size` bytes, counting the
/// uploads in flight.
fn check_staged(
    db: &Connection,
    session_id: &str,
    size: u64,
    in_flight: &InFlight,
) -> Result<(), StoreError> {
    if staged_count(db, session_id)? + in_flight.session >= MAX_STAGED_PER_SESSION {
        return Err(AttachmentReason::TooManyAttachments.error(format!(
            "a conversation can hold at most {MAX_STAGED_PER_SESSION} unsent attachments"
        )));
    }
    if staged_bytes(db)?
        .saturating_add(in_flight.bytes)
        .saturating_add(size)
        > MAX_STAGED_BYTES
    {
        return Err(AttachmentReason::TooManyAttachments
            .error("too many unsent attachment bytes are waiting; send or remove some first"));
    }
    Ok(())
}

fn seal_meta(cipher: &XChaCha20Poly1305, meta: &AttachmentMeta) -> Result<Vec<u8>, StoreError> {
    let plain = Zeroizing::new(serde_json::to_vec(meta).map_err(|e| e.to_string())?);
    DOMAIN.seal_record(cipher, &meta.id, &meta.session_id, &plain)
}

/// Open a row's sealed metadata; its identity and the plaintext lifecycle
/// columns must agree with it (fail closed otherwise).
fn open_meta(cipher: &XChaCha20Poly1305, row: &Row) -> Result<AttachmentMeta, StoreError> {
    let plain = DOMAIN.open_record(cipher, &row.id, &row.session_id, &row.meta)?;
    let meta: AttachmentMeta =
        serde_json::from_slice(&plain).map_err(|_| "invalid encrypted attachment metadata")?;
    if meta.id != row.id
        || meta.session_id != row.session_id
        || meta.state.as_str() != row.state
        || i64::try_from(meta.size).ok() != Some(row.size)
        || i64::try_from(meta.created_at).ok() != Some(row.created_at)
    {
        return Err("attachment record mismatch".into());
    }
    Ok(meta)
}

/// Check the ids a message would send (see
/// [`SessionStore::staged_attachments`]) and return their metadata in order.
fn checked_staged(
    db: &Connection,
    cipher: &XChaCha20Poly1305,
    session_id: &str,
    ids: &[String],
) -> Result<Vec<AttachmentMeta>, StoreError> {
    let unavailable = |detail: &str| AttachmentReason::AttachmentUnavailable.error(detail);
    if ids.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(unavailable(&format!(
            "at most {MAX_ATTACHMENTS_PER_MESSAGE} attachments can be sent with one message"
        )));
    }
    let mut metas = Vec::with_capacity(ids.len());
    let mut total = 0u64;
    for (position, id) in ids.iter().enumerate() {
        if ids[..position].contains(id) {
            return Err(unavailable("an attachment was listed twice"));
        }
        let row = row(db, id)?
            .filter(|row| row.session_id == session_id)
            .ok_or_else(|| unavailable("an attachment is not in this conversation"))?;
        if row.state != AttachmentState::Staged.as_str() {
            return Err(unavailable("an attachment was already sent"));
        }
        let meta = open_meta(cipher, &row)?;
        if meta.state != AttachmentState::Staged {
            return Err(unavailable("an attachment was already sent"));
        }
        if !meta.sendable() {
            return Err(unavailable(
                "an attachment cannot be sent by this version of AiDEX",
            ));
        }
        total = total.saturating_add(meta.size);
        metas.push(meta);
    }
    if total > MAX_MESSAGE_ATTACHMENT_BYTES {
        return Err(AttachmentReason::TooLarge.error(format!(
            "one message can carry at most {MAX_MESSAGE_ATTACHMENT_BYTES} attachment bytes"
        )));
    }
    Ok(metas)
}

/// The MIME hint of a client: `type/subtype`, lowercased, parameters dropped.
fn mime_hint(mime: Option<&str>) -> Option<String> {
    let mime = mime?.split(';').next()?.trim().to_ascii_lowercase();
    let (kind, subtype) = mime.split_once('/')?;
    let token = |part: &str| {
        (1..=63).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    (token(kind) && token(subtype)).then_some(mime)
}

fn check_upload_size(size: u64) -> Result<(), StoreError> {
    if size == 0 {
        return Err(AttachmentReason::TooLarge.error("the attachment is empty"));
    }
    if size > MAX_UPLOAD_BYTES {
        return Err(AttachmentReason::TooLarge.error(format!(
            "attachments are limited to {MAX_UPLOAD_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Read at most `limit` bytes into memory that is zeroed when dropped, even
/// when it grows; `None` when the reader has more.
fn read_bounded(
    reader: &mut dyn Read,
    limit: u64,
    size_hint: u64,
) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
    let capacity = usize::try_from(size_hint.min(limit)).unwrap_or(0) + 1;
    let mut out = Zeroizing::new(Vec::with_capacity(capacity));
    let mut block = Zeroizing::new(vec![0u8; 64 * 1024]);
    loop {
        let n = reader
            .read(&mut block)
            .map_err(|e| format!("attachment read failed: {}", e.kind()))?;
        if n == 0 {
            return Ok(Some(out));
        }
        if out.len() as u64 + n as u64 > limit {
            return Ok(None);
        }
        if out.len() + n > out.capacity() {
            let mut grown =
                Zeroizing::new(Vec::with_capacity((out.capacity() * 2).max(out.len() + n)));
            grown.extend_from_slice(&out);
            out = grown;
        }
        out.extend_from_slice(&block[..n]);
    }
}

/// Where new sealed content goes.
enum NewContent {
    Inline(Vec<u8>),
    Blob(String),
}

/// An attachment copy re-sealed for a fork, waiting for its row.
pub(crate) struct Resealed {
    pub(crate) meta: AttachmentMeta,
    content: NewContent,
}

impl Resealed {
    /// The blob written for it, to remove when its row is not inserted.
    pub(crate) fn blob(&self) -> Option<&str> {
        match &self.content {
            NewContent::Blob(address) => Some(address),
            NewContent::Inline(_) => None,
        }
    }
}

impl SessionStore {
    pub(crate) fn attachment_cipher(&self) -> Result<XChaCha20Poly1305, StoreError> {
        let key = self.key()?;
        sealed::cipher(key.expose())
    }

    /// Delete staged attachments older than [`STAGED_TTL`] with their blobs
    /// when an expiry is due, so drafts a client lost stop counting against
    /// the staged limits on a daemon that runs for days. Needs no key.
    fn expire_staged(&self) -> Result<(), StoreError> {
        if !self.staged_expiry.claim() {
            return Ok(());
        }
        let expired = {
            let db = self.db.lock().map_err(|_| "session store poisoned")?;
            purge_expired_staged(&db)?
        };
        for address in expired {
            self.attachment_blobs.remove(&address);
        }
        Ok(())
    }

    /// Change how often a running daemon deletes expired staged attachments
    /// (default [`STAGED_EXPIRY_INTERVAL`]).
    pub fn set_staged_expiry_interval(&self, interval: Duration) {
        self.staged_expiry.state().1 = interval;
    }

    /// Insert a new `staged` row after re-checking, in one transaction, that
    /// the session exists and the staged limits hold. A blob written for it is
    /// removed when the insert fails.
    fn insert_staged(
        &self,
        cipher: &XChaCha20Poly1305,
        meta: &AttachmentMeta,
        content: NewContent,
        in_flight: &InFlight,
    ) -> Result<(), StoreError> {
        let (inline, blob) = match content {
            NewContent::Inline(sealed) => (Some(sealed), None),
            NewContent::Blob(address) => (None, Some(address)),
        };
        let inserted = (|| {
            let sealed = seal_meta(cipher, meta)?;
            let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
            let tx = db.transaction().map_err(sql)?;
            check_session(&tx, &meta.session_id)?;
            check_staged(&tx, &meta.session_id, meta.size, in_flight)?;
            tx.execute(
                "INSERT INTO attachments(id,session_id,state,created_at,size,meta,content,blob) VALUES(?,?,?,?,?,?,?,?)",
                params![
                    meta.id,
                    meta.session_id,
                    meta.state.as_str(),
                    i64::try_from(meta.created_at).unwrap_or(i64::MAX),
                    i64::try_from(meta.size).unwrap_or(i64::MAX),
                    sealed,
                    inline,
                    blob
                ],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)
        })();
        if let Err(error) = inserted {
            if let Some(blob) = &blob {
                self.attachment_blobs.remove(blob);
            }
            return Err(error);
        }
        Ok(())
    }

    fn staged_meta(
        attachment_id: String,
        session_id: &str,
        name: String,
        sniffed: Sniffed,
        size: u64,
        sha256: String,
        source: Option<AttachmentSource>,
    ) -> AttachmentMeta {
        AttachmentMeta {
            id: attachment_id,
            session_id: session_id.into(),
            name,
            kind: sniffed.kind,
            mime: sniffed.mime,
            size,
            sha256,
            pages: sniffed.pages,
            width: sniffed.width,
            height: sniffed.height,
            animated: sniffed.animated,
            created_at: now(),
            state: AttachmentState::Staged,
            source,
            command_id: None,
            run_id: None,
            message_index: None,
            position: None,
            excluded: false,
        }
    }

    /// Start a chunked upload into `session_id`. Every chunk but the last must
    /// be exactly [`UploadTicket::chunk_bytes`] long. Refusals:
    /// `session not found`, `too_large` (empty or over [`MAX_UPLOAD_BYTES`]),
    /// `invalid_name`, `invalid_attachment` (SHA-256), `too_many_attachments`
    /// (staged limits, uploads in flight included), `busy` (more than
    /// [`MAX_INFLIGHT_UPLOADS`] or [`MAX_INFLIGHT_UPLOAD_BYTES`] in flight), and
    /// a locked vault. Expired staged attachments are deleted first when due.
    pub fn begin_attachment_upload(&self, new: NewUpload) -> Result<UploadTicket, StoreError> {
        self.uploads.evict_expired();
        self.expire_staged()?;
        let name = attachment_name(&new.name)?;
        check_upload_size(new.size)?;
        let sha256 = model::normalize_sha256(&new.sha256)?;
        // Fails with a locked vault before anything is created; nothing keeps it.
        self.key()?;
        let writer = self.attachment_blobs.writer()?;
        let upload_id = id("upl")?;
        let upload = Upload {
            attachment_id: id("att")?,
            session_id: new.session_id.clone(),
            name,
            mime: mime_hint(new.mime.as_deref()),
            source: new.source,
            size: new.size,
            sha256,
            received: 0,
            writer: Some(writer),
            touched: Instant::now(),
        };
        self.uploads.admit(upload_id.clone(), upload, |in_flight| {
            let db = self.db.lock().map_err(|_| "session store poisoned")?;
            check_session(&db, &new.session_id)?;
            check_staged(&db, &new.session_id, new.size, &in_flight)?;
            if in_flight.count >= MAX_INFLIGHT_UPLOADS
                || in_flight.bytes.saturating_add(new.size) > MAX_INFLIGHT_UPLOAD_BYTES
            {
                return Err(AttachmentReason::Busy
                    .error("too many uploads are in progress; try again when they finish"));
            }
            Ok(())
        })?;
        Ok(UploadTicket {
            upload_id,
            chunk_bytes: UPLOAD_CHUNK_BYTES,
        })
    }

    /// Seal and append a chunk at `offset` (which must equal the bytes
    /// received so far); returns the bytes received. A chunk repeated at an
    /// earlier chunk offset with that chunk's length is acknowledged without
    /// writing (an idempotent retry). Everything else is `bad_offset`. The key
    /// is resolved per chunk; when it cannot be (a locked vault), the upload is
    /// dropped with its file.
    pub fn append_upload(
        &self,
        upload_id: &str,
        offset: u64,
        data: &[u8],
    ) -> Result<u64, StoreError> {
        self.uploads.evict_expired();
        let entry = self.uploads.get(upload_id).ok_or_else(upload_not_found)?;
        let mut upload = lock_upload(&entry);
        if upload.writer.is_none() {
            return Err(upload_not_found());
        }
        let chunk = UPLOAD_CHUNK_BYTES as u64;
        let size = upload.size;
        let expected = |at: u64| chunk.min(size - at);
        let len = data.len() as u64;
        if offset < upload.received {
            if offset.is_multiple_of(chunk) && len == expected(offset) {
                upload.touched = Instant::now();
                return Ok(upload.received);
            }
            return Err(AttachmentReason::BadOffset.error(format!(
                "{} bytes were received; resend from there",
                upload.received
            )));
        }
        if offset > upload.received || offset >= size {
            return Err(AttachmentReason::BadOffset.error(format!(
                "{} bytes were received; resend from there",
                upload.received
            )));
        }
        if len != expected(offset) {
            return Err(AttachmentReason::BadOffset.error(format!(
                "every chunk but the last must be exactly {chunk} bytes"
            )));
        }
        let sealed = (|| {
            let cipher = self.attachment_cipher()?;
            let first = offset / sealed::SEGMENT_BYTES;
            let mut out = Vec::with_capacity(data.len() + 4 * sealed::OVERHEAD as usize);
            for (index, segment) in data.chunks(sealed::SEGMENT_BYTES as usize).enumerate() {
                out.extend(DOMAIN.seal_segment(
                    &cipher,
                    &upload.attachment_id,
                    &upload.sha256,
                    size,
                    first + index as u64,
                    segment,
                )?);
            }
            Ok::<_, StoreError>(out)
        })();
        let written = sealed.and_then(|sealed| match upload.writer.as_mut() {
            Some(writer) => writer.write(&sealed),
            None => Err(upload_not_found()),
        });
        if let Err(error) = written {
            // Fail closed: a locked vault or a failed write ends the upload.
            upload.writer = None;
            drop(upload);
            self.uploads.remove(upload_id);
            return Err(error);
        }
        upload.received += len;
        upload.touched = Instant::now();
        Ok(upload.received)
    }

    /// Finish an upload: verify the declared size and SHA-256
    /// (`hash_mismatch`), decide the kind from the bytes (`unsupported_type`,
    /// `sensitive_content`, `too_large`, `too_many_pages`), and store it
    /// `staged`. An incomplete upload is refused with `bad_offset` and kept;
    /// any other failure ends it.
    pub fn commit_upload(&self, upload_id: &str) -> Result<AttachmentMeta, StoreError> {
        self.uploads.evict_expired();
        {
            let entry = self.uploads.get(upload_id).ok_or_else(upload_not_found)?;
            let upload = lock_upload(&entry);
            if upload.writer.is_none() {
                return Err(upload_not_found());
            }
            if upload.received != upload.size {
                return Err(AttachmentReason::BadOffset.error(format!(
                    "the upload is incomplete: {} of {} bytes were received",
                    upload.received, upload.size
                )));
            }
        }
        let entry = self
            .uploads
            .remove(upload_id)
            .ok_or_else(upload_not_found)?;
        let mut upload = lock_upload(&entry);
        let mut writer = upload.writer.take().ok_or_else(upload_not_found)?;
        let in_flight = self.uploads.in_flight(&upload.session_id);
        let cipher = self.attachment_cipher()?;
        let size = upload.size;
        if writer.written() != sealed_len(size) {
            return Err(DOMAIN.truncated());
        }
        let mut plain = Zeroizing::new(Vec::with_capacity(usize::try_from(size).unwrap_or(0)));
        for index in 0..segment_count(size) {
            let (start, len) = segment_span(index, size);
            let sealed = writer.read_at(start, len)?;
            plain.extend_from_slice(&DOMAIN.open_segment(
                &cipher,
                &upload.attachment_id,
                &upload.sha256,
                size,
                index,
                &sealed,
            )?);
        }
        if digest(&plain) != upload.sha256 {
            return Err(AttachmentReason::HashMismatch
                .error("the uploaded bytes do not match the declared SHA-256"));
        }
        let sniffed = sniff_attachment(&plain, upload.mime.as_deref())?;
        drop(plain);
        let content = if size <= INLINE_ATTACHMENT_BYTES as u64 {
            NewContent::Inline(writer.read_at(0, sealed_len(size))?)
        } else {
            NewContent::Blob(writer.finish()?)
        };
        let name = std::mem::take(&mut upload.name);
        let meta = Self::staged_meta(
            upload.attachment_id.clone(),
            &upload.session_id,
            name,
            sniffed,
            size,
            upload.sha256.clone(),
            upload.source,
        );
        self.insert_staged(&cipher, &meta, content, &in_flight)?;
        Ok(meta)
    }

    /// Abort an upload and delete its file. Needs no key, so it also works
    /// while the vault is locked; `false` when the upload is unknown.
    pub fn abort_upload(&self, upload_id: &str) -> bool {
        self.uploads.abort(upload_id)
    }

    /// Abort every upload in flight (call it when the vault locks); returns how
    /// many were aborted.
    pub fn abort_all_uploads(&self) -> usize {
        self.uploads.abort_all()
    }

    /// Change the idle timeout of in-flight uploads (default
    /// [`UPLOAD_IDLE_TIMEOUT`]).
    pub fn set_upload_idle_timeout(&self, timeout: std::time::Duration) {
        self.uploads.set_idle_timeout(timeout);
    }

    /// Store bytes already in memory as a `staged` attachment of `session_id`
    /// (for example a workspace file read through its confined root), with
    /// the same name rules, kind sniffing and limits as an upload.
    pub fn create_attachment(
        &self,
        session_id: &str,
        new: NewAttachment,
    ) -> Result<AttachmentMeta, StoreError> {
        let name = attachment_name(&new.name)?;
        let size = new.bytes.len() as u64;
        check_upload_size(size)?;
        self.expire_staged()?;
        let cipher = self.attachment_cipher()?;
        {
            let db = self.db.lock().map_err(|_| "session store poisoned")?;
            check_session(&db, session_id)?;
        }
        let mime = mime_hint(new.mime.as_deref());
        let sniffed = sniff_attachment(&new.bytes, mime.as_deref())?;
        let meta = Self::staged_meta(
            id("att")?,
            session_id,
            name,
            sniffed,
            size,
            digest(&new.bytes),
            new.source,
        );
        let sealed = DOMAIN.seal_content(&cipher, &meta.id, &meta.sha256, &new.bytes)?;
        let content = if size <= INLINE_ATTACHMENT_BYTES as u64 {
            NewContent::Inline(sealed)
        } else {
            NewContent::Blob(self.attachment_blobs.write(&sealed)?)
        };
        let in_flight = self.uploads.in_flight(session_id);
        self.insert_staged(&cipher, &meta, content, &in_flight)?;
        Ok(meta)
    }

    /// [`create_attachment`](Self::create_attachment) from a reader, reading at
    /// most [`MAX_UPLOAD_BYTES`] (plus one byte to detect a larger file) into
    /// memory that is zeroed on drop. `size_hint` (for example the file length)
    /// only sizes the buffer.
    pub fn create_attachment_from_reader(
        &self,
        session_id: &str,
        name: &str,
        mime: Option<&str>,
        source: Option<AttachmentSource>,
        reader: &mut dyn Read,
        size_hint: u64,
    ) -> Result<AttachmentMeta, StoreError> {
        let name = attachment_name(name)?;
        self.key()?;
        let Some(bytes) = read_bounded(reader, MAX_UPLOAD_BYTES, size_hint)? else {
            return Err(AttachmentReason::TooLarge.error(format!(
                "attachments are limited to {MAX_UPLOAD_BYTES} bytes"
            )));
        };
        self.create_attachment(
            session_id,
            NewAttachment {
                name,
                mime: mime.map(Into::into),
                source,
                bytes,
            },
        )
    }

    /// Metadata of one attachment.
    pub fn attachment(&self, id: &str) -> Result<AttachmentMeta, StoreError> {
        let cipher = self.attachment_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        let row = row(&db, id)?.ok_or_else(not_found)?;
        open_meta(&cipher, &row)
    }

    /// A page of `session_id`'s attachments (staged and sent), newest first
    /// (`limit` 1-[`ATTACHMENT_LIST_LIMIT`]). A row whose metadata cannot be
    /// read (for example one a newer daemon wrote) is skipped. Expired staged
    /// attachments are deleted first when due.
    pub fn list_attachments(
        &self,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<AttachmentPage, StoreError> {
        if !(1..=ATTACHMENT_LIST_LIMIT).contains(&limit) {
            return Err(AttachmentReason::InvalidAttachment.error(format!(
                "attachment page limits are 1-{ATTACHMENT_LIST_LIMIT}"
            )));
        }
        self.expire_staged()?;
        let cipher = self.attachment_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        check_session(&db, session_id)?;
        let total: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM attachments WHERE session_id=?",
                [session_id],
                |r| r.get(0),
            )
            .map_err(sql)?;
        let total = usize::try_from(total).unwrap_or_default();
        let offset = offset.min(total);
        let rows = session_rows(&db, session_id, None, limit, offset)?;
        let end = offset + rows.len();
        Ok(AttachmentPage {
            attachments: rows
                .iter()
                .filter_map(|row| open_meta(&cipher, row).ok())
                .collect(),
            total,
            offset,
            next_offset: (end < total).then_some(end),
        })
    }

    /// The `sent` attachments of `session_id` by message index, each list in
    /// the order they were sent. Rows that cannot be read are skipped.
    pub fn message_attachments(
        &self,
        session_id: &str,
    ) -> Result<BTreeMap<usize, Vec<AttachmentMeta>>, StoreError> {
        let cipher = self.attachment_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        check_session(&db, session_id)?;
        let rows = session_rows(
            &db,
            session_id,
            Some(AttachmentState::Sent),
            usize::MAX >> 1,
            0,
        )?;
        drop(db);
        let mut out: BTreeMap<usize, Vec<AttachmentMeta>> = BTreeMap::new();
        for meta in rows.iter().filter_map(|row| open_meta(&cipher, row).ok()) {
            if let Some(index) = meta.message_index {
                out.entry(index).or_default().push(meta);
            }
        }
        for list in out.values_mut() {
            list.sort_by(|a, b| {
                (a.position, a.created_at, &a.id).cmp(&(b.position, b.created_at, &b.id))
            });
        }
        Ok(out)
    }

    /// The `sent` attachments of the user message at `message_index`, in the
    /// order they were sent.
    pub fn attachments_for_message(
        &self,
        session_id: &str,
        message_index: usize,
    ) -> Result<Vec<AttachmentMeta>, StoreError> {
        Ok(self
            .message_attachments(session_id)?
            .remove(&message_index)
            .unwrap_or_default())
    }

    /// Up to `limit` plaintext bytes from `offset`: at most
    /// [`ATTACHMENT_TEXT_PAGE_BYTES`] for text (pages start and end on
    /// character boundaries and always make progress) and
    /// [`ATTACHMENT_PAGE_BYTES`] otherwise.
    pub fn read_attachment(
        &self,
        id: &str,
        offset: u64,
        limit: usize,
    ) -> Result<AttachmentChunk, StoreError> {
        if limit == 0 {
            return Err(
                AttachmentReason::InvalidAttachment.error("the read limit must be positive")
            );
        }
        let cipher = self.attachment_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        let row = row(&db, id)?.ok_or_else(not_found)?;
        let meta = open_meta(&cipher, &row)?;
        let utf8 = meta.kind == AttachmentKind::Text;
        let cap = if utf8 {
            ATTACHMENT_TEXT_PAGE_BYTES
        } else {
            ATTACHMENT_PAGE_BYTES
        };
        let content = Content {
            domain: DOMAIN,
            id: &meta.id,
            sha256: &meta.sha256,
            size: meta.size,
        };
        let page = sealed::page(
            meta.size,
            offset,
            limit.min(cap) as u64,
            utf8,
            |start, end| {
                if start >= end {
                    return Ok(Zeroizing::new(vec![]));
                }
                self.with_stored(&db, &row, |stored| {
                    content.window(&cipher, stored, start, end)
                })
            },
        )?;
        let (bytes, next_offset) = match page {
            Ok(page) => page,
            Err(PageError::Beyond) => {
                return Err(AttachmentReason::InvalidAttachment
                    .error("the offset is beyond the attachment size"));
            }
            Err(PageError::InsideCharacter) => {
                return Err(AttachmentReason::InvalidAttachment
                    .error("the offset is inside a UTF-8 character"));
            }
        };
        Ok(AttachmentChunk {
            id: meta.id.clone(),
            bytes,
            encoding: if utf8 {
                AttachmentEncoding::Utf8
            } else {
                AttachmentEncoding::Base64
            },
            offset,
            next_offset,
            total_bytes: meta.size,
            sha256: meta.sha256.clone(),
        })
    }

    /// The whole plaintext of an attachment, for prompt assembly.
    pub fn attachment_bytes(&self, id: &str) -> Result<AttachmentBytes, StoreError> {
        let cipher = self.attachment_cipher()?;
        // Decrypt outside the database lock: prompts read up to 16 MiB.
        let (row, inline) = {
            let db = self.db.lock().map_err(|_| "session store poisoned")?;
            let row = row(&db, id)?.ok_or_else(not_found)?;
            let inline: Option<Vec<u8>> = match row.blob {
                Some(_) => None,
                None => db
                    .query_row(
                        "SELECT content FROM attachments WHERE id=?",
                        [&row.id],
                        |r| r.get::<_, Option<Vec<u8>>>(0),
                    )
                    .map_err(sql)?
                    .or_else(|| Some(vec![])),
            };
            (row, inline)
        };
        let meta = open_meta(&cipher, &row)?;
        let content = Content {
            domain: DOMAIN,
            id: &meta.id,
            sha256: &meta.sha256,
            size: meta.size,
        };
        let stored = match (&inline, &row.blob) {
            (Some(sealed), _) => Stored::Inline(sealed),
            (None, Some(address)) => Stored::Blob {
                dir: &self.attachment_blobs,
                address,
            },
            (None, None) => return Err(DOMAIN.truncated()),
        };
        let bytes = content.window(&cipher, &stored, 0, meta.size)?;
        if digest(&bytes) != meta.sha256 {
            return Err("attachment authentication failed".into());
        }
        Ok(AttachmentBytes { meta, bytes })
    }

    /// Run `f` with the row's sealed content (the inline column or its blob).
    fn with_stored<T>(
        &self,
        db: &Connection,
        row: &Row,
        f: impl FnOnce(&Stored<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        match &row.blob {
            Some(address) => f(&Stored::Blob {
                dir: &self.attachment_blobs,
                address,
            }),
            None => {
                let inline: Option<Vec<u8>> = db
                    .query_row(
                        "SELECT content FROM attachments WHERE id=?",
                        [&row.id],
                        |r| r.get(0),
                    )
                    .map_err(sql)?;
                f(&Stored::Inline(&inline.unwrap_or_default()))
            }
        }
    }

    /// Delete a `staged` attachment and its blob; `false` when no such
    /// attachment exists. Sent attachments live and die with their
    /// conversation (`attachment_sent`).
    pub fn delete_staged_attachment(&self, id: &str) -> Result<bool, StoreError> {
        let cipher = self.attachment_cipher()?;
        let blob = {
            let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
            let tx = db.transaction().map_err(sql)?;
            let Some(row) = row(&tx, id)? else {
                return Ok(false);
            };
            let meta = open_meta(&cipher, &row)?;
            if meta.state != AttachmentState::Staged {
                return Err(AttachmentReason::AttachmentSent
                    .error("sent attachments are deleted with their conversation"));
            }
            tx.execute(
                "DELETE FROM attachments WHERE id=? AND state='staged'",
                [id],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            row.blob
        };
        if let Some(blob) = blob {
            self.attachment_blobs.remove(&blob);
        }
        Ok(true)
    }

    /// Keep a sent attachment out of (or back in) the model context. It stays
    /// in the transcript; prompts show a placeholder instead of its content.
    pub fn set_attachment_excluded(
        &self,
        id: &str,
        excluded: bool,
    ) -> Result<AttachmentMeta, StoreError> {
        let cipher = self.attachment_cipher()?;
        let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
        let tx = db.transaction().map_err(sql)?;
        let row = row(&tx, id)?.ok_or_else(not_found)?;
        let mut meta = open_meta(&cipher, &row)?;
        if meta.state != AttachmentState::Sent {
            return Err(AttachmentReason::InvalidAttachment
                .error("only sent attachments can be excluded; remove a staged one instead"));
        }
        if meta.excluded != excluded {
            meta.excluded = excluded;
            tx.execute(
                "UPDATE attachments SET meta=? WHERE id=?",
                params![seal_meta(&cipher, &meta)?, id],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        Ok(meta)
    }

    /// The staged attachments `ids` of `session_id`, in the given order,
    /// checked exactly as [`begin_with`](Self::begin_with) checks a new
    /// command's attachments, without changing them: distinct ids of staged,
    /// sendable attachments of the session (`attachment_unavailable`), at most
    /// [`MAX_ATTACHMENTS_PER_MESSAGE`] (`attachment_unavailable`) and
    /// [`MAX_MESSAGE_ATTACHMENT_BYTES`] in total (`too_large`). The harness
    /// calls it before `begin_with` to gate kinds by model; it must skip the
    /// call for a command id the session already records, because a retry
    /// finds its ids already `sent` and `begin_with` returns the original run.
    pub fn staged_attachments(
        &self,
        session_id: &str,
        ids: &[String],
    ) -> Result<Vec<AttachmentMeta>, StoreError> {
        let cipher = self.attachment_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        check_session(&db, session_id)?;
        checked_staged(&db, &cipher, session_id, ids)
    }

    /// Mark staged attachments `sent` for a new run's user message, inside the
    /// caller's transaction, after the checks of
    /// [`staged_attachments`](Self::staged_attachments).
    pub(crate) fn link_attachments(
        &self,
        db: &Connection,
        session: &SessionDetail,
        ids: &[String],
        command_id: &str,
        run_id: &str,
        message_index: usize,
    ) -> Result<(), StoreError> {
        let cipher = self.attachment_cipher()?;
        let mut metas = checked_staged(db, &cipher, &session.id, ids)?;
        for (position, meta) in metas.iter_mut().enumerate() {
            meta.state = AttachmentState::Sent;
            meta.command_id = Some(command_id.into());
            meta.run_id = Some(run_id.into());
            meta.message_index = Some(message_index);
            meta.position = u32::try_from(position).ok();
        }
        let unavailable = |detail: &str| AttachmentReason::AttachmentUnavailable.error(detail);
        for meta in metas {
            let changed = db
                .execute(
                    "UPDATE attachments SET state='sent',meta=? WHERE id=? AND session_id=? AND state='staged'",
                    params![seal_meta(&cipher, &meta)?, meta.id, meta.session_id],
                )
                .map_err(sql)?;
            if changed != 1 {
                return Err(unavailable("an attachment was already sent"));
            }
        }
        Ok(())
    }

    /// Delete a session's attachment rows inside the caller's transaction and
    /// return the blob addresses to remove once it commits.
    pub(crate) fn delete_session_attachments(
        db: &Connection,
        session_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        let mut stmt = db
            .prepare("SELECT blob FROM attachments WHERE session_id=? AND blob IS NOT NULL")
            .map_err(sql)?;
        let blobs = stmt
            .query_map([session_id], |r| r.get::<_, String>(0))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        db.execute("DELETE FROM attachments WHERE session_id=?", [session_id])
            .map_err(sql)?;
        Ok(blobs)
    }

    /// The readable `sent` attachments of a session with their stored
    /// location, for a fork to re-seal.
    pub(crate) fn sent_attachment_rows(
        &self,
        cipher: &XChaCha20Poly1305,
        session_id: &str,
    ) -> Result<Vec<(AttachmentMeta, Option<String>)>, StoreError> {
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        let rows = session_rows(
            &db,
            session_id,
            Some(AttachmentState::Sent),
            usize::MAX >> 1,
            0,
        )?;
        Ok(rows
            .into_iter()
            .filter_map(|row| open_meta(cipher, &row).ok().map(|meta| (meta, row.blob)))
            .collect())
    }

    /// Re-seal one attachment's content under `copy`'s id, segment by
    /// segment, as inline bytes or a new blob.
    pub(crate) fn reseal_attachment(
        &self,
        cipher: &XChaCha20Poly1305,
        source: &AttachmentMeta,
        blob: Option<&str>,
        copy: AttachmentMeta,
    ) -> Result<Resealed, StoreError> {
        let size = source.size;
        let from = Content {
            domain: DOMAIN,
            id: &source.id,
            sha256: &source.sha256,
            size,
        };
        let inline = match blob {
            Some(_) => None,
            None => {
                let db = self.db.lock().map_err(|_| "session store poisoned")?;
                let content: Option<Vec<u8>> = db
                    .query_row(
                        "SELECT content FROM attachments WHERE id=?",
                        [&source.id],
                        |r| r.get(0),
                    )
                    .map_err(sql)?;
                Some(content.unwrap_or_default())
            }
        };
        let stored = match (&inline, blob) {
            (Some(content), _) => Stored::Inline(content),
            (None, Some(address)) => Stored::Blob {
                dir: &self.attachment_blobs,
                address,
            },
            (None, None) => return Err(DOMAIN.truncated()),
        };
        let reseal = |index: u64| -> Result<Vec<u8>, StoreError> {
            let sealed = from.sealed_segment(&stored, index)?;
            let plain =
                DOMAIN.open_segment(cipher, &source.id, &source.sha256, size, index, &sealed)?;
            DOMAIN.seal_segment(cipher, &copy.id, &copy.sha256, size, index, &plain)
        };
        if size <= INLINE_ATTACHMENT_BYTES as u64 {
            let mut out = Vec::with_capacity(usize::try_from(sealed_len(size)).unwrap_or(0));
            for index in 0..segment_count(size) {
                out.extend(reseal(index)?);
            }
            Ok(Resealed {
                meta: copy,
                content: NewContent::Inline(out),
            })
        } else {
            let mut writer = self.attachment_blobs.writer()?;
            for index in 0..segment_count(size) {
                writer.write(&reseal(index)?)?;
            }
            Ok(Resealed {
                content: NewContent::Blob(writer.finish()?),
                meta: copy,
            })
        }
    }

    /// Insert re-sealed rows of a fork inside the caller's transaction.
    pub(crate) fn insert_copies(
        db: &Connection,
        cipher: &XChaCha20Poly1305,
        copies: &[Resealed],
    ) -> Result<(), StoreError> {
        for Resealed { meta, content } in copies {
            let (inline, blob) = match content {
                NewContent::Inline(sealed) => (Some(sealed), None),
                NewContent::Blob(address) => (None, Some(address)),
            };
            db.execute(
                "INSERT INTO attachments(id,session_id,state,created_at,size,meta,content,blob) VALUES(?,?,?,?,?,?,?,?)",
                params![
                    meta.id,
                    meta.session_id,
                    meta.state.as_str(),
                    i64::try_from(meta.created_at).unwrap_or(i64::MAX),
                    i64::try_from(meta.size).unwrap_or(i64::MAX),
                    seal_meta(cipher, meta)?,
                    inline,
                    blob
                ],
            )
            .map_err(sql)?;
        }
        Ok(())
    }
}
