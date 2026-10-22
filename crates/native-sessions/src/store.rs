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
/// Vault secret holding the native record encryption key. AiDEX creates and
/// uses it; clients must not replace or delete it.
pub const NATIVE_RECORD_KEY_SECRET: &str = "native-record-encryption-key";
pub struct SessionStore {
    pub(crate) db: Mutex<Connection>,
    vault: Arc<VaultService>,
    /// Artifact blobs (`<db stem>-artifacts/`).
    pub(crate) blobs: crate::sealed::BlobDir,
    /// Attachment blobs and upload files (`<db stem>-attachments/`).
    pub(crate) attachment_blobs: crate::sealed::BlobDir,
    pub(crate) uploads: crate::attachments::Uploads,
    pub(crate) staged_expiry: crate::attachments::StagedExpiry,
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
        db.execute_batch(crate::artifacts::SCHEMA)
            .map_err(|e| e.to_string())?;
        // A daemon that predates artifacts deletes sessions without their
        // artifact rows; drop them so the sweep below reclaims their blobs.
        db.execute(crate::artifacts::PURGE_ORPHANS, [])
            .map_err(|e| e.to_string())?;
        let blobs = crate::sealed::BlobDir::beside(path, crate::sealed::Domain::Artifact);
        blobs.sweep(|address| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM artifacts WHERE blob=?)",
                [address],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())
        });
        // Attachments keep their own table, directory and sweep (which also
        // drops staged attachments older than a day, as later upload and list
        // calls do on a running daemon); none of it needs the key.
        let attachment_blobs =
            crate::sealed::BlobDir::beside(path, crate::sealed::Domain::Attachment);
        crate::attachments::open_maintenance(&db, &attachment_blobs)?;
        let store = Self {
            db: Mutex::new(db),
            vault,
            blobs,
            attachment_blobs,
            uploads: crate::attachments::Uploads::new(),
            staged_expiry: crate::attachments::StagedExpiry::new(),
        };
        Ok(store)
    }
    pub(crate) fn key(&self) -> Result<SecretBytes, StoreError> {
        let name = SecretName::new(NATIVE_RECORD_KEY_SECRET).map_err(|e| e.to_string())?;
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
    pub(crate) fn load(&self, db: &Connection, id: &str) -> Result<SessionDetail, StoreError> {
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
        for graph in &mut session.task_graphs {
            let run_interrupted = session.runs.iter().any(|run| {
                run.id == graph.root_run_id && run.status == RunStatus::Interrupted
            });
            if run_interrupted {
                if matches!(graph.status, TaskGraphStatus::Pending | TaskGraphStatus::Running) {
                    graph.status = TaskGraphStatus::Interrupted;
                }
                for node in &mut graph.nodes {
                    if matches!(node.status, TaskNodeStatus::Pending | TaskNodeStatus::Running) {
                        node.status = TaskNodeStatus::Interrupted;
                    }
                }
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
    pub(crate) fn save(&self, db: &Connection, s: &SessionDetail) -> Result<(), StoreError> {
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
            forked_from: None,
            task_graphs: vec![],
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
        self.begin_with(id, content, command, parent, &RunOptions::default())
    }
    /// [`begin`](Self::begin) with per-request options (invoked skills, web
    /// tools, attachments). Options are part of the command fingerprint.
    ///
    /// A repeated command id with the same arguments returns the original run
    /// before anything else is checked. A new root run is refused while
    /// another root run of the session is active, and a new child unless its
    /// parent is an active run of the session (both checked in the
    /// transaction that records the run). For a new command, attachment ids must
    /// be distinct staged attachments of the session (`attachment_unavailable`
    /// otherwise; also for a child run); in the same transaction as the new
    /// message they become `sent` with the run, command and message index.
    /// Content may be blank when attachments are present: the message then
    /// stores [`ATTACHMENT_ONLY_CONTENT`] so no blank user message is ever
    /// persisted.
    pub fn begin_with(
        &self,
        id: &str,
        content: &str,
        command: &str,
        parent: Option<String>,
        options: &RunOptions,
    ) -> Result<Run, StoreError> {
        if (content.trim().is_empty() && options.attachments.is_empty())
            || content.len() > 256 * 1024
            || command.is_empty()
            || command.len() > 128
        {
            return Err("invalid message or command id".into());
        }
        self.update_in(id, |db, s| {
            // Options join the fingerprint only when present, so command ids
            // recorded before they existed keep their fingerprints; attachment
            // ids join only when present, so skill and web fingerprints stay.
            let mut fp = if options.is_empty() {
                digest(format!("{content}\0{parent:?}").as_bytes())
            } else if options.attachments.is_empty() {
                digest(
                    format!(
                        "{content}\0{parent:?}\0{:?}\0{}",
                        options.skills, options.web_search
                    )
                    .as_bytes(),
                )
            } else {
                digest(
                    format!(
                        "{content}\0{parent:?}\0{:?}\0{}\0{:?}",
                        options.skills, options.web_search, options.attachments
                    )
                    .as_bytes(),
                )
            };
            if let Some(task) = &options.task {
                fp = digest(format!("{fp}\0{}\0{}", task.graph_id, task.node_id).as_bytes());
            }
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
            if parent.is_some() && !options.attachments.is_empty() {
                return Err(AttachmentReason::AttachmentUnavailable
                    .error("attachments can only be sent with a conversation message"));
            }
            // Checked where the child is recorded: a parent cancelled or
            // settled since its caller looked never gains a child.
            if let Some(parent) = &parent
                && !s.runs.iter().any(|r| r.id == *parent && r.status.active())
            {
                return Err("child requires an active parent".into());
            }
            let run_id = crate::store::id("run")?;
            if let Some(task) = &options.task {
                let parent_id = parent.as_deref().ok_or("task run requires a parent run")?;
                let graph = s.task_graphs.iter_mut().find(|graph| graph.id == task.graph_id)
                    .ok_or("task graph not found")?;
                if graph.root_run_id != parent_id {
                    return Err("task graph root does not match parent run".into());
                }
                if graph.status != TaskGraphStatus::Running {
                    return Err("task graph is not active".into());
                }
                let node = graph.nodes.iter_mut().find(|node| node.id == task.node_id)
                    .ok_or("task node not found")?;
                if node.status != TaskNodeStatus::Pending || node.run_id.is_some() {
                    return Err("task node is not pending or already has a run".into());
                }
                node.run_id = Some(run_id.clone());
                node.status = TaskNodeStatus::Running;
            }
            let content = if content.trim().is_empty() {
                ATTACHMENT_ONLY_CONTENT
            } else {
                content
            };
            let message = aidex_providers::ChatMessage::user(content);
            let mut transcript = if parent.is_some() {
                vec![]
            } else {
                s.messages.clone()
            };
            transcript.push(message.clone());
            let run = Run {
                id: run_id,
                session_id: id.into(),
                parent_run_id: parent.clone(),
                status: RunStatus::Pending,
                steps: 0,
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
                skills: options.skills.clone(),
                web_search: options.web_search,
                message_index: parent.is_none().then_some(s.messages.len()),
                attachments: options.attachments.clone(),
                mode: Some(s.configuration.mode),
                permission_mode: Some(s.configuration.permission_mode),
                workspace_id: s.configuration.workspace_id.clone(),
                inference_retries: 0,
                context_trimmed_messages: 0,
                tool_outputs: Vec::new(),
                task_graph_id: options.task.as_ref().map(|task| task.graph_id.clone()),
                task_node_id: options.task.as_ref().map(|task| task.node_id.clone()),
            };
            if !run.attachments.is_empty() {
                self.link_attachments(db, s, &run.attachments, command, &run.id, s.messages.len())?;
            }
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
        self.update_in(id, |_, s| mutate(s))
    }
    /// [`update`](Self::update) whose mutation also writes other tables in
    /// the same transaction.
    pub(crate) fn update_in<T>(
        &self,
        id: &str,
        mutate: impl FnOnce(&Connection, &mut SessionDetail) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let mut s = self.load(&tx, id)?;
        let result = mutate(&tx, &mut s)?;
        self.save(&tx, &s)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub fn run(&self, id: &str) -> Result<Run, StoreError> {
        let session = self.session_of(id)?;
        self.get(&session)?
            .runs
            .into_iter()
            .find(|r| r.id == id)
            .ok_or("run not found".into())
    }
    /// Use one step of the run's budget for an inference retry and count it in
    /// [`Run::inference_retries`], in one transaction. Refused, with nothing
    /// written, once the run has used `max_steps` steps or is no longer active
    /// (a cancel stops further retries). Returns the run's retry count.
    pub fn reserve_inference_retry(&self, run_id: &str) -> Result<u32, StoreError> {
        let session = self.session_of(run_id)?;
        self.update(&session, |s| {
            let maximum = s.configuration.max_steps;
            let run_index = s.runs.iter().position(|run| run.id == run_id).ok_or("run not found")?;
            let (task_graph_id, task_node_id, steps) = {
                let run = &s.runs[run_index];
                if !run.status.active() {
                    return Err("run is no longer active".into());
                }
                if run.steps >= maximum {
                    return Err("native run step budget exhausted before inference retry".into());
                }
                (run.task_graph_id.clone(), run.task_node_id.clone(), run.steps)
            };
            let task = match (task_graph_id, task_node_id) {
                (None, None) => None,
                (Some(graph_id), Some(node_id)) => Some((graph_id, node_id)),
                _ => return Err("incomplete task run identity".into()),
            };
            let task_indexes = if let Some((graph_id, node_id)) = task {
                let graph_index = s.task_graphs.iter().position(|graph| graph.id == graph_id)
                    .ok_or("task graph not found")?;
                let graph = &s.task_graphs[graph_index];
                let node_index = graph.nodes.iter().position(|node| node.id == node_id)
                    .ok_or("task node not found")?;
                if graph.steps_used >= graph.max_steps {
                    return Err("task graph step budget exhausted before inference retry".into());
                }
                if graph.nodes[node_index].steps_used >= graph.nodes[node_index].step_budget {
                    return Err("task node step budget exhausted before inference retry".into());
                }
                Some((graph_index, node_index))
            } else {
                None
            };
            let run = &mut s.runs[run_index];
            run.steps += 1;
            run.inference_retries = run.inference_retries.saturating_add(1);
            if let Some((graph_index, node_index)) = task_indexes {
                let graph = &mut s.task_graphs[graph_index];
                graph.steps_used += 1;
                graph.nodes[node_index].steps_used += 1;
            }
            Ok(run.inference_retries)
        })
    }
    /// Record that a request of the run left `trimmed` older messages out of its
    /// replay view. [`Run::context_trimmed_messages`] keeps the largest count of
    /// any request; zero writes nothing.
    pub fn record_context_trim(&self, run_id: &str, trimmed: u32) -> Result<(), StoreError> {
        if trimmed == 0 {
            return Ok(());
        }
        let session = self.session_of(run_id)?;
        self.update(&session, |s| {
            let run = s
                .runs
                .iter_mut()
                .find(|r| r.id == run_id)
                .ok_or("run not found")?;
            run.context_trimmed_messages = run.context_trimmed_messages.max(trimmed);
            Ok(())
        })
    }
    /// The session a run belongs to, from the plaintext run metadata.
    fn session_of(&self, run_id: &str) -> Result<String, StoreError> {
        let db = self.db.lock().map_err(|_| "session store poisoned")?;
        db.query_row("SELECT session_id FROM runs WHERE id=?", [run_id], |r| {
            r.get(0)
        })
        .map_err(|e| e.to_string())
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
        let blobs = Self::delete_session_artifacts(&tx, id)?;
        let attachment_blobs = Self::delete_session_attachments(&tx, id)?;
        let n = tx
            .execute("DELETE FROM sessions WHERE id=?", [id])
            .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        drop(db);
        for blob in blobs {
            self.blobs.remove(&blob);
        }
        for blob in attachment_blobs {
            self.attachment_blobs.remove(&blob);
        }
        self.uploads.abort_session(id);
        Ok(n > 0)
    }
}
fn status_of(s: RunStatus) -> String {
    serde_json::to_value(s).unwrap().as_str().unwrap().into()
}
fn status(s: RunStatus) -> String {
    status_of(s)
}
