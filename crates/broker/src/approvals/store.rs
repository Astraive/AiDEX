//! Durable approval records in SQLite (`DrivePaths::sessions()/approvals.sqlite3`).
//!
//! Every transition is a compare-and-set `UPDATE … WHERE status = '<expected>'`
//! inside an immediate transaction, so racing decisions (approve vs. cancel vs.
//! expire) have exactly one winner and the losers observe the winner's state. The
//! store emits no events; [`crate::Broker`] wraps each transition with its
//! `approval.*` event. Raw tool input is never stored here: only the card (already
//! redacted), the binding fingerprint, and the session-grant key.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use aidex_core::{EntityId, Timestamp};
use aidex_policy::ApprovalScope;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use crate::{
    BrokerError, Principal,
    approvals::{
        ApprovalCard, ApprovalFilter, ApprovalRecord, ApprovalState, ApprovalStatus, GrantInfo,
    },
};

/// File name of the approval database inside `DrivePaths::sessions()`.
pub const APPROVALS_FILE: &str = "approvals.sqlite3";
/// Schema version written by this build.
const SCHEMA_VERSION: i64 = 1;
/// Most records one `list` returns.
const MAX_LIST: usize = 500;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS approvals (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    session_id TEXT NOT NULL,
    workspace_id TEXT NOT NULL,
    call_id TEXT NOT NULL,
    tool_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    grant_key TEXT,
    card TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending','approved','denied','expired','revoked','cancelled')),
    scope TEXT CHECK (scope IS NULL OR scope IN ('once','session')),
    actor TEXT,
    resolved_ms INTEGER,
    reason TEXT,
    consumed_ms INTEGER,
    grant_expires_ms INTEGER
);
CREATE INDEX IF NOT EXISTS approvals_status ON approvals(status);
CREATE INDEX IF NOT EXISTS approvals_session ON approvals(session_id);
CREATE INDEX IF NOT EXISTS approvals_grant ON approvals(grant_key) WHERE grant_key IS NOT NULL;
";

const COLUMNS: &str = "id, session_id, workspace_id, call_id, tool_id, fingerprint, card, \
    status, scope, actor, resolved_ms, reason, consumed_ms, grant_expires_ms";

/// A new pending approval.
pub(crate) struct NewApproval {
    pub card: ApprovalCard,
    pub call_id: EntityId,
    pub fingerprint: String,
    pub grant_key: Option<String>,
}

/// The approval database. `Send + Sync`; one connection guarded by a mutex.
pub struct ApprovalStore {
    path: PathBuf,
    connection: Mutex<Connection>,
}

impl std::fmt::Debug for ApprovalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl ApprovalStore {
    /// Open (or create) the database at `path`; its directory must exist. Symlinked
    /// database files (and SQLite sidecars) are refused, and a database written by a
    /// newer build is rejected untouched.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, BrokerError> {
        let path = path.into();
        for suffix in ["", "-wal", "-shm", "-journal"] {
            refuse_link(Path::new(&format!("{}{suffix}", path.display())))?;
        }
        let connection = Connection::open(&path)?;
        connection.busy_timeout(Duration::from_secs(10))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        migrate(&connection)?;
        Ok(Self {
            path,
            connection: Mutex::new(connection),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // Transactions keep the database consistent even if a holder panicked.
        self.connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// One approval, with grant activity evaluated at `now`.
    pub fn get(
        &self,
        id: &EntityId,
        now: &Timestamp,
    ) -> Result<Option<ApprovalRecord>, BrokerError> {
        let connection = self.lock();
        select_one(&connection, id, now)
    }

    /// Approvals matching `filter`, newest first.
    pub fn list(
        &self,
        filter: &ApprovalFilter,
        now: &Timestamp,
    ) -> Result<Vec<ApprovalRecord>, BrokerError> {
        let connection = self.lock();
        let limit =
            i64::try_from(filter.limit.unwrap_or(MAX_LIST).clamp(1, MAX_LIST)).unwrap_or(500);
        let mut statement = connection.prepare(&format!(
            "SELECT {COLUMNS} FROM approvals
             WHERE (?1 IS NULL OR session_id = ?1)
               AND (?2 IS NULL OR workspace_id = ?2)
               AND (?3 IS NULL OR status = ?3)
             ORDER BY seq DESC LIMIT ?4"
        ))?;
        let rows = statement.query_map(
            params![
                filter.session_id.as_ref().map(EntityId::as_str),
                filter.workspace_id.as_ref().map(EntityId::as_str),
                filter.status.map(ApprovalStatus::as_str),
                limit
            ],
            |row| Ok(parse_row(row, now)),
        )?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row??);
        }
        Ok(records)
    }

    pub(crate) fn insert(
        &self,
        new: &NewApproval,
        now: &Timestamp,
    ) -> Result<ApprovalRecord, BrokerError> {
        let card = serde_json::to_string(&new.card)
            .map_err(|error| BrokerError::Store(format!("cannot encode the card: {error}")))?;
        let connection = self.lock();
        connection.execute(
            "INSERT INTO approvals (id, session_id, workspace_id, call_id, tool_id, fingerprint,
                 grant_key, card, created_ms, expires_ms, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'pending')",
            params![
                new.card.approval_id.as_str(),
                new.card.session_id.as_str(),
                new.card.workspace_id.as_str(),
                new.call_id.as_str(),
                new.card.tool_id,
                new.fingerprint,
                new.grant_key,
                card,
                new.card.created_at.unix_millis(),
                new.card.expires_at.unix_millis(),
            ],
        )?;
        select_one(&connection, &new.card.approval_id, now)?
            .ok_or_else(|| BrokerError::Store("the inserted approval is missing".into()))
    }

    /// `pending` → `approved` while not yet expired. `None` when another transition
    /// won (or the approval does not exist).
    pub(crate) fn approve(
        &self,
        id: &EntityId,
        scope: ApprovalScope,
        by: &Principal,
        now: &Timestamp,
        grant_expires: Option<&Timestamp>,
    ) -> Result<Option<ApprovalRecord>, BrokerError> {
        let actor = encode_principal(by)?;
        self.transition(id, now, |tx| {
            Ok(tx.execute(
                "UPDATE approvals SET status = 'approved', scope = ?2, actor = ?3,
                     resolved_ms = ?4, grant_expires_ms = ?5
                 WHERE id = ?1 AND status = 'pending' AND expires_ms > ?4",
                params![
                    id.as_str(),
                    scope.as_str(),
                    actor,
                    now.unix_millis(),
                    grant_expires.map(Timestamp::unix_millis)
                ],
            )? == 1)
        })
    }

    /// `pending` → `denied`.
    pub(crate) fn deny(
        &self,
        id: &EntityId,
        by: &Principal,
        now: &Timestamp,
    ) -> Result<Option<ApprovalRecord>, BrokerError> {
        let actor = encode_principal(by)?;
        self.transition(id, now, |tx| {
            Ok(tx.execute(
                "UPDATE approvals SET status = 'denied', actor = ?2, resolved_ms = ?3
                 WHERE id = ?1 AND status = 'pending'",
                params![id.as_str(), actor, now.unix_millis()],
            )? == 1)
        })
    }

    /// `pending` → `cancelled`.
    pub(crate) fn cancel(
        &self,
        id: &EntityId,
        now: &Timestamp,
        reason: &str,
    ) -> Result<Option<ApprovalRecord>, BrokerError> {
        self.transition(id, now, |tx| {
            Ok(tx.execute(
                "UPDATE approvals SET status = 'cancelled', resolved_ms = ?2, reason = ?3
                 WHERE id = ?1 AND status = 'pending'",
                params![id.as_str(), now.unix_millis(), reason],
            )? == 1)
        })
    }

    /// `pending` → `revoked`, or `approved` → `revoked` while the approval can still
    /// authorize something (an unconsumed approval or a session grant).
    pub(crate) fn revoke(
        &self,
        id: &EntityId,
        by: &Principal,
        now: &Timestamp,
        reason: Option<&str>,
    ) -> Result<Option<ApprovalRecord>, BrokerError> {
        let actor = encode_principal(by)?;
        self.transition(id, now, |tx| {
            Ok(tx.execute(
                "UPDATE approvals SET status = 'revoked', actor = ?2, resolved_ms = ?3, reason = ?4
                 WHERE id = ?1 AND (status = 'pending'
                     OR (status = 'approved' AND (scope = 'session' OR consumed_ms IS NULL)))",
                params![id.as_str(), actor, now.unix_millis(), reason],
            )? == 1)
        })
    }

    /// Mark an approved call as executed. `false` when it was already consumed or is
    /// no longer approved (revoked meanwhile).
    pub(crate) fn consume(&self, id: &EntityId, now: &Timestamp) -> Result<bool, BrokerError> {
        let connection = self.lock();
        Ok(connection.execute(
            "UPDATE approvals SET consumed_ms = ?2
             WHERE id = ?1 AND status = 'approved' AND consumed_ms IS NULL",
            params![id.as_str(), now.unix_millis()],
        )? == 1)
    }

    /// Expire every pending approval whose deadline has passed.
    pub(crate) fn expire_due(&self, now: &Timestamp) -> Result<Vec<ApprovalRecord>, BrokerError> {
        self.bulk(
            now,
            "status = 'pending' AND expires_ms <= ?1",
            None,
            |tx, id| {
                Ok(tx.execute(
                    "UPDATE approvals SET status = 'expired', resolved_ms = ?2
                     WHERE id = ?1 AND status = 'pending' AND expires_ms <= ?2",
                    params![id, now.unix_millis()],
                )? == 1)
            },
        )
    }

    /// Cancel every pending approval (of one session, or all).
    pub(crate) fn cancel_pending(
        &self,
        session: Option<&EntityId>,
        now: &Timestamp,
        reason: &str,
    ) -> Result<Vec<ApprovalRecord>, BrokerError> {
        self.bulk(now, "status = 'pending'", session, |tx, id| {
            Ok(tx.execute(
                "UPDATE approvals SET status = 'cancelled', resolved_ms = ?2, reason = ?3
                 WHERE id = ?1 AND status = 'pending'",
                params![id, now.unix_millis(), reason],
            )? == 1)
        })
    }

    /// Revoke every approval that could still authorize execution: unconsumed
    /// approvals and session grants (of one session, or all).
    pub(crate) fn revoke_active(
        &self,
        session: Option<&EntityId>,
        by: &Principal,
        now: &Timestamp,
        reason: &str,
    ) -> Result<Vec<ApprovalRecord>, BrokerError> {
        let actor = encode_principal(by)?;
        let condition = "status = 'approved' AND (scope = 'session' OR consumed_ms IS NULL)";
        self.bulk(now, condition, session, |tx, id| {
            Ok(tx.execute(
                &format!(
                    "UPDATE approvals SET status = 'revoked', actor = ?2, resolved_ms = ?3, reason = ?4
                     WHERE id = ?1 AND {condition}"
                ),
                params![id, actor, now.unix_millis(), reason],
            )? == 1)
        })
    }

    /// An active session grant for exactly this session, workspace, tool, and grant
    /// key.
    pub(crate) fn find_grant(
        &self,
        session: &EntityId,
        workspace: &EntityId,
        tool: &str,
        grant_key: &str,
        now: &Timestamp,
    ) -> Result<Option<ApprovalRecord>, BrokerError> {
        let connection = self.lock();
        let id: Option<String> = connection
            .query_row(
                "SELECT id FROM approvals
                 WHERE session_id = ?1 AND workspace_id = ?2 AND tool_id = ?3 AND grant_key = ?4
                   AND status = 'approved' AND scope = 'session' AND grant_expires_ms > ?5
                 ORDER BY seq DESC LIMIT 1",
                params![
                    session.as_str(),
                    workspace.as_str(),
                    tool,
                    grant_key,
                    now.unix_millis()
                ],
                |row| row.get(0),
            )
            .optional()?;
        match id {
            Some(id) => select_one(&connection, &entity(&id)?, now),
            None => Ok(None),
        }
    }

    /// Run one compare-and-set update in an immediate transaction; returns the
    /// updated record when it applied.
    fn transition(
        &self,
        id: &EntityId,
        now: &Timestamp,
        update: impl FnOnce(&Transaction<'_>) -> Result<bool, BrokerError>,
    ) -> Result<Option<ApprovalRecord>, BrokerError> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let applied = update(&tx)?;
        let record = if applied {
            select_one(&tx, id, now)?
        } else {
            None
        };
        tx.commit()?;
        Ok(record)
    }

    /// Select matching ids and apply `update` to each inside one immediate
    /// transaction; returns the records that changed.
    fn bulk(
        &self,
        now: &Timestamp,
        condition: &str,
        session: Option<&EntityId>,
        mut update: impl FnMut(&Transaction<'_>, &str) -> Result<bool, BrokerError>,
    ) -> Result<Vec<ApprovalRecord>, BrokerError> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids: Vec<String> = {
            let mut statement = tx.prepare(&format!(
                "SELECT id FROM approvals WHERE {condition} AND (?2 IS NULL OR session_id = ?2)
                 ORDER BY seq"
            ))?;
            let rows = statement.query_map(
                params![now.unix_millis(), session.map(EntityId::as_str)],
                |row| row.get(0),
            )?;
            rows.collect::<Result<_, _>>()?
        };
        let mut changed = Vec::new();
        for id in ids {
            if update(&tx, &id)?
                && let Some(record) = select_one(&tx, &entity(&id)?, now)?
            {
                changed.push(record);
            }
        }
        tx.commit()?;
        Ok(changed)
    }
}

fn migrate(connection: &Connection) -> Result<(), BrokerError> {
    connection
        .execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);")?;
    let version: Option<i64> = connection
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .optional()?
        .flatten();
    match version {
        None => {
            connection.execute_batch(SCHEMA)?;
            connection.execute(
                "INSERT INTO schema_version (version) VALUES (?1)",
                params![SCHEMA_VERSION],
            )?;
            Ok(())
        }
        Some(SCHEMA_VERSION) => {
            connection.execute_batch(SCHEMA)?;
            Ok(())
        }
        Some(other) => Err(BrokerError::Store(format!(
            "approval database schema {other} is not supported by this build (expected {SCHEMA_VERSION})"
        ))),
    }
}

fn refuse_link(path: &Path) -> Result<(), BrokerError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            BrokerError::Store(format!("{} must be a regular file", path.display())),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(BrokerError::Store(format!("{}: {error}", path.display()))),
    }
}

fn entity(text: &str) -> Result<EntityId, BrokerError> {
    EntityId::new(text).map_err(|error| BrokerError::Store(format!("corrupt id `{text}`: {error}")))
}

fn timestamp(millis: i64) -> Result<Timestamp, BrokerError> {
    Timestamp::from_unix_millis(millis)
        .map_err(|error| BrokerError::Store(format!("corrupt timestamp {millis}: {error}")))
}

fn encode_principal(principal: &Principal) -> Result<String, BrokerError> {
    serde_json::to_string(principal)
        .map_err(|error| BrokerError::Store(format!("cannot encode principal: {error}")))
}

fn select_one(
    connection: &Connection,
    id: &EntityId,
    now: &Timestamp,
) -> Result<Option<ApprovalRecord>, BrokerError> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM approvals WHERE id = ?1"),
            params![id.as_str()],
            |row| Ok(parse_row(row, now)),
        )
        .optional()?
        .transpose()
}

fn parse_row(row: &Row<'_>, now: &Timestamp) -> Result<ApprovalRecord, BrokerError> {
    let text = |index: usize| -> Result<String, BrokerError> { Ok(row.get::<_, String>(index)?) };
    let card: ApprovalCard = serde_json::from_str(&text(6)?)
        .map_err(|error| BrokerError::Store(format!("corrupt approval card: {error}")))?;
    let status: ApprovalStatus = text(7)?.parse()?;
    let scope: Option<String> = row.get(8)?;
    let scope = scope
        .map(|scope| {
            scope
                .parse::<ApprovalScope>()
                .map_err(|error| BrokerError::Store(error.to_string()))
        })
        .transpose()?;
    let actor: Option<String> = row.get(9)?;
    let by = actor
        .map(|actor| {
            serde_json::from_str::<Principal>(&actor)
                .map_err(|error| BrokerError::Store(format!("corrupt principal: {error}")))
        })
        .transpose()?;
    let resolved: Option<i64> = row.get(10)?;
    let at = resolved.map(timestamp).transpose()?;
    let reason: Option<String> = row.get(11)?;
    let consumed: Option<i64> = row.get(12)?;
    let grant_expires: Option<i64> = row.get(13)?;
    let missing = || {
        BrokerError::Store(format!(
            "approval {} has an incomplete state",
            card.approval_id
        ))
    };
    let state = match status {
        ApprovalStatus::Pending => ApprovalState::Pending,
        ApprovalStatus::Approved => ApprovalState::Approved {
            scope: scope.ok_or_else(missing)?,
            by: by.ok_or_else(missing)?,
            at: at.ok_or_else(missing)?,
        },
        ApprovalStatus::Denied => ApprovalState::Denied {
            by: by.ok_or_else(missing)?,
            at: at.ok_or_else(missing)?,
        },
        ApprovalStatus::Expired => ApprovalState::Expired {
            at: at.ok_or_else(missing)?,
        },
        ApprovalStatus::Revoked => ApprovalState::Revoked {
            by: by.ok_or_else(missing)?,
            at: at.ok_or_else(missing)?,
            reason,
        },
        ApprovalStatus::Cancelled => ApprovalState::Cancelled {
            at: at.ok_or_else(missing)?,
            reason,
        },
    };
    let grant = match (scope, grant_expires) {
        (Some(ApprovalScope::Session), Some(expires)) => Some(GrantInfo {
            active: status == ApprovalStatus::Approved && expires > now.unix_millis(),
            expires_at: timestamp(expires)?,
        }),
        _ => None,
    };
    Ok(ApprovalRecord {
        approval_id: entity(&text(0)?)?,
        session_id: entity(&text(1)?)?,
        workspace_id: entity(&text(2)?)?,
        call_id: entity(&text(3)?)?,
        tool_id: text(4)?,
        intent_fingerprint: text(5)?,
        state,
        card,
        consumed_at: consumed.map(timestamp).transpose()?,
        grant,
    })
}
