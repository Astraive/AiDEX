use aidex_native_sessions::RuntimeError;
use aidex_vault::{
    ConsumerScope, SecretBytes, SecretKind, SecretName, SecretPolicy, VaultErrorKind, VaultService,
};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use std::{
    collections::BTreeMap,
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
pub(crate) struct State {
    pub root: PathBuf,
    pub vault: Arc<VaultService>,
    lock: Mutex<()>,
}
impl State {
    pub fn new(root: PathBuf, vault: Arc<VaultService>) -> Result<Self, RuntimeError> {
        std::fs::create_dir_all(&root).map_err(|_| {
            error(
                "state_unavailable",
                "Runtime state directory cannot be created",
            )
        })?;
        if std::fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(error("invalid_config", "Runtime state cannot be a symlink"));
        }
        Ok(Self {
            root,
            vault,
            lock: Mutex::new(()),
        })
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
        if std::fs::symlink_metadata(&dest).is_ok_and(|m| m.file_type().is_symlink()) {
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
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.len() > MAX_STATE as u64 + 64
        {
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
        let mut files = BTreeMap::<String, Vec<u8>>::new();
        let mut size = 0;
        collect(home, home, &mut files, &mut size)?;
        let plain = Zeroizing::new(
            serde_json::to_vec(&files)
                .map_err(|_| error("state_corrupt", "Runtime snapshot invalid"))?,
        );
        self.write(name, &plain)
    }
    pub fn restore(&self, name: &str, home: &Path) -> Result<(), RuntimeError> {
        let Some(plain) = self.read(name)? else {
            return Ok(());
        };
        let files: BTreeMap<String, Vec<u8>> = serde_json::from_slice(&plain)
            .map_err(|_| error("state_corrupt", "Runtime snapshot invalid"))?;
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
fn collect(
    root: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    size: &mut usize,
) -> Result<(), RuntimeError> {
    for entry in std::fs::read_dir(dir)
        .map_err(|_| error("state_unavailable", "Runtime profile cannot be read"))?
    {
        let entry =
            entry.map_err(|_| error("state_unavailable", "Runtime profile cannot be read"))?;
        let kind = entry
            .file_type()
            .map_err(|_| error("state_unavailable", "Runtime profile cannot be read"))?;
        if kind.is_symlink() {
            return Err(error("state_corrupt", "Runtime profile contains a symlink"));
        }
        if kind.is_dir() {
            collect(root, &entry.path(), files, size)?;
        } else if kind.is_file() {
            let bytes = std::fs::read(entry.path())
                .map_err(|_| error("state_unavailable", "Runtime profile cannot be read"))?;
            *size += bytes.len();
            if *size > MAX_STATE / 4 || files.len() > 4096 {
                return Err(error(
                    "state_limit",
                    "Runtime profile snapshot exceeds bounds",
                ));
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .components()
                .map(|c| {
                    c.as_os_str().to_str().ok_or_else(|| {
                        error("state_corrupt", "Runtime profile filename unsupported")
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            files.insert(relative, bytes);
        }
    }
    Ok(())
}
