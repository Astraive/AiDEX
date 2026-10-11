//! `VaultService`: the thread-safe, daemon-owned vault with idle auto-lock.
//!
//! The daemon exposes user operations (init/unlock/lock/set/list/delete/passphrase)
//! over IPC. Handle issuance and resolution are Rust-only methods for the broker; they
//! are deliberately not RPC methods. While sealed, state is re-read from disk on every
//! call, so external restores or damage are reported instead of a stale cache.

use crate::{
    BackupReport, ConsumerScope, CreateOptions, KdfParams, RecoveryKey, SecretBytes, SecretHandle,
    SecretKind, SecretMeta, SecretName, SecretPolicy, SecretString, UnlockMethod, UnlockedVault,
    Vault, VaultError, VaultErrorKind,
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
}

/// The daemon's vault: `Arc`-shareable, all methods take `&self`.
pub struct VaultService {
    inner: Arc<Inner>,
}

impl VaultService {
    /// Serve the vault stored in `dir`. Never fails because of vault contents (a
    /// damaged vault is reported by [`VaultService::status`]); starts the auto-lock thread.
    pub fn open(dir: impl Into<PathBuf>, config: VaultServiceConfig) -> Result<Self, VaultError> {
        let dir = dir.into();
        let inner = Arc::new(Inner {
            dir: dir.clone(),
            config: Mutex::new(config),
            session: Mutex::new(None),
        });
        let weak = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("aidex-vault-autolock".into())
            .spawn(move || auto_lock_loop(&weak))
            .map_err(|error| VaultError::io("start the auto-lock thread for vault", dir, error))?;
        Ok(Self { inner })
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
        expire_if_idle(&mut session, idle, Instant::now());
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

    /// Create the vault and leave it unlocked.
    pub fn init(
        &self,
        passphrase: &SecretString,
        recovery_key: bool,
    ) -> Result<InitOutcome, VaultError> {
        let kdf = self.config().kdf;
        let mut session = self.session();
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
    /// already unlocked vault re-verifies the passphrase and keeps the session.
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
            let mut session = self.session();
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

    /// Lock and zeroize keys; every handle stops resolving.
    pub fn lock(&self) -> LockOutcome {
        let mut session = self.session();
        match session.take() {
            Some(mut current) => LockOutcome {
                was_unlocked: true,
                revoked_handles: current.vault.revoke_all_handles(),
            },
            None => LockOutcome {
                was_unlocked: false,
                revoked_handles: 0,
            },
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

    /// Backend-only metadata; no credential value or handle is exposed.
    pub fn secret_version(&self, name: &SecretName) -> Result<crate::SecretVersion, VaultError> {
        self.with_vault(|vault| vault.secret_version(name))
    }

    /// Atomically check an approved revision, then resolve and revoke its scoped handle.
    pub fn resolve_versioned(
        &self,
        name: &SecretName,
        consumer: &ConsumerScope,
        expected: &crate::SecretVersion,
    ) -> Result<SecretBytes, VaultError> {
        self.with_vault(|vault| {
            if &vault.secret_version(name)? != expected {
                return Err(VaultError::HandleDenied(
                    crate::HandleDenial::RevisionChanged,
                ));
            }
            let handle = vault.issue_handle(name, consumer, Duration::from_secs(30))?;
            let result = vault.resolve(&handle, consumer);
            vault.revoke_handle(&handle.handle_id);
            result
        })
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
        lock_or_recover_session(&self.inner.session)
    }

    /// Run `operation` on the unlocked vault, failing closed when it is not unlocked.
    fn with_vault<T>(
        &self,
        operation: impl FnOnce(&mut UnlockedVault) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let idle = self.config().idle_timeout;
        let mut session = self.session();
        let now = Instant::now();
        expire_if_idle(&mut session, idle, now);
        let Some(current) = session.as_mut() else {
            return Err(match Vault::open(&self.inner.dir) {
                Ok(_) => VaultError::Locked,
                Err(error) => error,
            });
        };
        current.last_used = now;
        let result = operation(&mut current.vault);
        if result
            .as_ref()
            .is_err_and(|error| error.kind() == VaultErrorKind::Conflict)
        {
            // The on-disk vault changed underneath this session; require a fresh unlock.
            *session = None;
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

fn expire_if_idle(session: &mut Option<Session>, idle: Option<Duration>, now: Instant) {
    let expired = match (session.as_ref(), idle) {
        (Some(current), Some(timeout)) => {
            now.saturating_duration_since(current.last_used) >= timeout
        }
        _ => false,
    };
    if expired {
        *session = None;
    }
}

fn auto_lock_loop(inner: &Weak<Inner>) {
    loop {
        let tick = {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let idle = lock_or_recover(&inner.config).idle_timeout;
            let mut session = lock_or_recover_session(&inner.session);
            expire_if_idle(&mut session, idle, Instant::now());
            idle.map_or(MAX_TICK, |timeout| (timeout / 4).clamp(MIN_TICK, MAX_TICK))
        };
        thread::sleep(tick);
    }
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        poisoned.into_inner()
    })
}

/// A panic while the vault was in use locks it (fail closed) before continuing.
fn lock_or_recover_session(mutex: &Mutex<Option<Session>>) -> MutexGuard<'_, Option<Session>> {
    mutex.lock().unwrap_or_else(|poisoned| {
        mutex.clear_poison();
        let mut guard = poisoned.into_inner();
        *guard = None;
        guard
    })
}

impl fmt::Debug for VaultService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultService")
            .field("dir", &self.inner.dir)
            .finish_non_exhaustive()
    }
}
