//! Encrypted artifacts of native sessions: documents, code, data, diagrams,
//! plans, and saved web pages.
//!
//! The `artifacts` table is additive (`CREATE TABLE IF NOT EXISTS`), so daemons
//! that predate it open the same database and simply never read it. A row keeps
//! only the artifact id, its session id, and, for content above
//! [`INLINE_ARTIFACT_BYTES`], the content address of its sealed blob file in
//! plaintext. Metadata (name, kind, MIME type, plan status, ...) and bytes are
//! sealed with the store's record key, so artifacts are unavailable while the
//! vault is locked, exactly like session history.
mod model;

pub use model::{
    ARTIFACT_LIST_LIMIT, ARTIFACT_PAGE_BYTES, ArtifactChunk, ArtifactEncoding, ArtifactKind,
    ArtifactMeta, ArtifactPage, INLINE_ARTIFACT_BYTES, MAX_ARTIFACT_BYTES,
    MAX_ARTIFACT_LANGUAGE_BYTES, MAX_ARTIFACT_NAME_BYTES, NewArtifact, PlanStatus,
};

use crate::sealed::{self, Content, Domain, PageError, Stored};
use crate::{SessionStore, StoreError, digest, id, now};
use chacha20poly1305::XChaCha20Poly1305;
use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

const DOMAIN: Domain = Domain::Artifact;

/// Schema migration run on every open; older daemons ignore the table.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS artifacts(id TEXT PRIMARY KEY,session_id TEXT NOT NULL,meta BLOB NOT NULL,content BLOB,blob TEXT); CREATE INDEX IF NOT EXISTS artifacts_by_session ON artifacts(session_id);";

/// Run on every open: rows of sessions deleted by a daemon that predates the
/// table (it deletes only `runs` and `sessions`).
pub(crate) const PURGE_ORPHANS: &str =
    "DELETE FROM artifacts WHERE session_id NOT IN (SELECT id FROM sessions)";

/// Plaintext columns of one artifact row (the sealed content is read separately).
struct Row {
    session_id: String,
    meta: Vec<u8>,
    blob: Option<String>,
}

fn sql(error: rusqlite::Error) -> StoreError {
    error.to_string()
}

impl SessionStore {
    /// Seal and store a new artifact of `session_id` (and of `run_id`, which must
    /// belong to that session). A `plan` starts `active` and supersedes the
    /// session's previous active plan in the same transaction.
    pub fn create_artifact(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        new: NewArtifact,
    ) -> Result<ArtifactMeta, StoreError> {
        let described = model::describe(&new)?;
        let cipher = self.artifact_cipher()?;
        let at = now();
        let meta = ArtifactMeta {
            id: id("art")?,
            session_id: session_id.into(),
            run_id: run_id.map(Into::into),
            name: described.name,
            kind: new.kind,
            mime: described.mime,
            language: described.language,
            size: new.bytes.len() as u64,
            sha256: digest(&new.bytes),
            encoding: model::encoding_of(&new.bytes),
            plan_status: (new.kind == ArtifactKind::Plan).then_some(PlanStatus::Active),
            created_at: at,
            updated_at: at,
        };
        let (content, blob) = self.seal_content(&cipher, &meta.id, &meta.sha256, &new.bytes)?;
        let inserted = (|| {
            let sealed = seal_meta(&cipher, &meta)?;
            let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
            let tx = db.transaction().map_err(sql)?;
            check_owner(&tx, &meta.session_id, meta.run_id.as_deref())?;
            if meta.kind == ArtifactKind::Plan {
                supersede_active_plans(&tx, &cipher, &meta.session_id, None)?;
            }
            tx.execute(
                "INSERT INTO artifacts(id,session_id,meta,content,blob) VALUES(?,?,?,?,?)",
                params![meta.id, meta.session_id, sealed, content, blob],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)
        })();
        if let Err(error) = inserted {
            if let Some(blob) = &blob {
                self.blobs.remove(blob);
            }
            return Err(error);
        }
        Ok(meta)
    }

    /// A page of `session_id`'s artifacts, newest first (`limit` 1-100).
    pub fn list_artifacts(
        &self,
        session_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<ArtifactPage, StoreError> {
        if !(1..=ARTIFACT_LIST_LIMIT).contains(&limit) {
            return Err(format!(
                "invalid artifact page limit: 1-{ARTIFACT_LIST_LIMIT}"
            ));
        }
        let cipher = self.artifact_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        check_owner(&db, session_id, None)?;
        let total: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM artifacts WHERE session_id=?",
                [session_id],
                |r| r.get(0),
            )
            .map_err(sql)?;
        let total = usize::try_from(total).unwrap_or_default();
        let offset = offset.min(total);
        let mut stmt = db
            .prepare(
                "SELECT id,meta FROM artifacts WHERE session_id=? ORDER BY rowid DESC LIMIT ? OFFSET ?",
            )
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![session_id, limit as i64, offset as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })
            .map_err(sql)?;
        let mut artifacts = vec![];
        for row in rows {
            let (id, sealed) = row.map_err(sql)?;
            artifacts.push(open_meta(&cipher, &id, session_id, &sealed)?);
        }
        let end = offset + artifacts.len();
        Ok(ArtifactPage {
            artifacts,
            total,
            offset,
            next_offset: (end < total).then_some(end),
        })
    }

    /// Metadata of one artifact.
    pub fn artifact(&self, id: &str) -> Result<ArtifactMeta, StoreError> {
        let cipher = self.artifact_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        let row = row(&db, id)?;
        open_meta(&cipher, id, &row.session_id, &row.meta)
    }

    /// Up to `limit` (at most [`ARTIFACT_PAGE_BYTES`]) plaintext bytes from
    /// `offset`. Pages of `utf8` artifacts start and end on character
    /// boundaries; a page always makes progress unless `offset` is the end.
    pub fn read_artifact(
        &self,
        id: &str,
        offset: u64,
        limit: usize,
    ) -> Result<ArtifactChunk, StoreError> {
        if limit == 0 {
            return Err("invalid artifact read limit: must be positive".into());
        }
        let limit = limit.min(ARTIFACT_PAGE_BYTES) as u64;
        let cipher = self.artifact_cipher()?;
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        let row = row(&db, id)?;
        let meta = open_meta(&cipher, id, &row.session_id, &row.meta)?;
        let size = meta.size;
        let utf8 = meta.encoding == ArtifactEncoding::Utf8;
        let page = sealed::page(size, offset, limit, utf8, |start, end| {
            self.plaintext(&db, &cipher, &meta, row.blob.as_deref(), start, end)
        })?;
        let (bytes, next_offset) = match page {
            Ok(page) => page,
            Err(PageError::Beyond) => {
                return Err("invalid artifact offset: beyond the artifact size".into());
            }
            Err(PageError::InsideCharacter) => {
                return Err("invalid artifact offset: inside a UTF-8 character".into());
            }
        };
        Ok(ArtifactChunk {
            id: meta.id,
            bytes,
            encoding: meta.encoding,
            offset,
            next_offset,
            total_bytes: size,
            sha256: meta.sha256,
        })
    }

    /// Replace an artifact's bytes (same id, name, kind, and plan status).
    pub fn update_artifact(&self, id: &str, bytes: Vec<u8>) -> Result<ArtifactMeta, StoreError> {
        model::check_size(bytes.len())?;
        let cipher = self.artifact_cipher()?;
        let sha256 = digest(&bytes);
        let (content, blob) = self.seal_content(&cipher, id, &sha256, &bytes)?;
        let replaced = (|| {
            let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
            let tx = db.transaction().map_err(sql)?;
            let row = row(&tx, id)?;
            let mut meta = open_meta(&cipher, id, &row.session_id, &row.meta)?;
            meta.size = bytes.len() as u64;
            meta.sha256 = sha256.clone();
            meta.encoding = model::encoding_of(&bytes);
            meta.updated_at = now();
            tx.execute(
                "UPDATE artifacts SET meta=?,content=?,blob=? WHERE id=?",
                params![seal_meta(&cipher, &meta)?, content, blob, id],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
            Ok::<_, StoreError>((meta, row.blob))
        })();
        match replaced {
            Ok((meta, previous)) => {
                if let Some(previous) = previous.filter(|p| Some(p) != blob.as_ref()) {
                    self.blobs.remove(&previous);
                }
                Ok(meta)
            }
            Err(error) => {
                if let Some(blob) = &blob {
                    self.blobs.remove(blob);
                }
                Err(error)
            }
        }
    }

    /// Delete one artifact and its blob; `false` when no such artifact exists.
    pub fn delete_artifact(&self, id: &str) -> Result<bool, StoreError> {
        self.key()?;
        let blob = {
            let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
            let tx = db.transaction().map_err(sql)?;
            let Some(blob) = tx
                .query_row("SELECT blob FROM artifacts WHERE id=?", [id], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .optional()
                .map_err(sql)?
            else {
                return Ok(false);
            };
            tx.execute("DELETE FROM artifacts WHERE id=?", [id])
                .map_err(sql)?;
            tx.commit().map_err(sql)?;
            blob
        };
        if let Some(blob) = blob {
            self.blobs.remove(&blob);
        }
        Ok(true)
    }

    /// Set a plan artifact's status. Making a plan `active` supersedes the
    /// session's other active plan; other artifacts are refused.
    pub fn set_plan_status(
        &self,
        id: &str,
        status: PlanStatus,
    ) -> Result<ArtifactMeta, StoreError> {
        let cipher = self.artifact_cipher()?;
        let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
        let tx = db.transaction().map_err(sql)?;
        let row = row(&tx, id)?;
        let mut meta = open_meta(&cipher, id, &row.session_id, &row.meta)?;
        if meta.kind != ArtifactKind::Plan {
            return Err("artifact is not a plan".into());
        }
        if meta.plan_status != Some(status) {
            if status == PlanStatus::Active {
                supersede_active_plans(&tx, &cipher, &meta.session_id, Some(id))?;
            }
            meta.plan_status = Some(status);
            meta.updated_at = now();
            tx.execute(
                "UPDATE artifacts SET meta=? WHERE id=?",
                params![seal_meta(&cipher, &meta)?, id],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        Ok(meta)
    }

    /// Delete a session's artifact rows inside the caller's transaction and
    /// return the blob addresses to remove once it commits.
    pub(crate) fn delete_session_artifacts(
        db: &Connection,
        session_id: &str,
    ) -> Result<Vec<String>, StoreError> {
        let mut stmt = db
            .prepare("SELECT blob FROM artifacts WHERE session_id=? AND blob IS NOT NULL")
            .map_err(sql)?;
        let blobs = stmt
            .query_map([session_id], |r| r.get::<_, String>(0))
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        db.execute("DELETE FROM artifacts WHERE session_id=?", [session_id])
            .map_err(sql)?;
        Ok(blobs)
    }

    fn artifact_cipher(&self) -> Result<XChaCha20Poly1305, StoreError> {
        let key = self.key()?;
        sealed::cipher(key.expose())
    }

    /// Seal content as inline column bytes (≤ 1 MiB) or as a blob file.
    fn seal_content(
        &self,
        cipher: &XChaCha20Poly1305,
        id: &str,
        sha256: &str,
        bytes: &[u8],
    ) -> Result<(Option<Vec<u8>>, Option<String>), StoreError> {
        let sealed = DOMAIN.seal_content(cipher, id, sha256, bytes)?;
        if bytes.len() <= INLINE_ARTIFACT_BYTES {
            Ok((Some(sealed), None))
        } else {
            Ok((None, Some(self.blobs.write(&sealed)?)))
        }
    }

    /// Decrypt plaintext bytes `[start, end)`, touching only the segments they span.
    fn plaintext(
        &self,
        db: &Connection,
        cipher: &XChaCha20Poly1305,
        meta: &ArtifactMeta,
        blob: Option<&str>,
        start: u64,
        end: u64,
    ) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        if start >= end {
            return Ok(Zeroizing::new(vec![]));
        }
        let content = Content {
            domain: DOMAIN,
            id: &meta.id,
            sha256: &meta.sha256,
            size: meta.size,
        };
        match blob {
            Some(address) => content.window(
                cipher,
                &Stored::Blob {
                    dir: &self.blobs,
                    address,
                },
                start,
                end,
            ),
            None => {
                let inline: Option<Vec<u8>> = db
                    .query_row(
                        "SELECT content FROM artifacts WHERE id=?",
                        [&meta.id],
                        |r| r.get(0),
                    )
                    .map_err(sql)?;
                let inline = inline.unwrap_or_default();
                content.window(cipher, &Stored::Inline(&inline), start, end)
            }
        }
    }
}

/// An artifact row whose session still exists.
fn row(db: &Connection, id: &str) -> Result<Row, StoreError> {
    db.query_row(
        "SELECT a.session_id,a.meta,a.blob FROM artifacts a JOIN sessions s ON s.id=a.session_id WHERE a.id=?",
        [id],
        |r| {
            Ok(Row {
                session_id: r.get(0)?,
                meta: r.get(1)?,
                blob: r.get(2)?,
            })
        },
    )
    .optional()
    .map_err(sql)?
    .ok_or_else(|| "artifact not found".into())
}

/// The session must exist and the run, when named, must belong to it.
fn check_owner(db: &Connection, session_id: &str, run_id: Option<&str>) -> Result<(), StoreError> {
    let exists: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?)",
            [session_id],
            |r| r.get(0),
        )
        .map_err(sql)?;
    if !exists {
        return Err("session not found".into());
    }
    if let Some(run_id) = run_id {
        let owner: Option<String> = db
            .query_row("SELECT session_id FROM runs WHERE id=?", [run_id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(sql)?;
        if owner.as_deref() != Some(session_id) {
            return Err("run not found in this session".into());
        }
    }
    Ok(())
}

/// Mark every active plan of the session except `keep` as superseded.
fn supersede_active_plans(
    db: &Connection,
    cipher: &XChaCha20Poly1305,
    session_id: &str,
    keep: Option<&str>,
) -> Result<(), StoreError> {
    let rows = {
        let mut stmt = db
            .prepare("SELECT id,meta FROM artifacts WHERE session_id=?")
            .map_err(sql)?;
        stmt.query_map([session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .map_err(sql)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql)?
    };
    for (id, sealed) in rows {
        if keep == Some(id.as_str()) {
            continue;
        }
        let mut meta = open_meta(cipher, &id, session_id, &sealed)?;
        if meta.plan_status == Some(PlanStatus::Active) {
            meta.plan_status = Some(PlanStatus::Superseded);
            meta.updated_at = now();
            db.execute(
                "UPDATE artifacts SET meta=? WHERE id=?",
                params![seal_meta(cipher, &meta)?, id],
            )
            .map_err(sql)?;
        }
    }
    Ok(())
}

fn seal_meta(cipher: &XChaCha20Poly1305, meta: &ArtifactMeta) -> Result<Vec<u8>, StoreError> {
    let plain = Zeroizing::new(serde_json::to_vec(meta).map_err(|e| e.to_string())?);
    DOMAIN.seal_record(cipher, &meta.id, &meta.session_id, &plain)
}

fn open_meta(
    cipher: &XChaCha20Poly1305,
    id: &str,
    session_id: &str,
    sealed: &[u8],
) -> Result<ArtifactMeta, StoreError> {
    let plain = DOMAIN.open_record(cipher, id, session_id, sealed)?;
    let meta: ArtifactMeta =
        serde_json::from_slice(&plain).map_err(|_| "invalid encrypted artifact metadata")?;
    if meta.id != id || meta.session_id != session_id {
        return Err("artifact identity mismatch".into());
    }
    Ok(meta)
}
