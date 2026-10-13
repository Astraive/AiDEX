use crate::snapshot::{self, Capture};
use aidex_native_sessions::{RuntimeError, digest};
use aidex_vault::{
    ConsumerScope, SecretBytes, SecretKind, SecretName, SecretPolicy, VaultErrorKind, VaultService,
};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use std::{
    collections::BTreeSet,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use zeroize::Zeroizing;
pub(crate) fn error(code: &str, message: &str) -> RuntimeError {
    RuntimeError {
        code: code.into(),
        message: message.into(),
    }
}
/// The vault record holding the key that seals external runtime state on the
/// drive: configuration, account profiles and native session snapshots. AiDEX
/// manages it; clients must not replace or delete it.
pub const RUNTIME_STATE_KEY_SECRET: &str = "external-runtime-encryption-key";
pub(crate) const MAX_STATE: usize = 8 * 1024 * 1024;
/// Whether an entry is a symlink or, on Windows, any reparse point (junctions
/// included): neither may be followed out of an app-owned directory.
pub(crate) fn unsafe_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_type().is_symlink()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}
pub(crate) struct State {
    pub root: PathBuf,
    pub vault: Arc<VaultService>,
    lock: Mutex<()>,
    account_lock: Mutex<()>,
    pending_logins: Mutex<BTreeSet<String>>,
}
impl State {
    pub fn new(root: PathBuf, vault: Arc<VaultService>) -> Result<Self, RuntimeError> {
        std::fs::create_dir_all(&root).map_err(|_| {
            error(
                "state_unavailable",
                "Runtime state directory cannot be created",
            )
        })?;
        if std::fs::symlink_metadata(&root).is_ok_and(|m| unsafe_link(&m)) {
            return Err(error("invalid_config", "Runtime state cannot be a symlink"));
        }
        Ok(Self {
            root,
            vault,
            lock: Mutex::new(()),
            account_lock: Mutex::new(()),
            pending_logins: Mutex::new(BTreeSet::new()),
        })
    }
    /// Serializes account profile writes with revision checks and rotations.
    pub fn account_guard(&self) -> Result<MutexGuard<'_, ()>, RuntimeError> {
        self.account_lock
            .lock()
            .map_err(|_| error("state_unavailable", "Managed account lock unavailable"))
    }
    fn logins(&self) -> Result<MutexGuard<'_, BTreeSet<String>>, RuntimeError> {
        self.pending_logins
            .lock()
            .map_err(|_| error("state_unavailable", "Managed login state unavailable"))
    }
    pub fn set_login_pending(&self, id: &str, pending: bool) -> Result<(), RuntimeError> {
        let mut logins = self.logins()?;
        if pending {
            logins.insert(id.to_owned());
        } else {
            logins.remove(id);
        }
        Ok(())
    }
    pub fn login_pending(&self, id: &str) -> Result<bool, RuntimeError> {
        Ok(self.logins()?.contains(id))
    }
    pub fn has_pending_logins(&self) -> bool {
        self.logins().is_ok_and(|logins| !logins.is_empty())
    }
    fn key(&self) -> Result<SecretBytes, RuntimeError> {
        let name = SecretName::new(RUNTIME_STATE_KEY_SECRET).unwrap();
        let consumer = ConsumerScope::process("aidex-adapters").unwrap();
        match self.vault.secret_meta(&name) {
            Ok(_) => {}
            Err(e) if e.kind() == VaultErrorKind::SecretNotFound => {
                let mut key = Zeroizing::new(vec![0; 32]);
                getrandom::fill(&mut key)
                    .map_err(|_| error("state_unavailable", "Random source unavailable"))?;
                self.vault
                    .put_secret(
                        &name,
                        SecretKind::Generic,
                        SecretBytes::new(key.to_vec()),
                        SecretPolicy::new([consumer.clone()]).unwrap(),
                    )
                    .map_err(|_| {
                        error("vault_locked", "Runtime encryption requires unlocked vault")
                    })?;
            }
            Err(_) => {
                return Err(error(
                    "vault_locked",
                    "Runtime state requires unlocked vault",
                ));
            }
        }
        let handle = self
            .vault
            .issue_handle(&name, &consumer, Duration::from_secs(5))
            .map_err(|_| error("vault_locked", "Runtime state requires unlocked vault"))?;
        let key = self
            .vault
            .resolve(&handle, &consumer)
            .map_err(|_| error("vault_locked", "Runtime key unavailable"));
        self.vault.revoke_handle(&handle.handle_id);
        key
    }
    pub fn write(&self, name: &str, plain: &[u8]) -> Result<(), RuntimeError> {
        let _lock = self
            .lock
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime state lock unavailable"))?;
        if plain.len() > MAX_STATE {
            return Err(error("state_limit", "Runtime state exceeds 8 MiB"));
        }
        let key = self.key()?;
        let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
            .map_err(|_| error("state_corrupt", "Invalid runtime encryption key"))?;
        let mut nonce = [0; 24];
        getrandom::fill(&mut nonce)
            .map_err(|_| error("state_unavailable", "Random source unavailable"))?;
        let sealed = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| error("state_unavailable", "Runtime encryption failed"))?;
        let dest = self.root.join(name);
        if std::fs::symlink_metadata(&dest).is_ok_and(|m| unsafe_link(&m)) {
            return Err(error("state_corrupt", "Runtime record is a symlink"));
        }
        let mut tmp = tempfile::NamedTempFile::new_in(&self.root)
            .map_err(|_| error("state_unavailable", "Runtime record cannot be written"))?;
        tmp.write_all(&nonce)
            .and_then(|()| tmp.write_all(&sealed))
            .and_then(|()| tmp.as_file().sync_all())
            .map_err(|_| error("state_unavailable", "Runtime record cannot be synced"))?;
        tmp.persist(&dest)
            .map_err(|_| error("state_unavailable", "Runtime record cannot be committed"))?;
        Ok(())
    }
    pub fn read(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, RuntimeError> {
        let _lock = self
            .lock
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime state lock unavailable"))?;
        // Locked state must never look like an empty configuration.
        let key = self.key()?;
        let path = self.root.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(error("state_unavailable", "Runtime record unavailable")),
        };
        if !metadata.is_file() || unsafe_link(&metadata) || metadata.len() > MAX_STATE as u64 + 64 {
            return Err(error("state_corrupt", "Invalid runtime record"));
        }
        let bytes = std::fs::read(path)
            .map_err(|_| error("state_unavailable", "Runtime record unavailable"))?;
        if bytes.len() < 40 {
            return Err(error("state_corrupt", "Truncated runtime record"));
        }
        let cipher = XChaCha20Poly1305::new_from_slice(key.expose())
            .map_err(|_| error("state_corrupt", "Invalid runtime key"))?;
        cipher
            .decrypt(
                XNonce::from_slice(&bytes[..24]),
                Payload {
                    msg: &bytes[24..],
                    aad: name.as_bytes(),
                },
            )
            .map(|p| Some(Zeroizing::new(p)))
            .map_err(|_| error("state_corrupt", "Runtime record authentication failed"))
    }
    /// Seal `home` as a `base64_v1` native recovery archive. A capture that
    /// fails (bounds, links, a busy vendor database) publishes nothing.
    pub fn snapshot_native(
        &self,
        name: &str,
        home: &Path,
        capture: Capture<'_>,
    ) -> Result<(), RuntimeError> {
        let plain = snapshot::capture(home, capture)?;
        self.write(name, &plain)
    }
    /// Remove one record; a missing record is not an error.
    pub fn remove(&self, name: &str) -> Result<(), RuntimeError> {
        let _lock = self
            .lock
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime state lock unavailable"))?;
        // Only an unlocked vault may change the sealed state.
        let _key = self.key()?;
        if Path::new(name)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
            || name.contains([':', '/', '\\'])
        {
            return Err(error("state_corrupt", "Invalid runtime record name"));
        }
        let path = self.root.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Ok(m) if m.is_file() && !unsafe_link(&m) => std::fs::remove_file(path)
                .map_err(|_| error("state_unavailable", "Runtime record cannot be removed")),
            _ => Err(error("state_corrupt", "Invalid runtime record")),
        }
    }
    /// Remove every native session snapshot of `runtime` (named
    /// `runtime-<digest(id)>-<digest(session:branch)>.sealed`).
    pub fn remove_runtime_snapshots(&self, runtime: &str) -> Result<(), RuntimeError> {
        let prefix = format!("runtime-{}-", digest(runtime.as_bytes()));
        for entry in std::fs::read_dir(&self.root)
            .map_err(|_| error("state_unavailable", "Runtime records unavailable"))?
        {
            let entry =
                entry.map_err(|_| error("state_unavailable", "Runtime records unavailable"))?;
            if let Some(name) = entry
                .file_name()
                .to_str()
                .filter(|n| n.starts_with(&prefix) && n.ends_with(".sealed"))
            {
                self.remove(name)?;
            }
        }
        Ok(())
    }
    /// Digest of a snapshot's plaintext: the evidence a resume reference binds.
    pub fn snapshot_digest(&self, name: &str) -> Result<String, RuntimeError> {
        self.read(name)?.map(|bytes| digest(&bytes)).ok_or_else(|| {
            error(
                "resume_unavailable",
                "Encrypted native snapshot evidence is missing",
            )
        })
    }
    fn account_record(profile: &str) -> String {
        format!("account-{}.sealed", digest(profile.as_bytes()))
    }
    /// The managed account's revision; it changes whenever a different account
    /// may have been connected, never on a credential refresh.
    pub fn account_revision(&self, profile: &str) -> Result<Option<String>, RuntimeError> {
        self.read(&Self::account_record(profile))?
            .map(|bytes| {
                let revision = std::str::from_utf8(&bytes)
                    .map_err(|_| error("state_corrupt", "Invalid managed account revision"))?;
                if revision.len() != 64 || !revision.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(error("state_corrupt", "Invalid managed account revision"));
                }
                Ok(revision.to_owned())
            })
            .transpose()
    }
    /// Start a new account revision and drop the runtime's session snapshots,
    /// which belong to the previous account.
    pub fn rotate_account_revision(
        &self,
        profile: &str,
        runtime: &str,
    ) -> Result<(), RuntimeError> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random)
            .map_err(|_| error("state_unavailable", "Managed account revision unavailable"))?;
        self.write(&Self::account_record(profile), digest(&random).as_bytes())?;
        self.remove_runtime_snapshots(runtime)
    }
    /// Restore an archive (either format) into `home`; `false` when the record
    /// does not exist.
    pub fn restore(&self, name: &str, home: &Path) -> Result<bool, RuntimeError> {
        let Some(plain) = self.read(name)? else {
            return Ok(false);
        };
        let files = snapshot::decode(&plain)?;
        for (path, bytes) in files {
            let bytes = Zeroizing::new(bytes);
            let relative = Path::new(&path);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
                || path.contains(':')
                || path.contains('\\')
            {
                return Err(error("state_corrupt", "Runtime snapshot path invalid"));
            }
            let path = home.join(relative);
            std::fs::create_dir_all(path.parent().unwrap())
                .and_then(|()| std::fs::write(path, bytes.as_slice()))
                .map_err(|_| error("state_unavailable", "Runtime snapshot cannot be restored"))?;
        }
        Ok(true)
    }
}
#[cfg(test)]
mod snapshot_tests {
    use super::*;
    fn state(root: &Path) -> State {
        let vault = Arc::new(
            VaultService::open(
                root.join("vault"),
                aidex_vault::VaultServiceConfig {
                    kdf: aidex_vault::KdfParams::new(8, 1, 1).unwrap(),
                    idle_timeout: None,
                },
            )
            .unwrap(),
        );
        vault
            .init(
                &aidex_vault::SecretString::from("snapshot-fixture-password".to_owned()),
                false,
            )
            .unwrap();
        State::new(root.join("private"), vault).unwrap()
    }
    #[test]
    fn codex_generated_tmp_is_excluded_and_durable_session_and_sqlite_files_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        std::fs::create_dir_all(home.join(".tmp/arg0")).unwrap();
        std::fs::create_dir(home.join("sessions")).unwrap();
        std::fs::File::create(home.join(".tmp/arg0/generated-tool.exe"))
            .unwrap()
            .set_len(MAX_STATE as u64)
            .unwrap();
        std::fs::write(
            home.join("sessions/rollout.jsonl"),
            b"durable native conversation",
        )
        .unwrap();
        std::fs::write(home.join("state.sqlite"), b"durable SQLite fixture").unwrap();
        std::fs::write(home.join("state.sqlite-wal"), b"durable WAL fixture").unwrap();
        let state = state(root.path());
        state
            .snapshot_native("fixture.sealed", &home, Capture::skipping(&[".tmp"]))
            .unwrap();
        let restored = root.path().join("restored");
        std::fs::create_dir(&restored).unwrap();
        assert!(state.restore("fixture.sealed", &restored).unwrap());
        assert!(!restored.join(".tmp").exists());
        assert_eq!(
            std::fs::read(restored.join("sessions/rollout.jsonl")).unwrap(),
            b"durable native conversation"
        );
        assert_eq!(
            std::fs::read(restored.join("state.sqlite")).unwrap(),
            b"durable SQLite fixture"
        );
        assert_eq!(
            std::fs::read(restored.join("state.sqlite-wal")).unwrap(),
            b"durable WAL fixture"
        );
        assert_eq!(
            state
                .snapshot_native("other.sealed", &home, Capture::skipping(&[]))
                .unwrap_err()
                .code,
            "state_limit"
        );
        assert!(state.read("other.sealed").unwrap().is_none());
    }
    #[test]
    fn a_legacy_byte_array_archive_still_restores() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        // The exact plaintext shape earlier daemons wrote for native snapshots.
        state
            .write(
                "legacy.sealed",
                br#"{"sessions/rollout.jsonl":[104,105],"auth.json":[123,125]}"#,
            )
            .unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        assert!(state.restore("legacy.sealed", &home).unwrap());
        assert_eq!(
            std::fs::read(home.join("sessions/rollout.jsonl")).unwrap(),
            b"hi"
        );
        assert_eq!(std::fs::read(home.join("auth.json")).unwrap(), b"{}");
        assert!(!state.restore("missing.sealed", &home).unwrap());
    }
    #[test]
    fn a_busy_vendor_database_publishes_no_partial_record() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("rollout.jsonl"), b"turn").unwrap();
        let database = rusqlite::Connection::open(home.join("state.sqlite")).unwrap();
        database
            .execute_batch("CREATE TABLE turns(content TEXT); BEGIN EXCLUSIVE; INSERT INTO turns VALUES('in flight');")
            .unwrap();
        let capture = Capture {
            skip: &[],
            sqlite_deadline: Duration::from_millis(200),
        };
        assert_eq!(
            state
                .snapshot_native("busy.sealed", &home, capture)
                .unwrap_err()
                .code,
            "state_busy"
        );
        assert!(state.read("busy.sealed").unwrap().is_none());
        database.execute_batch("COMMIT").unwrap();
        state
            .snapshot_native("busy.sealed", &home, capture)
            .unwrap();
        assert!(state.read("busy.sealed").unwrap().is_some());
    }
    #[test]
    fn snapshot_removal_is_scoped_to_one_runtime() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let ours = format!("runtime-{}-{}.sealed", digest(b"ours"), digest(b"s:b"));
        let theirs = format!("runtime-{}-{}.sealed", digest(b"theirs"), digest(b"s:b"));
        for name in [&ours, &theirs] {
            state.write(name, b"{}").unwrap();
        }
        state.write("runtime-config.sealed", b"[]").unwrap();
        state.rotate_account_revision("profile", "ours").unwrap();
        assert!(state.read(&ours).unwrap().is_none());
        assert!(state.read(&theirs).unwrap().is_some());
        assert!(state.read("runtime-config.sealed").unwrap().is_some());
        assert!(state.account_revision("profile").unwrap().is_some());
    }
}
