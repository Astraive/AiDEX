use crate::*;
use aidex_core::EntityId;
use aidex_vault::{
    ConsumerScope, SecretBytes, SecretKind, SecretName, SecretPolicy, VaultErrorKind, VaultService,
    VaultState,
};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;
pub type StoreError = String;
pub struct SessionStore {
    pub(crate) db: Mutex<Connection>,
    vault: Arc<VaultService>,
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn id(prefix: &str) -> Result<String, StoreError> {
    EntityId::generate(prefix)
        .map(|i| i.to_string())
        .map_err(|e| e.to_string())
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
impl SessionStore {
    pub fn open(path: impl AsRef<Path>, vault: Arc<VaultService>) -> Result<Self, StoreError> {
        let path = path.as_ref();
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("session database cannot be a symbolic link".into());
        }
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,status TEXT NOT NULL,payload BLOB NOT NULL); CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY, session_id TEXT NOT NULL,status TEXT NOT NULL); CREATE TABLE IF NOT EXISTS memory(id TEXT PRIMARY KEY,workspace TEXT NOT NULL,payload BLOB NOT NULL); UPDATE runs SET status='interrupted' WHERE status IN ('pending','running','waiting_approval'); UPDATE sessions SET status='interrupted' WHERE status IN ('pending','running','waiting_approval');").map_err(|e|e.to_string())?;
        Ok(Self {
            db: Mutex::new(db),
            vault,
        })
    }
    pub(crate) fn key(&self) -> Result<SecretBytes, StoreError> {
        let name = SecretName::new("native-record-encryption-key").map_err(|e| e.to_string())?;
        let consumer = ConsumerScope::process("native-record-store").map_err(|e| e.to_string())?;
        match self.vault.secret_meta(&name) {
            Ok(_) => {}
            Err(e) if e.kind() == VaultErrorKind::SecretNotFound => {
                let mut key = Zeroizing::new(vec![0u8; 32]);
                getrandom::fill(&mut key).map_err(|e| e.to_string())?;
                self.vault
                    .put_secret(
                        &name,
                        SecretKind::Generic,
                        SecretBytes::new(key.to_vec()),
                        SecretPolicy::new([consumer.clone()]).map_err(|e| e.to_string())?,
                    )
                    .map_err(|e| e.to_string())?;
            }
            Err(e) => return Err(e.to_string()),
        }
        let handle = self
            .vault
            .issue_handle(&name, &consumer, Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        let key = self
            .vault
            .resolve(&handle, &consumer)
            .map_err(|e| e.to_string());
        self.vault.revoke_handle(&handle.handle_id);
        let key = key?;
        if key.len() != 32 {
            return Err("invalid native record key".into());
        }
        Ok(key)
    }
    fn seal(&self, s: &SessionDetail) -> Result<Vec<u8>, StoreError> {
        let key = self.key()?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(key.expose()).map_err(|_| "invalid record key")?;
        let plain = Zeroizing::new(serde_json::to_vec(s).map_err(|e| e.to_string())?);
        let mut nonce = [0u8; 24];
        getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
        let aad = format!("aidex-native-v1:session:{}", s.id);
        let encrypted = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| "record encryption failed")?;
        let mut out = nonce.to_vec();
        out.extend(encrypted);
        Ok(out)
    }
    fn load(&self, db: &Connection, id: &str) -> Result<SessionDetail, StoreError> {
        let bytes: Vec<u8> = db
            .query_row("SELECT payload FROM sessions WHERE id=?", [id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if bytes.len() < 40 {
            return Err("truncated encrypted native record".into());
        }
        let key = self.key()?;
        let cipher =
            XChaCha20Poly1305::new_from_slice(key.expose()).map_err(|_| "invalid record key")?;
        let aad = format!("aidex-native-v1:session:{id}");
        let plain = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&bytes[..24]),
                    Payload {
                        msg: &bytes[24..],
                        aad: aad.as_bytes(),
                    },
                )
                .map_err(|_| "native record authentication failed")?,
        );
        let mut session: SessionDetail =
            serde_json::from_slice(&plain).map_err(|_| "invalid encrypted native record")?;
        if session.id != id {
            return Err("record identity mismatch".into());
        }
        for run in &mut session.runs {
            let status: String = db
                .query_row("SELECT status FROM runs WHERE id=?", [&run.id], |r| {
                    r.get(0)
                })
                .map_err(|e| e.to_string())?;
            if status == "interrupted" {
                run.status = RunStatus::Interrupted;
            }
        }
        for approval in &mut session.approvals {
            if approval.status == "pending"
                && session
                    .runs
                    .iter()
                    .any(|r| r.id == approval.run_id && r.status == RunStatus::Interrupted)
            {
                approval.status = "expired".into();
            }
        }
        Ok(session)
    }
    fn save(&self, db: &Connection, s: &SessionDetail) -> Result<(), StoreError> {
        let bytes = self.seal(s)?;
        let status = s
            .runs
            .iter()
            .rev()
            .find(|r| r.parent_run_id.is_none())
            .map_or("idle".into(), |r| status(r.status));
        db.execute("INSERT INTO sessions(id,status,payload) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET status=excluded.status,payload=excluded.payload",params![s.id,status,bytes]).map_err(|e|e.to_string())?;
        for r in &s.runs {
            db.execute("INSERT INTO runs(id,session_id,status) VALUES(?,?,?) ON CONFLICT(id) DO UPDATE SET status=excluded.status",params![r.id,s.id,status_of(r.status)]).map_err(|e|e.to_string())?;
        }
        Ok(())
    }
    pub fn create(&self, configuration: SessionConfig) -> Result<SessionDetail, StoreError> {
        configuration.validate()?;
        let s = SessionDetail {
            id: id("session")?,
            configuration,
            pinned: false,
            created_at: now(),
            messages: vec![],
            runs: vec![],
            approvals: vec![],
            checkpoints: vec![],
            commands: Default::default(),
            runtime_ref: None,
        };
        let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        self.save(&tx, &s)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(s)
    }
    pub fn get(&self, id: &str) -> Result<SessionDetail, StoreError> {
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        self.load(&db, id)
    }
    pub fn list(&self) -> Result<Vec<SessionSummary>, StoreError> {
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        let mut stmt = db
            .prepare("SELECT id,status FROM sessions ORDER BY id DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let locked = self.vault.status().state != VaultState::Unlocked;
        let mut result = vec![];
        for row in rows {
            let (id, status) = row.map_err(|e| e.to_string())?;
            let s = if locked {
                None
            } else {
                Some(self.load(&db, &id)?)
            };
            result.push(SessionSummary {
                id,
                status,
                locked,
                title: s.as_ref().map(|s| s.configuration.title.clone()),
                pinned: s.as_ref().map(|s| s.pinned),
                workspace_id: s
                    .as_ref()
                    .and_then(|s| s.configuration.workspace_id.clone()),
                mode: s.as_ref().map(|s| s.configuration.mode),
                provider_id: s.as_ref().map(|s| s.configuration.provider_id.clone()),
                model_id: s.as_ref().map(|s| s.configuration.model_id.clone()),
                created_at: s.as_ref().map(|s| s.created_at),
            });
        }
        Ok(result)
    }
    pub fn begin(
        &self,
        id: &str,
        content: &str,
        command: &str,
        parent: Option<String>,
    ) -> Result<Run, StoreError> {
        if content.trim().is_empty()
            || content.len() > 256 * 1024
            || command.is_empty()
            || command.len() > 128
        {
            return Err("invalid message or command id".into());
        }
        self.update(id, |s| {
            let fp = digest(format!("{content}\0{parent:?}").as_bytes());
            if let Some(existing) = s.commands.get(command) {
                if existing.fingerprint != fp {
                    return Err("command id reused with different arguments".into());
                }
                return s
                    .runs
                    .iter()
                    .find(|r| r.id == existing.run_id)
                    .cloned()
                    .ok_or("missing command run".into());
            }
            if parent.is_none()
                && s.runs
                    .iter()
                    .any(|r| r.parent_run_id.is_none() && r.status.active())
            {
                return Err("session already has an active root run".into());
            }
            let message = aidex_providers::ChatMessage::user(content);
            let mut transcript = if parent.is_some() {
                vec![]
            } else {
                s.messages.clone()
            };
            transcript.push(message.clone());
            let run = Run {
                id: crate::store::id("run")?,
                session_id: id.into(),
                parent_run_id: parent.clone(),
                status: RunStatus::Pending,
                steps: 0,
                inference_retries: 0,
                context_trimmed_messages: 0,
                error: None,
                created_at: now(),
                updated_at: now(),
                transcript,
                pending_calls: vec![],
                effective_max_tokens: None,
                estimated_input_tokens: None,
                model_used: None,
                runtime_ref: if parent.is_none() {
                    s.runtime_ref.clone()
                } else {
                    None
                },
            };
            if parent.is_none() {
                s.messages.push(message);
            }
            s.runs.push(run.clone());
            s.commands.insert(
                command.into(),
                CommandRecord {
                    fingerprint: fp,
                    run_id: run.id.clone(),
                },
            );
            Ok(run)
        })
    }
    pub fn update<T>(
        &self,
        id: &str,
        mutate: impl FnOnce(&mut SessionDetail) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let mut s = self.load(&tx, id)?;
        let result = mutate(&mut s)?;
        self.save(&tx, &s)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub fn run(&self, id: &str) -> Result<Run, StoreError> {
        let session: String = {
            let db = self.db.lock().map_err(|_| "session store poisoned")?;
            db.query_row("SELECT session_id FROM runs WHERE id=?", [id], |r| r.get(0))
                .map_err(|e| e.to_string())?
        };
        self.get(&session)?
            .runs
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("run not found".into())
    }
    pub fn run_summaries(&self) -> Result<Vec<RunSummary>, StoreError> {
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        let mut stmt = db
            .prepare("SELECT id,session_id,status FROM runs ORDER BY id DESC")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = vec![];
        for row in rows {
            let (id, session_id, status) = row.map_err(|e| e.to_string())?;
            let status = serde_json::from_value(serde_json::Value::String(status))
                .map_err(|_| "invalid persisted run status")?;
            out.push(RunSummary {
                id,
                session_id,
                status,
            });
        }
        Ok(out)
    }
    pub fn delete(&self, id: &str) -> Result<bool, StoreError> {
        let s = self.get(id)?;
        if s.runs.iter().any(|r| r.status.active()) {
            return Err("cancel active runs before deleting session".into());
        }
        let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM runs WHERE session_id=?", [id])
            .map_err(|e| e.to_string())?;
        let n = tx
            .execute("DELETE FROM sessions WHERE id=?", [id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(n > 0)
    }
}
fn status_of(s: RunStatus) -> String {
    serde_json::to_value(s).unwrap().as_str().unwrap().into()
}
fn status(s: RunStatus) -> String {
    status_of(s)
}
