//! `aidex-vault`: the portable, passphrase-encrypted AiDEX secret vault and its
//! scoped secret handles (spec §15, §92).
//!
//! Key hierarchy: passphrase → Argon2id → key-encryption key → unwraps a random
//! 256-bit vault master key (an optional recovery key wraps the same master key);
//! per-record keys are HKDF-SHA256 subkeys; records and the secret index are
//! XChaCha20-Poly1305 sealed with associated data that binds them to their vault,
//! record, revision, and file. See `FORMAT.md` for the format and threat model.
//!
//! Secret values leave the vault only through [`UnlockedVault::resolve`] (or
//! [`VaultService::resolve`]) for a handle issued to an allowed [`ConsumerScope`].
//! There is no export API.

mod backup;
mod crypto;
mod error;
mod format;
mod fsio;
mod handles;
mod kdf;
mod model;
mod observer;
mod recovery;
mod scope;
mod secret;
mod service;
mod unlocked;
mod vault;

pub use backup::BackupReport;
pub use error::{HandleDenial, IntegrityTarget, VaultError, VaultErrorKind};
pub use format::FORMAT_VERSION;
pub use handles::{MAX_HANDLE_TTL, MAX_HANDLES, SecretHandle};
pub use kdf::{KdfParams, MAX_PASSPHRASE_BYTES, MIN_PASSPHRASE_CHARS};
pub use model::{
    MAX_CONSUMERS, MAX_NAME_LEN, SecretKind, SecretMeta, SecretName, SecretPolicy, SecretVersion,
};
pub use observer::{LockNotice, LockReason};
pub use recovery::RecoveryKey;
pub use scope::{ConsumerKind, ConsumerScope};
pub use secret::{SecretBytes, SecretString};
pub use service::{
    DEFAULT_IDLE_TIMEOUT, InitOutcome, LockOutcome, VaultService, VaultServiceConfig, VaultState,
    VaultStatus,
};
pub use unlocked::{MAX_RECORD_FILES, MAX_SECRET_BYTES, MAX_SECRETS, UnlockedVault, VerifyReport};
pub use vault::{CreateOptions, LockedVault, UnlockMethod, Vault};

/// Filesystem failure injection for the tests of crates that embed the vault. Only
/// with the `test-hooks` feature, which only dev-dependencies enable.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub mod testing {
    pub use crate::fsio::{WriteFailure, fail_next_write};
}
