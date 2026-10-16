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
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant, SystemTime},
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
/// The host temporary folder's parent of every daemon's runtime and account
/// homes: `<temp>/aidex-rt/<per daemon>/`.
pub(crate) fn temp_base() -> PathBuf {
    std::env::temp_dir().join("aidex-rt")
}
/// The lock file a daemon holds for as long as its temporary root is in use.
const TEMP_LOCK: &str = "daemon.lock";
/// A root whose lock file is younger than this is never swept: its daemon
/// may be between creating the lock file and taking its lock.
pub(crate) const TEMP_SWEEP_GRACE: Duration = Duration::from_secs(60);
/// One daemon's parent of temporary homes. Its lock is held until it is
/// dropped, so a later daemon can tell a crashed daemon's root from a live one.
struct TempRoot {
    // Fields drop in order: the lock is released, then the folder removed,
    // then the shared base removed unless another daemon's root is left in it.
    _lock: std::fs::File,
    dir: tempfile::TempDir,
    _base: PruneBase,
}
/// Removes the shared base of daemon roots when dropped, if it is empty.
struct PruneBase(PathBuf);
impl Drop for PruneBase {
    fn drop(&mut self) {
        // Not recursive: fails harmlessly while another daemon's root exists.
        let _ = std::fs::remove_dir(&self.0);
    }
}
/// Create `base` (only the owner may enter it, on Unix) if it is missing.
fn make_base(base: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(base)
}
/// Whether a shared base with `metadata` belongs to `uid` and nobody else
/// may add, rename or remove entries in it: another user who could would
/// swap a daemon's root for a link to their own folder and read the
/// plaintext homes made in it.
#[cfg(unix)]
fn private_base(metadata: &std::fs::Metadata, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    metadata.uid() == uid && metadata.mode() & 0o022 == 0
}
/// The user id this process creates files as, read from an anonymous
/// temporary file.
#[cfg(unix)]
fn own_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let file = tempfile::tempfile().ok()?;
    file.metadata().ok().map(|metadata| metadata.uid())
}
impl TempRoot {
    /// A new locked root under `base`. Fails (the caller then falls back to a
    /// plain temporary folder) when `base` is a link or, on Unix, belongs to
    /// another user or is writable by others.
    fn create(base: &Path) -> std::io::Result<Self> {
        let mut attempts = 0;
        let dir = loop {
            attempts += 1;
            let made = make_base(base).and_then(|()| {
                let metadata = std::fs::symlink_metadata(base)?;
                if unsafe_link(&metadata) || !metadata.is_dir() {
                    return Err(std::io::Error::other("temporary base is a link"));
                }
                tempfile::Builder::new().prefix("d-").tempdir_in(base)
            });
            match made {
                // Another daemon's last root (or a sweep) took the empty base
                // with it in between: make it again.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && attempts < 3 => {}
                made => break made?,
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // Checked after the root exists, so the base it was made in is
            // the one checked: only its owner can replace it in a sticky
            // temporary folder.
            let uid = std::fs::symlink_metadata(dir.path())?.uid();
            let metadata = std::fs::symlink_metadata(base)?;
            if unsafe_link(&metadata) || !private_base(&metadata, uid) {
                // Dropping `dir` removes the root; the base is left alone.
                return Err(std::io::Error::other("temporary base is not private"));
            }
        }
        let base = PruneBase(base.to_owned());
        let lock = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(dir.path().join(TEMP_LOCK))
            .and_then(|lock| fs2::FileExt::try_lock_exclusive(&lock).map(|()| lock));
        match lock {
            Ok(lock) => Ok(Self {
                _lock: lock,
                dir,
                _base: base,
            }),
            Err(e) => {
                // The root first, then the base it may leave empty.
                drop(dir);
                drop(base);
                Err(e)
            }
        }
    }
}
/// How long ago `metadata` was last modified; `None` when unknown or in the
/// future.
fn age(metadata: &std::fs::Metadata) -> Option<Duration> {
    metadata
        .modified()
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
}
/// Remove every daemon root under `base` whose lock no daemon holds: what a
/// crash, kill, power loss or pulled drive left mid-launch or mid-login
/// (restored credentials and decrypted native history). Best effort: links
/// and reparse points are never followed or removed, a root whose lock file
/// (or, without one, the root itself) is younger than `grace` is left alone,
/// and nothing fails over a root that cannot be removed. On Unix a base
/// another user owns is not touched. The base itself is removed once no root
/// is left in it. Returns how many roots were removed.
pub(crate) fn sweep_stale_temp_roots(base: &Path, grace: Duration) -> usize {
    sweep_roots(base, grace, owned_by_us)
}
/// Whether this process's user owns the entry `metadata` describes; always
/// on hosts without Unix ownership, whose temporary folder is per user.
fn owned_by_us(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        own_uid() == Some(metadata.uid())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}
/// [`sweep_stale_temp_roots`], touching nothing under a base `ours` rejects.
fn sweep_roots(base: &Path, grace: Duration, ours: fn(&std::fs::Metadata) -> bool) -> usize {
    let Ok(base_metadata) = std::fs::symlink_metadata(base) else {
        return 0;
    };
    if unsafe_link(&base_metadata) || !base_metadata.is_dir() || !ours(&base_metadata) {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(base) else {
        return 0;
    };
    let recent = |metadata: &std::fs::Metadata| age(metadata).is_none_or(|age| age < grace);
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if unsafe_link(&metadata) || !metadata.is_dir() {
            continue;
        }
        let lock = path.join(TEMP_LOCK);
        match std::fs::symlink_metadata(&lock) {
            Ok(m) if unsafe_link(&m) || !m.is_file() || recent(&m) => continue,
            Ok(_) => {
                let Ok(file) = std::fs::OpenOptions::new().write(true).open(&lock) else {
                    continue;
                };
                if fs2::FileExt::try_lock_exclusive(&file).is_err() {
                    // A live daemon holds it.
                    continue;
                }
                // Closing the handle releases the lock before the removal.
                drop(file);
            }
            // A root whose daemon died before it created its lock file.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !recent(&metadata) => {}
            Err(_) => continue,
        }
        // Like `TempDir`'s own drop, this removes links without following them.
        if std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    // Not recursive: fails harmlessly while any root (live or kept) is left.
    let _ = std::fs::remove_dir(base);
    removed
}
/// While held, other launches of the same managed account wait before they
/// restore its credential: the holder may be refreshing the rotating refresh
/// token the stored profile still holds.
pub(crate) struct ProfileLease {
    state: Arc<State>,
    profile: String,
}
impl Drop for ProfileLease {
    fn drop(&mut self) {
        self.state
            .leases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.profile);
        self.state.lease_freed.notify_all();
    }
}
/// How a wait for a [`ProfileLease`] ended.
pub(crate) enum Lease {
    Held(ProfileLease),
    /// The holder kept it past the deadline; the launch goes on without one.
    TimedOut,
    /// The waiting launch was cancelled or its gate closed.
    Stopped,
}
/// Counts one live launch of a managed account until dropped.
pub(crate) struct ProfileUse {
    state: Arc<State>,
    profile: String,
}
impl Drop for ProfileUse {
    fn drop(&mut self) {
        let mut uses = self
            .state
            .uses
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = uses.get_mut(&self.profile) {
            *count -= 1;
            if *count == 0 {
                uses.remove(&self.profile);
            }
        }
    }
}
pub(crate) struct State {
    pub root: PathBuf,
    pub vault: Arc<VaultService>,
    lock: Mutex<()>,
    account_lock: Mutex<()>,
    pending_logins: Mutex<BTreeSet<String>>,
    /// Managed accounts whose credential a launch is starting with.
    leases: Mutex<BTreeSet<String>>,
    lease_freed: Condvar,
    /// Live launches per managed account.
    uses: Mutex<BTreeMap<String, usize>>,
    /// Serializes the compare-and-save of refreshed managed credentials.
    credential_lock: Mutex<()>,
    temp: Mutex<Option<TempRoot>>,
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
            leases: Mutex::new(BTreeSet::new()),
            lease_freed: Condvar::new(),
            uses: Mutex::new(BTreeMap::new()),
            credential_lock: Mutex::new(()),
            temp: Mutex::new(None),
        })
    }
    /// A new temporary home (`<prefix>XXXX`) under this daemon's locked root
    /// in the host temporary folder (`<temp>/aidex-rt/d-XXXX/`), so a crash
    /// leaves it to a later daemon's sweep ([`sweep_stale_temp_roots`]).
    /// Dropping it removes it; the root goes with this state, and the shared
    /// `aidex-rt` base with the last root in it. A host where no locked root
    /// can be made (on Unix, also one whose base another user owns or may
    /// write to) gets a plain temporary folder instead, as before.
    pub fn temp_home(&self, prefix: &str) -> Result<tempfile::TempDir, RuntimeError> {
        let unavailable = || error("state_unavailable", "Runtime temporary home unavailable");
        let mut root = self.temp.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = root.as_ref() {
            match tempfile::Builder::new()
                .prefix(prefix)
                .tempdir_in(existing.dir.path())
            {
                Ok(home) => return Ok(home),
                // The root is still there: the failure is not its absence, and
                // replacing the root would remove live homes with it.
                Err(_) if existing.dir.path().is_dir() => return Err(unavailable()),
                // Removed from outside: start a new one.
                Err(_) => {}
            }
        }
        let Ok(fresh) = TempRoot::create(&temp_base()) else {
            return tempfile::Builder::new()
                .prefix(prefix)
                .tempdir()
                .map_err(|_| unavailable());
        };
        let home = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(fresh.dir.path())
            .map_err(|_| unavailable())?;
        *root = Some(fresh);
        Ok(home)
    }
    /// How many temporary homes this daemon's root holds now (launches,
    /// parked approvals and account commands); for diagnostics and tests.
    pub fn temp_homes(&self) -> usize {
        let root = self.temp.lock().unwrap_or_else(PoisonError::into_inner);
        root.as_ref()
            .and_then(|root| std::fs::read_dir(root.dir.path()).ok())
            .map_or(0, |entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.file_name() != TEMP_LOCK)
                    .count()
            })
    }
    /// Wait until no other launch is starting with the managed account
    /// `profile`, then hold it. `stopped` is polled while waiting.
    pub fn lease_profile(
        self: &Arc<Self>,
        profile: &str,
        timeout: Duration,
        stopped: &dyn Fn() -> bool,
    ) -> Lease {
        let deadline = Instant::now() + timeout;
        let mut leases = self.leases.lock().unwrap_or_else(PoisonError::into_inner);
        while leases.contains(profile) {
            if stopped() {
                return Lease::Stopped;
            }
            let now = Instant::now();
            if now >= deadline {
                return Lease::TimedOut;
            }
            let slice = (deadline - now).min(Duration::from_millis(25));
            leases = self
                .lease_freed
                .wait_timeout(leases, slice)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        leases.insert(profile.to_owned());
        Lease::Held(ProfileLease {
            state: self.clone(),
            profile: profile.to_owned(),
        })
    }
    /// Count a live launch of the managed account `profile`.
    pub fn use_profile(self: &Arc<Self>, profile: &str) -> ProfileUse {
        *self
            .uses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(profile.to_owned())
            .or_default() += 1;
        ProfileUse {
            state: self.clone(),
            profile: profile.to_owned(),
        }
    }
    /// Whether a launch of the managed account `profile` is live.
    pub fn profile_in_use(&self, profile: &str) -> bool {
        self.uses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(profile)
    }
    /// Held while a refreshed managed credential is compared with the stored
    /// one and saved.
    pub fn credential_guard(&self) -> Result<MutexGuard<'_, ()>, RuntimeError> {
        self.credential_lock
            .lock()
            .map_err(|_| error("state_unavailable", "Managed credential lock unavailable"))
    }
    /// Digest of the credential the managed account profile record holds,
    /// in the form `profiles::credential_digest` gives for a restored home;
    /// `None` when the account is signed out.
    pub fn stored_credential_digest(&self, profile: &str) -> Result<Option<String>, RuntimeError> {
        let Some(plain) = self.read(&crate::profiles::profile_record(profile))? else {
            return Ok(None);
        };
        let mut stored = None;
        for (path, bytes) in snapshot::decode(&plain)? {
            let bytes = Zeroizing::new(bytes);
            if path == "auth.json" {
                stored = Some(digest(&bytes));
            }
        }
        Ok(stored)
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
    /// Whether `name` is one of `runtime`'s branch-scoped native session
    /// snapshots (`runtime-<digest(id)>-<digest(session:branch)>.sealed`).
    pub fn is_branch_snapshot(runtime: &str, name: &str) -> bool {
        name.strip_prefix(&format!("runtime-{}-", digest(runtime.as_bytes())))
            .and_then(|rest| rest.strip_suffix(".sealed"))
            .is_some_and(|branch| {
                branch.len() == 64
                    && branch
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
    }
    /// The snapshot name daemons before branch-scoped snapshots used for a
    /// session of `runtime`.
    pub fn legacy_snapshot(runtime: &str, session: &str) -> String {
        format!(
            "runtime-{}.sealed",
            digest(format!("{runtime}:{session}").as_bytes())
        )
    }
    /// Whether `name` has the form daemons before branch-scoped snapshots gave
    /// every session snapshot (`runtime-<digest(runtime:session)>.sealed`,
    /// one digest). The name does not tell which runtime it belongs to.
    pub fn is_legacy_snapshot(name: &str) -> bool {
        name.strip_prefix("runtime-")
            .and_then(|rest| rest.strip_suffix(".sealed"))
            .is_some_and(|digest| {
                digest.len() == 64
                    && digest
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
    }
    /// Remove every record whose name `matches`.
    fn remove_matching(&self, matches: impl Fn(&str) -> bool) -> Result<(), RuntimeError> {
        for entry in std::fs::read_dir(&self.root)
            .map_err(|_| error("state_unavailable", "Runtime records unavailable"))?
        {
            let entry =
                entry.map_err(|_| error("state_unavailable", "Runtime records unavailable"))?;
            if let Some(name) = entry.file_name().to_str().filter(|n| matches(n)) {
                self.remove(name)?;
            }
        }
        Ok(())
    }
    /// Remove every native session snapshot of `runtime` (named
    /// `runtime-<digest(id)>-<digest(session:branch)>.sealed`).
    pub fn remove_runtime_snapshots(&self, runtime: &str) -> Result<(), RuntimeError> {
        let prefix = format!("runtime-{}-", digest(runtime.as_bytes()));
        self.remove_matching(|name| name.starts_with(&prefix) && name.ends_with(".sealed"))
    }
    /// Remove every snapshot an older daemon saved under the legacy name, of
    /// any runtime: in profile mode such a daemon sealed the whole home, so
    /// one can hold a managed account's `auth.json`, and nothing tells whose
    /// it is. This daemon never restores them; an older one would, on a
    /// resume after a rollback. For a sign-out, an account change and the
    /// removal of a managed account.
    pub fn remove_legacy_snapshots(&self) -> Result<(), RuntimeError> {
        self.remove_matching(Self::is_legacy_snapshot)
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
    /// Remove the managed account's revision record (the account is gone).
    pub fn remove_account_revision(&self, profile: &str) -> Result<(), RuntimeError> {
        self.remove(&Self::account_record(profile))
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
    #[test]
    fn only_a_runtimes_own_branch_snapshot_names_are_branch_snapshots() {
        let branch = format!("runtime-{}-{}.sealed", digest(b"ours"), digest(b"s:b"));
        assert!(State::is_branch_snapshot("ours", &branch));
        assert!(!State::is_branch_snapshot("theirs", &branch));
        for forged in [
            "runtime-config.sealed".to_owned(),
            State::legacy_snapshot("ours", "s"),
            format!("runtime-{}-../profile.sealed", digest(b"ours")),
            format!("runtime-{}-{}.sealed", digest(b"ours"), "A".repeat(64)),
            format!("runtime-{}-{}", digest(b"ours"), digest(b"s:b")),
        ] {
            assert!(!State::is_branch_snapshot("ours", &forged), "{forged}");
        }
    }
    #[test]
    fn the_legacy_sweep_removes_only_legacy_named_snapshots() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        let legacy = [
            State::legacy_snapshot("codex-fixture", "s"),
            State::legacy_snapshot("other-runtime", "t"),
        ];
        let branch = format!("runtime-{}-{}.sealed", digest(b"ours"), digest(b"s:b"));
        let kept = [
            branch.clone(),
            "runtime-config.sealed".to_owned(),
            crate::profiles::profile_record("p"),
            State::account_record("p"),
        ];
        for name in legacy.iter().chain(&kept) {
            assert_eq!(State::is_legacy_snapshot(name), legacy.contains(name));
            state.write(name, b"{}").unwrap();
        }
        assert!(!State::is_legacy_snapshot(&format!(
            "runtime-{}.sealed",
            "A".repeat(64)
        )));
        state.remove_legacy_snapshots().unwrap();
        for name in &legacy {
            assert!(state.read(name).unwrap().is_none(), "{name}");
        }
        for name in &kept {
            assert!(state.read(name).unwrap().is_some(), "{name}");
        }
        state.remove_account_revision("p").unwrap();
        assert!(state.read(&State::account_record("p")).unwrap().is_none());
    }
    /// A daemon root as a crashed daemon leaves it: a runtime home with a
    /// restored credential and a lock file nobody holds.
    fn crashed_root(base: &Path, name: &str, lock_age: Duration) -> PathBuf {
        let root = base.join(name);
        std::fs::create_dir_all(root.join("aidex-runtime-x/home")).unwrap();
        std::fs::write(root.join("aidex-runtime-x/home/auth.json"), b"{}").unwrap();
        let lock = std::fs::File::create(root.join(TEMP_LOCK)).unwrap();
        lock.set_modified(SystemTime::now() - lock_age).unwrap();
        root
    }
    #[test]
    fn the_sweep_removes_only_roots_no_daemon_holds() {
        let base = tempfile::tempdir().unwrap();
        let old = Duration::from_secs(3600);
        let stale = crashed_root(base.path(), "d-stale", old);
        let held = crashed_root(base.path(), "d-held", old);
        let holder = std::fs::OpenOptions::new()
            .write(true)
            .open(held.join(TEMP_LOCK))
            .unwrap();
        fs2::FileExt::try_lock_exclusive(&holder).unwrap();
        // Its daemon may be about to take the lock it just created.
        let starting = crashed_root(base.path(), "d-starting", Duration::ZERO);
        // A root a live daemon made through `TempRoot`.
        let live = TempRoot::create(base.path()).unwrap();
        assert_eq!(sweep_stale_temp_roots(base.path(), TEMP_SWEEP_GRACE), 1);
        assert!(!stale.exists());
        assert!(held.join("aidex-runtime-x/home/auth.json").exists());
        assert!(starting.exists());
        assert!(live.dir.path().join(TEMP_LOCK).exists());
        drop(holder);
        assert_eq!(sweep_stale_temp_roots(base.path(), TEMP_SWEEP_GRACE), 1);
        assert!(!held.exists());
        assert!(live.dir.path().exists());
        let path = live.dir.path().to_owned();
        drop(live);
        assert!(!path.exists());
    }
    #[test]
    fn the_sweep_never_follows_or_removes_a_link() {
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = crashed_root(outside.path(), "d-target", Duration::from_secs(3600));
        let link = base.path().join("d-link");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&target, &link);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, &link);
        if linked.is_err() {
            // Creating links needs a privilege this host does not grant.
            return;
        }
        assert_eq!(sweep_stale_temp_roots(base.path(), TEMP_SWEEP_GRACE), 0);
        assert!(target.join("aidex-runtime-x/home/auth.json").exists());
    }
    #[test]
    fn the_shared_base_goes_with_the_last_root() {
        let parent = tempfile::tempdir().unwrap();
        let base = parent.path().join("aidex-rt");
        let first = TempRoot::create(&base).unwrap();
        let second = TempRoot::create(&base).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&base).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        drop(first);
        // Another daemon's root is still in it.
        assert!(base.is_dir());
        drop(second);
        assert!(!base.exists());
        // A later root makes it again, and the sweep leaves no empty base.
        drop(TempRoot::create(&base).unwrap());
        assert!(!base.exists());
        crashed_root(&base, "d-stale", Duration::from_secs(3600));
        assert_eq!(sweep_stale_temp_roots(&base, TEMP_SWEEP_GRACE), 1);
        assert!(!base.exists());
    }
    #[test]
    fn the_sweep_leaves_a_base_that_is_not_ours_alone() {
        let parent = tempfile::tempdir().unwrap();
        let base = parent.path().join("aidex-rt");
        let stale = crashed_root(&base, "d-stale", Duration::from_secs(3600));
        assert_eq!(sweep_roots(&base, TEMP_SWEEP_GRACE, |_| false), 0);
        assert!(stale.join("aidex-runtime-x/home/auth.json").exists());
        assert_eq!(sweep_roots(&base, TEMP_SWEEP_GRACE, owned_by_us), 1);
        assert!(!base.exists());
    }
    #[cfg(unix)]
    #[test]
    fn a_base_another_user_owns_or_may_write_to_is_not_used() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let parent = tempfile::tempdir().unwrap();
        let base = parent.path().join("aidex-rt");
        make_base(&base).unwrap();
        let metadata = std::fs::symlink_metadata(&base).unwrap();
        assert!(private_base(&metadata, metadata.uid()));
        assert!(!private_base(&metadata, metadata.uid().wrapping_add(1)));
        for mode in [0o770, 0o707, 0o1777] {
            std::fs::set_permissions(&base, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(TempRoot::create(&base).is_err(), "{mode:o}");
            // The root it made there is gone, and the base is left as it was.
            assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0, "{mode:o}");
        }
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(TempRoot::create(&base).is_ok());
    }
    #[test]
    fn a_profile_lease_holds_other_launches_until_it_is_released() {
        let root = tempfile::tempdir().unwrap();
        let state = Arc::new(state(root.path()));
        let Lease::Held(first) = state.lease_profile("p", Duration::from_secs(5), &|| false) else {
            panic!("free lease");
        };
        assert!(matches!(
            state.lease_profile("p", Duration::from_millis(50), &|| false),
            Lease::TimedOut
        ));
        assert!(matches!(
            state.lease_profile("p", Duration::from_secs(5), &|| true),
            Lease::Stopped
        ));
        // Another account is independent.
        assert!(matches!(
            state.lease_profile("q", Duration::from_millis(50), &|| false),
            Lease::Held(_)
        ));
        let waiter = {
            let state = state.clone();
            std::thread::spawn(move || {
                matches!(
                    state.lease_profile("p", Duration::from_secs(5), &|| false),
                    Lease::Held(_)
                )
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        drop(first);
        assert!(waiter.join().unwrap());
        let used = state.use_profile("p");
        assert!(state.profile_in_use("p"));
        drop(used);
        assert!(!state.profile_in_use("p"));
    }
    #[test]
    fn the_stored_credential_digest_matches_a_restored_home() {
        let root = tempfile::tempdir().unwrap();
        let state = state(root.path());
        assert!(state.stored_credential_digest("p").unwrap().is_none());
        let credential = br#"{"tokens":{"access_token":"fixture"}}"#;
        let files = std::collections::BTreeMap::from([(
            "auth.json".to_owned(),
            Zeroizing::new(credential.to_vec()),
        )]);
        state
            .write(
                &crate::profiles::profile_record("p"),
                &snapshot::encode_legacy(&files).unwrap(),
            )
            .unwrap();
        let home = root.path().join("home");
        std::fs::create_dir(&home).unwrap();
        assert!(
            state
                .restore(&crate::profiles::profile_record("p"), &home)
                .unwrap()
        );
        assert_eq!(
            state.stored_credential_digest("p").unwrap(),
            Some(crate::profiles::credential_digest(&home).unwrap())
        );
    }
}
