use aidex_native_sessions::RuntimeError;
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
    sync::{Arc, Mutex},
    time::Duration,
};
use zeroize::Zeroizing;
pub(crate) fn error(code: &str, message: &str) -> RuntimeError {
    RuntimeError {
        code: code.into(),
        message: message.into(),
    }
}
pub(crate) const MAX_STATE: usize = 8 * 1024 * 1024;
pub(crate) fn unsafe_link(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
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
    pub fn account_guard(&self) -> Result<std::sync::MutexGuard<'_, ()>, RuntimeError> {
        self.account_lock
            .lock()
            .map_err(|_| error("state_unavailable", "Managed account lock unavailable"))
    }
    pub fn set_login_pending(&self, id: &str, pending: bool) -> Result<(), RuntimeError> {
        let mut logins = self
            .pending_logins
            .lock()
            .map_err(|_| error("state_unavailable", "Managed login state unavailable"))?;
        if pending {
            logins.insert(id.to_owned());
        } else {
            logins.remove(id);
        }
        Ok(())
    }
    pub fn login_pending(&self, id: &str) -> Result<bool, RuntimeError> {
        Ok(self
            .pending_logins
            .lock()
            .map_err(|_| error("state_unavailable", "Managed login state unavailable"))?
            .contains(id))
    }
    fn key(&self) -> Result<SecretBytes, RuntimeError> {
        let name = SecretName::new("external-runtime-encryption-key").unwrap();
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
    pub fn snapshot(&self, name: &str, home: &Path) -> Result<(), RuntimeError> {
        self.snapshot_excluding(name, home, &[])
    }
    pub fn snapshot_excluding(
        &self,
        name: &str,
        home: &Path,
        excluded: &[Zeroizing<String>],
    ) -> Result<(), RuntimeError> {
        self.snapshot_policy(name, home, excluded, false)
    }
    pub fn snapshot_native(
        &self,
        name: &str,
        home: &Path,
        driver: crate::Driver,
        excluded: &[Zeroizing<String>],
    ) -> Result<(), RuntimeError> {
        self.snapshot_policy(name, home, excluded, driver == crate::Driver::Codex)
    }
    fn snapshot_policy(
        &self,
        name: &str,
        home: &Path,
        excluded: &[Zeroizing<String>],
        skip_codex_tmp: bool,
    ) -> Result<(), RuntimeError> {
        let plain = crate::snapshot::capture(home, skip_codex_tmp, excluded)?;
        self.write(name, &plain)
    }
    pub fn remove(&self, name: &str) -> Result<(), RuntimeError> {
        let _lock = self
            .lock
            .lock()
            .map_err(|_| error("state_unavailable", "Runtime state lock unavailable"))?;
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
    pub fn remove_runtime_snapshots(&self, runtime: &str) -> Result<(), RuntimeError> {
        let prefix = format!(
            "runtime-{}-",
            aidex_native_sessions::digest(runtime.as_bytes())
        );
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
    pub fn snapshot_digest(&self, name: &str) -> Result<String, RuntimeError> {
        self.read(name)?
            .map(|bytes| aidex_native_sessions::digest(&bytes))
            .ok_or_else(|| {
                error(
                    "resume_unavailable",
                    "Encrypted native snapshot evidence is missing",
                )
            })
    }
    pub fn account_revision(&self, profile: &str) -> Result<Option<String>, RuntimeError> {
        let record = format!(
            "account-{}.sealed",
            aidex_native_sessions::digest(profile.as_bytes())
        );
        self.read(&record)?
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
    pub fn rotate_account_revision(
        &self,
        profile: &str,
        runtime: &str,
    ) -> Result<(), RuntimeError> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random)
            .map_err(|_| error("state_unavailable", "Managed account revision unavailable"))?;
        self.write(
            &format!(
                "account-{}.sealed",
                aidex_native_sessions::digest(profile.as_bytes())
            ),
            aidex_native_sessions::digest(&random).as_bytes(),
        )?;
        self.remove_runtime_snapshots(runtime)
    }
    pub fn restore(&self, name: &str, home: &Path) -> Result<(), RuntimeError> {
        let Some(plain) = self.read(name)? else {
            return Ok(());
        };
        let files = crate::snapshot::decode(&plain)?;
        for (path, bytes) in files {
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
                .and_then(|()| std::fs::write(path, bytes))
                .map_err(|_| error("state_unavailable", "Runtime snapshot cannot be restored"))?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod snapshot_tests {
    use super::*;
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
        let vault = Arc::new(
            VaultService::open(
                root.path().join("vault"),
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
        let state = State::new(root.path().join("private"), vault).unwrap();
        state
            .snapshot_native("fixture.sealed", &home, crate::Driver::Codex, &[])
            .unwrap();
        let restored = root.path().join("restored");
        std::fs::create_dir(&restored).unwrap();
        state.restore("fixture.sealed", &restored).unwrap();
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
                .snapshot_native("other.sealed", &home, crate::Driver::Acp, &[])
                .unwrap_err()
                .code,
            "state_limit"
        );
    }
}
