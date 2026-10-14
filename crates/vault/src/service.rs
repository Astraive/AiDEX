//! `VaultService`: the thread-safe, daemon-owned vault with idle auto-lock.
//!
//! The daemon exposes user operations (init/unlock/lock/set/list/delete/passphrase)
//! over IPC. Handle issuance and resolution are Rust-only methods for the broker; they
//! are deliberately not RPC methods. While sealed, state is re-read from disk on every
//! call, so external restores or damage are reported instead of a stale cache. Every
//! end of an unlocked session is reported once to the lock observers (see
//! [`crate::observer`]), and the next session begins only after they have seen it.
//! The one exception: unlocking while a session is open whose `vault.json` changed
//! on disk replaces that session without a notice, since the vault never locks.

use crate::{
    BackupReport, ConsumerScope, CreateOptions, KdfParams, LockNotice, LockReason, RecoveryKey,
    SecretBytes, SecretHandle, SecretKind, SecretMeta, SecretName, SecretPolicy, SecretString,
    UnlockMethod, UnlockedVault, Vault, VaultError, VaultErrorKind, observer::LockNotices,
};
use aidex_core::{EntityId, Timestamp};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, Weak},
    thread,
    time::{Duration, Instant},
};

/// Default idle period after which an unlocked vault locks itself.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_TICK: Duration = Duration::from_secs(1);
const MIN_TICK: Duration = Duration::from_millis(10);

/// Daemon-side vault configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VaultServiceConfig {
    /// Argon2id parameters for vaults created by [`VaultService::init`]. Existing
    /// vaults keep the parameters recorded in their header.
    pub kdf: KdfParams,
    /// Lock after this long without secret operations; `None` disables auto-lock.
    pub idle_timeout: Option<Duration>,
}

impl Default for VaultServiceConfig {
    fn default() -> Self {
        Self {
            kdf: KdfParams::default(),
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
        }
    }
}

/// Coarse vault state for status reporting.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VaultState {
    /// No vault exists on the drive yet.
    Missing,
    Locked,
    Unlocked,
    /// Vault data exists but cannot be used (missing header, corruption, unknown version).
    Unavailable,
}

/// `vault.status` result. Contains no secret material.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultStatus {
    pub state: VaultState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_id: Option<EntityId>,
    /// Number of secrets (only while unlocked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub has_recovery_key: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlocked_by: Option<UnlockMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unlocked_at: Option<Timestamp>,
    /// Auto-lock period in seconds, when enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_lock_secs: Option<u64>,
    /// Stable error kind when `state` is `unavailable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Result of [`VaultService::init`]. The recovery key is returned once and not kept.
pub struct InitOutcome {
    pub vault_id: EntityId,
    pub recovery_key: Option<RecoveryKey>,
}

impl fmt::Debug for InitOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InitOutcome")
            .field("vault_id", &self.vault_id)
            .field("recovery_key", &self.recovery_key)
            .finish()
    }
}

/// Result of [`VaultService::lock`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockOutcome {
    /// Whether the vault was unlocked before this call.
    pub was_unlocked: bool,
    /// Handles that were still live and are now unusable.
    pub revoked_handles: usize,
}

struct Session {
    vault: UnlockedVault,
    unlocked_at: Timestamp,
    last_used: Instant,
}

struct Inner {
    dir: PathBuf,
    config: Mutex<VaultServiceConfig>,
    session: Mutex<Option<Session>>,
    notices: Arc<LockNotices>,
}

/// The daemon's vault: `Arc`-shareable, all methods take `&self`.
pub struct VaultService {
    inner: Arc<Inner>,
}

impl VaultService {
    /// Serve the vault stored in `dir`. Never fails because of vault contents (a
    /// damaged vault is reported by [`VaultService::status`]); starts the thread that
    /// auto-locks and delivers lock notices.
    pub fn open(dir: impl Into<PathBuf>, config: VaultServiceConfig) -> Result<Self, VaultError> {
        let dir = dir.into();
        let notices = Arc::new(LockNotices::default());
        let inner = Arc::new(Inner {
            dir: dir.clone(),
            config: Mutex::new(config),
            session: Mutex::new(None),
            notices: Arc::clone(&notices),
        });
        let weak = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("aidex-vault-autolock".into())
            .spawn(move || auto_lock_loop(&weak, &notices))
            .map_err(|error| VaultError::io("start the auto-lock thread for vault", dir, error))?;
        Ok(Self { inner })
    }

    /// Call `observer` once for every unlocked session that ends, whatever the
    /// [`LockReason`]. Observers never run while the vault's own locks are held:
    /// [`VaultService::lock`] runs them on its caller's thread before it returns,
    /// and every other ending runs them on the service's background thread, one
    /// notice at a time and in order ([`VaultService::unlock`] and
    /// [`VaultService::init`] wait for them, or run them on their own thread, before
    /// a new session begins). An observer must not hold a strong reference to this
    /// service (or to anything that owns it), or neither is ever dropped.
    pub fn add_lock_observer(&self, observer: impl Fn(&LockNotice) + Send + Sync + 'static) {
        self.inner.notices.observe(Arc::new(observer));
    }

    /// Whether `error`, returned by an operation on the unlocked vault (secrets,
    /// handles, passphrase change, backup), ended the session: a `Conflict`, or an
    /// uncertain commit of this vault's own header. The vault must be unlocked again.
    pub fn ends_session(&self, error: &VaultError) -> bool {
        self.ending_reason(error).is_some()
    }

    fn ending_reason(&self, error: &VaultError) -> Option<LockReason> {
        match error {
            VaultError::CommitUncertain { path, .. } if path.parent() == Some(self.dir()) => {
                Some(LockReason::CommitUncertain)
            }
            _ if error.kind() == VaultErrorKind::Conflict => Some(LockReason::Conflict),
            _ => None,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.inner.dir
    }

    pub fn config(&self) -> VaultServiceConfig {
        *lock_or_recover(&self.inner.config)
    }

    /// KDF parameters for vaults created later by [`VaultService::init`].
    pub fn set_kdf_params(&self, kdf: KdfParams) {
        lock_or_recover(&self.inner.config).kdf = kdf;
    }

    pub fn set_idle_timeout(&self, idle_timeout: Option<Duration>) {
        lock_or_recover(&self.inner.config).idle_timeout = idle_timeout;
    }

    pub fn status(&self) -> VaultStatus {
        let idle = self.config().idle_timeout;
        let mut session = self.session();
        self.inner
            .expire_if_idle(&mut session, idle, Instant::now());
        let base = VaultStatus {
            state: VaultState::Missing,
            vault_id: None,
            secrets: None,
            has_recovery_key: None,
            unlocked_by: None,
            unlocked_at: None,
            idle_lock_secs: idle.map(|timeout| timeout.as_secs()),
            reason: None,
            message: None,
        };
        if let Some(session) = session.as_ref() {
            return VaultStatus {
                state: VaultState::Unlocked,
                vault_id: Some(session.vault.vault_id().clone()),
                secrets: Some(session.vault.len()),
                has_recovery_key: Some(session.vault.has_recovery_key()),
                unlocked_by: Some(session.vault.unlock_method()),
                unlocked_at: Some(session.unlocked_at.clone()),
                ..base
            };
        }
        match Vault::open(&self.inner.dir) {
            Ok(locked) => VaultStatus {
                state: VaultState::Locked,
                vault_id: Some(locked.vault_id().clone()),
                has_recovery_key: Some(locked.has_recovery_key()),
                ..base
            },
            Err(VaultError::NotFound) => base,
            Err(error) => VaultStatus {
                state: VaultState::Unavailable,
                reason: Some(error.kind().as_str().to_owned()),
                message: Some(error.to_string()),
                ..base
            },
        }
    }

    /// Count now as use of the unlocked session, as a secret operation does: the
    /// idle auto-lock starts its period again. For work that needs the vault later
    /// without using it now (the daemon calls this while agent runs execute).
    /// Returns whether the vault is unlocked. It never unlocks and never reads the
    /// vault file, and a session already past its idle period still ends here.
    pub fn keep_alive(&self) -> bool {
        let idle = self.config().idle_timeout;
        let mut session = self.session();
        let now = Instant::now();
        self.inner.expire_if_idle(&mut session, idle, now);
        match session.as_mut() {
            Some(current) => {
                current.last_used = now;
                true
            }
            None => false,
        }
    }

    /// Create the vault and leave it unlocked (after every earlier session end has
    /// reached the lock observers, as for [`VaultService::unlock`]).
    pub fn init(
        &self,
        passphrase: &SecretString,
        recovery_key: bool,
    ) -> Result<InitOutcome, VaultError> {
        let kdf = self.config().kdf;
        let mut session = self.settled_session();
        if session.is_some() || Vault::exists(&self.inner.dir) {
            return Err(VaultError::AlreadyExists);
        }
        let (vault, recovery_key) = Vault::create(
            &self.inner.dir,
            passphrase,
            CreateOptions { kdf, recovery_key },
        )?;
        let vault_id = vault.vault_id().clone();
        *session = Some(Session::new(vault));
        Ok(InitOutcome {
            vault_id,
            recovery_key,
        })
    }

    /// Unlock with the passphrase (re-reading the header from disk). Unlocking an
    /// already unlocked vault re-verifies the passphrase and keeps the session; when
    /// `vault.json` changed on disk since that session loaded it, the session is
    /// replaced without a lock notice (the vault never locks).
    ///
    /// Every session that ended before this unlock has reached the lock observers
    /// before the new session begins, even one that ends on another thread while
    /// this call waits for the vault: a notice still queued is delivered on this
    /// thread, or awaited while the background thread delivers it. So an observer's
    /// cleanup never reaches the new session. An observer that unlocks the vault
    /// itself does not wait for the notice it is handling or those queued behind it.
    pub fn unlock(&self, passphrase: &SecretString) -> Result<VaultStatus, VaultError> {
        self.unlock_with(|locked| locked.unlock(passphrase))
    }

    pub fn unlock_with_recovery(&self, key: &RecoveryKey) -> Result<VaultStatus, VaultError> {
        self.unlock_with(|locked| locked.unlock_with_recovery(key))
    }

    fn unlock_with(
        &self,
        unlock: impl FnOnce(&crate::LockedVault) -> Result<UnlockedVault, VaultError>,
    ) -> Result<VaultStatus, VaultError> {
        {
            let mut session = self.settled_session();
            let locked = Vault::open(&self.inner.dir)?;
            let vault = unlock(&locked)?;
            match session.as_mut() {
                // Same committed state: keep the session (and its live handles).
                Some(current) if current.vault.header_digest() == vault.header_digest() => {
                    current.last_used = Instant::now();
                }
                _ => *session = Some(Session::new(vault)),
            }
        }
        Ok(self.status())
    }

    /// Lock and zeroize keys; every handle stops resolving. Lock observers have seen
    /// this and every earlier session end when it returns: they run on this thread,
    /// so do not call it while holding a lock an observer takes.
    pub fn lock(&self) -> LockOutcome {
        let revoked = {
            let mut session = self.session();
            self.inner.end_session(&mut session, LockReason::Requested)
        };
        self.inner.notices.deliver();
        LockOutcome {
            was_unlocked: revoked.is_some(),
            revoked_handles: revoked.unwrap_or(0),
        }
    }

    pub fn put_secret(
        &self,
        name: &SecretName,
        kind: SecretKind,
        value: SecretBytes,
        policy: SecretPolicy,
    ) -> Result<SecretMeta, VaultError> {
        self.with_vault(|vault| vault.put(name, kind, value, policy))
    }

    pub fn list_secrets(&self) -> Result<Vec<SecretMeta>, VaultError> {
        self.with_vault(|vault| Ok(vault.list()))
    }

    pub fn secret_meta(&self, name: &SecretName) -> Result<SecretMeta, VaultError> {
        self.with_vault(|vault| vault.meta(name))
    }

    /// Backend-only: the current record and revision of a secret (no value).
    pub fn secret_version(&self, name: &SecretName) -> Result<crate::SecretVersion, VaultError> {
        self.with_vault(|vault| vault.secret_version(name))
    }

    /// Broker-only: resolve a secret for `consumer` only while `expected` is still
    /// its current version (see [`UnlockedVault::resolve_versioned`]). The check,
    /// handle issue, resolution and revocation hold the vault lock together.
    pub fn resolve_versioned(
        &self,
        name: &SecretName,
        consumer: &ConsumerScope,
        expected: &crate::SecretVersion,
    ) -> Result<SecretBytes, VaultError> {
        self.with_vault(|vault| vault.resolve_versioned(name, consumer, expected))
    }

    pub fn delete_secret(&self, name: &SecretName) -> Result<SecretMeta, VaultError> {
        self.with_vault(|vault| vault.delete(name))
    }

    /// Change the passphrase. `old` may be omitted only when the vault was unlocked
    /// with the recovery key.
    pub fn change_passphrase(
        &self,
        old: Option<&SecretString>,
        new: &SecretString,
    ) -> Result<(), VaultError> {
        self.with_vault(|vault| match old {
            Some(old) => vault.change_passphrase(old, new),
            None => vault.reset_passphrase(new),
        })
    }

    /// Broker-only: issue a scoped handle (no RPC method exposes this).
    pub fn issue_handle(
        &self,
        name: &SecretName,
        consumer: &ConsumerScope,
        ttl: Duration,
    ) -> Result<SecretHandle, VaultError> {
        self.with_vault(|vault| vault.issue_handle(name, consumer, ttl))
    }

    /// Broker-only: resolve a handle for the consumer it is being delivered to.
    pub fn resolve(
        &self,
        handle: &SecretHandle,
        consumer: &ConsumerScope,
    ) -> Result<SecretBytes, VaultError> {
        self.with_vault(|vault| vault.resolve(handle, consumer))
    }

    /// Revoke one handle; `false` when it is unknown, already revoked, or the vault is locked.
    pub fn revoke_handle(&self, handle_id: &EntityId) -> bool {
        self.session()
            .as_mut()
            .is_some_and(|session| session.vault.revoke_handle(handle_id))
    }

    /// Revoke every handle without locking (emergency stop keeps the vault usable for
    /// the user but cuts off every consumer).
    pub fn revoke_all_handles(&self) -> usize {
        self.session()
            .as_mut()
            .map_or(0, |session| session.vault.revoke_all_handles())
    }

    /// Back up the unlocked vault to `dest` (encrypted, verified).
    pub fn backup_to(&self, dest: &Path) -> Result<BackupReport, VaultError> {
        self.with_vault(|vault| vault.backup_to(dest))
    }

    fn session(&self) -> MutexGuard<'_, Option<Session>> {
        self.inner.session()
    }

    /// The session lock, taken once every session end queued so far has been
    /// delivered to the lock observers, for a caller that may begin a new session.
    /// Ends are queued only under this lock, so none can slip in while the caller
    /// holds it; one that was queued while this waited for the lock is delivered
    /// (with the lock released, as observers may use the vault) before retrying.
    fn settled_session(&self) -> MutexGuard<'_, Option<Session>> {
        loop {
            self.inner.notices.deliver();
            #[cfg(test)]
            if let Some(hook) = tests::BEFORE_SESSION_LOCK.take() {
                hook();
            }
            let session = self.session();
            if self.inner.notices.settled() {
                return session;
            }
        }
    }

    /// Run `operation` on the unlocked vault, failing closed when it is not unlocked.
    fn with_vault<T>(
        &self,
        operation: impl FnOnce(&mut UnlockedVault) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let idle = self.config().idle_timeout;
        let mut session = self.session();
        let now = Instant::now();
        self.inner.expire_if_idle(&mut session, idle, now);
        let Some(current) = session.as_mut() else {
            return Err(match Vault::open(&self.inner.dir) {
                Ok(_) => VaultError::Locked,
                Err(error) => error,
            });
        };
        current.last_used = now;
        let result = operation(&mut current.vault);
        if let Some(reason) = result
            .as_ref()
            .err()
            .and_then(|error| self.ending_reason(error))
        {
            // External changes or an ambiguous local commit require a fresh unlock.
            self.inner.end_session(&mut session, reason);
        }
        result
    }
}

impl Session {
    fn new(vault: UnlockedVault) -> Self {
        Self {
            vault,
            unlocked_at: Timestamp::now(),
            last_used: Instant::now(),
        }
    }
}

impl Inner {
    /// A panic while the vault was in use locks it (fail closed) before continuing.
    fn session(&self) -> MutexGuard<'_, Option<Session>> {
        self.session.lock().unwrap_or_else(|poisoned| {
            self.session.clear_poison();
            let mut guard = poisoned.into_inner();
            self.end_session(&mut guard, LockReason::Panic);
            guard
        })
    }

    /// End the session (with its lock held) and queue its notice; returns the
    /// handles revoked, or `None` when the vault was not unlocked.
    fn end_session(&self, session: &mut Option<Session>, reason: LockReason) -> Option<usize> {
        let mut ended = session.take()?;
        let revoked_handles = ended.vault.revoke_all_handles();
        self.notices.push(LockNotice {
            vault_id: ended.vault.vault_id().clone(),
            reason,
            revoked_handles,
        });
        Some(revoked_handles)
    }

    fn expire_if_idle(&self, session: &mut Option<Session>, idle: Option<Duration>, now: Instant) {
        let expired = match (session.as_ref(), idle) {
            (Some(current), Some(timeout)) => {
                now.saturating_duration_since(current.last_used) >= timeout
            }
            _ => false,
        };
        if expired {
            self.end_session(session, LockReason::Idle);
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.notices.close();
    }
}

/// Lock idle sessions and deliver queued lock notices until the service is dropped.
/// Holds the service only while checking, never while observers run or it waits.
fn auto_lock_loop(inner: &Weak<Inner>, notices: &LockNotices) {
    loop {
        let tick = {
            let Some(inner) = inner.upgrade() else {
                // A session that ended just before the service was dropped is
                // still reported.
                notices.deliver();
                return;
            };
            let idle = lock_or_recover(&inner.config).idle_timeout;
            let mut session = inner.session();
            inner.expire_if_idle(&mut session, idle, Instant::now());
            idle.map_or(MAX_TICK, |timeout| (timeout / 4).clamp(MIN_TICK, MAX_TICK))
        };
        notices.deliver();
        notices.wait(tick);
    }
}

pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        poisoned.into_inner()
    })
}

impl fmt::Debug for VaultService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultService")
            .field("dir", &self.inner.dir)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsio::{WriteFailure, fail_next_write};
    use std::cell::RefCell;

    const PASSPHRASE: &str = "correct horse battery staple";

    thread_local! {
        /// Runs once on this thread inside `settled_session`, after the queued
        /// notices were delivered and before the session lock is taken: the window
        /// in which another thread can end the session.
        pub(super) static BEFORE_SESSION_LOCK: RefCell<Option<Box<dyn FnOnce()>>> =
            const { RefCell::new(None) };
    }

    fn passphrase(text: &str) -> SecretString {
        SecretString::from(text.to_owned())
    }

    fn service_with_secret() -> (
        tempfile::TempDir,
        VaultService,
        SecretName,
        ConsumerScope,
        SecretHandle,
    ) {
        let root = tempfile::tempdir().unwrap();
        let service = VaultService::open(
            root.path().join("vault"),
            VaultServiceConfig {
                kdf: KdfParams::new(8, 1, 1).unwrap(),
                idle_timeout: None,
            },
        )
        .unwrap();
        service.init(&passphrase(PASSPHRASE), false).unwrap();
        let name = SecretName::new("api-key").unwrap();
        let scope = ConsumerScope::provider("openai").unwrap();
        service
            .put_secret(
                &name,
                SecretKind::ApiKey,
                SecretBytes::from(b"original".to_vec()),
                SecretPolicy::new([scope.clone()]).unwrap(),
            )
            .unwrap();
        let handle = service
            .issue_handle(&name, &scope, Duration::from_secs(60))
            .unwrap();
        (root, service, name, scope, handle)
    }

    fn assert_uncertain(error: &VaultError) {
        assert!(matches!(error, VaultError::CommitUncertain { .. }));
        assert_eq!(error.kind(), VaultErrorKind::Io);
        assert_eq!(error.code(), aidex_core::ServiceErrorCode::Unavailable);
        assert!(std::error::Error::source(error).is_some());
    }

    #[test]
    fn ambiguous_delete_drops_session_and_stale_handles() {
        let (_root, service, name, scope, handle) = service_with_secret();
        fail_next_write(service.dir(), WriteFailure::AfterRename);
        assert_uncertain(&service.delete_secret(&name).unwrap_err());
        assert_eq!(service.status().state, VaultState::Locked);
        assert_eq!(
            service.list_secrets().unwrap_err().kind(),
            VaultErrorKind::Locked
        );
        assert_eq!(
            service.secret_meta(&name).unwrap_err().kind(),
            VaultErrorKind::Locked
        );
        assert_eq!(
            service.resolve(&handle, &scope).unwrap_err().kind(),
            VaultErrorKind::Locked
        );

        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        assert!(service.list_secrets().unwrap().is_empty());
        assert_eq!(
            service.secret_meta(&name).unwrap_err().kind(),
            VaultErrorKind::SecretNotFound
        );
        assert_eq!(
            service.resolve(&handle, &scope).unwrap_err().kind(),
            VaultErrorKind::HandleDenied
        );
    }

    #[test]
    fn ambiguous_policy_replacement_preserves_new_record_and_requires_unlock() {
        let (_root, service, name, scope, handle) = service_with_secret();
        let replacement_scope = ConsumerScope::provider("other").unwrap();
        fail_next_write(service.dir(), WriteFailure::AfterRename);
        let error = service
            .put_secret(
                &name,
                SecretKind::ApiKey,
                SecretBytes::from(b"replacement".to_vec()),
                SecretPolicy::new([replacement_scope.clone()]).unwrap(),
            )
            .unwrap_err();
        assert_uncertain(&error);
        assert_eq!(service.status().state, VaultState::Locked);
        assert!(service.resolve(&handle, &scope).is_err());
        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        assert_eq!(
            service
                .issue_handle(&name, &scope, Duration::from_secs(60))
                .unwrap_err()
                .kind(),
            VaultErrorKind::ConsumerNotAllowed
        );
        let replacement = service
            .issue_handle(&name, &replacement_scope, Duration::from_secs(60))
            .unwrap();
        assert_eq!(
            service
                .resolve(&replacement, &replacement_scope)
                .unwrap()
                .expose(),
            b"replacement"
        );
    }

    #[test]
    fn ambiguous_passphrase_change_requires_new_passphrase_on_fresh_unlock() {
        let (_root, service, _name, scope, handle) = service_with_secret();
        let new = passphrase("a different long passphrase");
        fail_next_write(service.dir(), WriteFailure::AfterRename);
        assert_uncertain(
            &service
                .change_passphrase(Some(&passphrase(PASSPHRASE)), &new)
                .unwrap_err(),
        );
        assert_eq!(service.status().state, VaultState::Locked);
        assert_eq!(
            service.unlock(&passphrase(PASSPHRASE)).unwrap_err().kind(),
            VaultErrorKind::WrongPassphrase
        );
        service.unlock(&new).unwrap();
        assert_eq!(
            service.resolve(&handle, &scope).unwrap_err().kind(),
            VaultErrorKind::HandleDenied
        );
    }

    #[test]
    fn pre_rename_io_error_keeps_original_session_and_committed_state() {
        let (_root, service, name, scope, handle) = service_with_secret();
        let header = std::fs::read(service.dir().join("vault.json")).unwrap();
        fail_next_write(service.dir(), WriteFailure::BeforeRename);
        let error = service.delete_secret(&name).unwrap_err();
        assert!(matches!(error, VaultError::Io { .. }));
        assert_eq!(error.kind(), VaultErrorKind::Io);
        assert!(!service.ends_session(&error));
        assert_eq!(service.status().state, VaultState::Unlocked);
        assert_eq!(
            std::fs::read(service.dir().join("vault.json")).unwrap(),
            header
        );
        assert_eq!(service.list_secrets().unwrap().len(), 1);
        assert_eq!(
            service.resolve(&handle, &scope).unwrap().expose(),
            b"original"
        );
        service.delete_secret(&name).unwrap();
        assert!(service.list_secrets().unwrap().is_empty());
    }

    #[test]
    fn direct_session_reconciles_index_and_revokes_handles_after_ambiguous_commit() {
        let (_root, service, name, scope, handle) = service_with_secret();
        // Retain a direct UnlockedVault to exercise the storage boundary independently
        // of the service's invalidation.
        service.lock();
        let mut vault = Vault::open(service.dir())
            .unwrap()
            .unlock(&passphrase(PASSPHRASE))
            .unwrap();
        let direct_handle = vault
            .issue_handle(&name, &scope, Duration::from_secs(60))
            .unwrap();
        fail_next_write(vault.dir(), WriteFailure::AfterRename);
        assert_uncertain(&vault.delete(&name).unwrap_err());
        assert!(vault.list().is_empty());
        assert!(vault.meta(&name).is_err());
        assert_eq!(vault.active_handles(), 0);
        assert!(vault.resolve(&direct_handle, &scope).is_err());
        assert!(vault.resolve(&handle, &scope).is_err());
        assert_eq!(vault.verify().unwrap().records, 0);
    }

    #[test]
    fn ambiguous_backup_commit_does_not_discard_unchanged_source_session() {
        let (root, service, _name, scope, handle) = service_with_secret();
        let destination = root.path().join("backup");
        // Avoid injecting the failure at initial records-directory creation.
        std::fs::create_dir_all(destination.join("records")).unwrap();
        fail_next_write(&destination, WriteFailure::AfterRename);
        let error = service.backup_to(&destination).unwrap_err();
        assert_uncertain(&error);
        assert!(!service.ends_session(&error));
        assert_eq!(service.status().state, VaultState::Unlocked);
        assert_eq!(
            service.resolve(&handle, &scope).unwrap().expose(),
            b"original"
        );
    }

    /// Every notice delivered so far; the observer holds no service reference.
    fn observe(service: &VaultService) -> Arc<Mutex<Vec<LockNotice>>> {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&notices);
        service.add_lock_observer(move |notice| seen.lock().unwrap().push(notice.clone()));
        notices
    }

    /// Wait (generously, for slow CI runners) until `count` notices arrived.
    fn wait_for<T: Clone + fmt::Debug>(seen: &Mutex<Vec<T>>, count: usize) -> Vec<T> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let current = seen.lock().unwrap().clone();
            if current.len() >= count {
                return current;
            }
            assert!(Instant::now() < deadline, "waited for {count}: {current:?}");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn every_session_end_is_observed_once_in_order() {
        let (_root, service, name, _scope, _handle) = service_with_secret();
        let vault_id = service.status().vault_id.unwrap();
        let notices = observe(&service);
        let notice = |reason, revoked_handles| LockNotice {
            vault_id: vault_id.clone(),
            reason,
            revoked_handles,
        };

        // An uncertain commit of the vault's own header (its handles were
        // already revoked by the commit) is delivered by the background thread.
        fail_next_write(service.dir(), WriteFailure::AfterRename);
        let error = service.delete_secret(&name).unwrap_err();
        assert!(service.ends_session(&error));
        assert_eq!(
            wait_for(&notices, 1),
            [notice(LockReason::CommitUncertain, 0)]
        );
        assert!(!service.lock().was_unlocked, "already locked: no notice");

        // An explicit lock is observed before `lock` returns.
        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        let scope = ConsumerScope::provider("openai").unwrap();
        service
            .put_secret(
                &name,
                SecretKind::ApiKey,
                SecretBytes::from(b"value".to_vec()),
                SecretPolicy::new([scope.clone()]).unwrap(),
            )
            .unwrap();
        service
            .issue_handle(&name, &scope, Duration::from_secs(60))
            .unwrap();
        assert_eq!(service.lock().revoked_handles, 1);
        assert_eq!(
            notices.lock().unwrap()[1..],
            [notice(LockReason::Requested, 1)]
        );

        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        service.set_idle_timeout(Some(Duration::from_millis(50)));
        assert_eq!(wait_for(&notices, 3)[2..], [notice(LockReason::Idle, 0)]);
        service.set_idle_timeout(None);

        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        let mut other = Vault::open(service.dir())
            .unwrap()
            .unlock(&passphrase(PASSPHRASE))
            .unwrap();
        other.delete(&name).unwrap();
        let error = service.delete_secret(&name).unwrap_err();
        assert_eq!(error.kind(), VaultErrorKind::Conflict);
        assert!(service.ends_session(&error));
        assert_eq!(
            wait_for(&notices, 4)[3..],
            [notice(LockReason::Conflict, 0)]
        );

        thread::sleep(Duration::from_millis(100));
        assert_eq!(notices.lock().unwrap().len(), 4, "each end is seen once");
    }

    #[test]
    fn observers_run_outside_the_vault_lock_and_may_use_the_vault() {
        let (_root, service, _name, _scope, _handle) = service_with_secret();
        let service = Arc::new(service);
        let weak = Arc::downgrade(&service);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        service.add_lock_observer(move |_| {
            // Both calls would deadlock if observers ran under the session lock
            // or waited for their own delivery.
            if let Some(service) = weak.upgrade() {
                let state = service.status().state;
                record
                    .lock()
                    .unwrap()
                    .push((state, service.lock().was_unlocked));
            }
        });
        service.lock();
        assert_eq!(*seen.lock().unwrap(), [(VaultState::Locked, false)]);

        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        service.set_idle_timeout(Some(Duration::from_millis(20)));
        assert_eq!(
            wait_for(&seen, 2),
            [(VaultState::Locked, false), (VaultState::Locked, false)]
        );
    }

    #[test]
    fn keep_alive_holds_off_the_idle_lock_only_while_it_is_called() {
        let (_root, service, _name, _scope, _handle) = service_with_secret();
        let notices = observe(&service);
        service.set_idle_timeout(Some(Duration::from_millis(600)));
        // Kept alive well past the idle period: the session stays.
        let until = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < until {
            assert!(service.keep_alive());
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(service.status().state, VaultState::Unlocked);
        assert!(notices.lock().unwrap().is_empty());
        // Once the calls stop, the idle lock happens as usual.
        assert_eq!(
            wait_for(&notices, 1)
                .iter()
                .map(|notice| notice.reason)
                .collect::<Vec<_>>(),
            [LockReason::Idle]
        );
        // A keep-alive never unlocks a locked vault.
        assert!(!service.keep_alive());
        assert_eq!(service.status().state, VaultState::Locked);

        // A session already past its idle period ends instead of being revived.
        service.set_idle_timeout(None);
        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        thread::sleep(Duration::from_millis(60));
        service.set_idle_timeout(Some(Duration::from_millis(50)));
        assert!(!service.keep_alive());
        assert_eq!(wait_for(&notices, 2)[1].reason, LockReason::Idle);
    }

    #[test]
    fn observers_are_released_with_the_service() {
        let (_root, service, _name, _scope, _handle) = service_with_secret();
        let marker = Arc::new(());
        let held = Arc::clone(&marker);
        service.add_lock_observer(move |_| {
            let _ = &held;
        });
        drop(service);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Arc::strong_count(&marker) > 1 {
            assert!(
                Instant::now() < deadline,
                "the observer outlived the service"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn unlock_delivers_a_session_end_from_another_thread_before_the_new_session() {
        let (_root, service, name, _scope, _handle) = service_with_secret();
        let service = Arc::new(service);
        let weak = Arc::downgrade(&service);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        service.add_lock_observer(move |notice| {
            // A slow cleanup: an unlock that did not wait for it would open the
            // next session, and return, before it ran. It must find the vault
            // locked, not the session that the unlock below opens next.
            thread::sleep(Duration::from_millis(200));
            let state = weak.upgrade().map(|service| service.status().state);
            record.lock().unwrap().push((notice.reason, state));
        });

        // After `unlock` delivered the queued notices but before it takes the
        // session lock, another thread ends the session with an uncertain commit.
        let other = Arc::clone(&service);
        BEFORE_SESSION_LOCK.set(Some(Box::new(move || {
            thread::spawn(move || {
                fail_next_write(other.dir(), WriteFailure::AfterRename);
                assert_uncertain(&other.delete_secret(&name).unwrap_err());
            })
            .join()
            .unwrap();
        })));
        service.unlock(&passphrase(PASSPHRASE)).unwrap();
        assert!(BEFORE_SESSION_LOCK.take().is_none(), "the hook ran");
        assert_eq!(
            *seen.lock().unwrap(),
            [(LockReason::CommitUncertain, Some(VaultState::Locked))],
            "delivered before unlock returned, while the vault was still locked"
        );
        assert_eq!(service.status().state, VaultState::Unlocked);
        thread::sleep(Duration::from_millis(100));
        assert_eq!(seen.lock().unwrap().len(), 1, "delivered once");
    }

    #[test]
    fn a_panic_while_the_vault_is_in_use_locks_it_and_is_observed() {
        let (_root, service, _name, scope, handle) = service_with_secret();
        let vault_id = service.status().vault_id.unwrap();
        let notices = observe(&service);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            service.with_vault(|_| -> Result<(), VaultError> {
                panic!("injected panic while the vault is in use")
            })
        }));
        assert!(panicked.is_err());
        // The next use of the poisoned session fails closed and queues the notice;
        // `lock` delivers it on this thread before returning.
        assert_eq!(service.status().state, VaultState::Locked);
        assert!(!service.lock().was_unlocked);
        assert_eq!(
            *notices.lock().unwrap(),
            [LockNotice {
                vault_id,
                reason: LockReason::Panic,
                revoked_handles: 1,
            }]
        );
        assert_eq!(
            service.resolve(&handle, &scope).unwrap_err().kind(),
            VaultErrorKind::Locked
        );
        thread::sleep(Duration::from_millis(100));
        assert_eq!(notices.lock().unwrap().len(), 1, "delivered once");
    }
}
