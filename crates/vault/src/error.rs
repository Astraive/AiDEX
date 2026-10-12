//! Vault errors with stable kinds. Messages never contain secret values,
//! passphrases, recovery keys, or key material.

use aidex_core::ServiceErrorCode;
use std::{fmt, io, path::PathBuf};

/// Which authenticated structure failed verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrityTarget {
    /// The header MAC (slots, vault id, timestamps, sealed index) did not verify.
    Header,
    /// The sealed secret index did not decrypt or does not belong to this vault.
    Index,
    /// A record file did not decrypt for the record it is referenced as.
    Record,
}

impl IntegrityTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Header => "vault header",
            Self::Index => "secret index",
            Self::Record => "secret record",
        }
    }
}

/// Why a secret handle was refused. Every denial fails closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandleDenial {
    /// No handle with this id was issued by the current unlock session.
    Unknown,
    /// The presented handle does not match what was issued (modified or forged).
    Mismatch,
    /// The handle was revoked.
    Revoked,
    /// The handle's lifetime ended.
    Expired,
    /// The presenting consumer is not the consumer the handle was issued to.
    ConsumerMismatch,
    /// The secret was deleted after the handle was issued.
    SecretDeleted,
    /// The secret's value was replaced (or the secret re-created) since the
    /// revision the caller was bound to.
    RevisionChanged,
    /// The secret's policy no longer allows the handle's consumer.
    PolicyRevoked,
}

impl HandleDenial {
    /// Stable snake_case name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown_handle",
            Self::Mismatch => "handle_mismatch",
            Self::Revoked => "handle_revoked",
            Self::Expired => "handle_expired",
            Self::ConsumerMismatch => "consumer_mismatch",
            Self::SecretDeleted => "secret_deleted",
            Self::RevisionChanged => "secret_revision_changed",
            Self::PolicyRevoked => "policy_revoked",
        }
    }
}

/// Stable, transport-safe error categories (`as_str` is the wire name).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum VaultErrorKind {
    NotFound,
    AlreadyExists,
    MissingHeader,
    Corrupt,
    Tampered,
    UnsupportedVersion,
    WrongPassphrase,
    WrongRecoveryKey,
    NoRecoveryKey,
    InvalidRecoveryKey,
    Locked,
    Conflict,
    InvalidArgument,
    ForbiddenConsumer,
    SecretNotFound,
    ConsumerNotAllowed,
    HandleDenied,
    LimitExceeded,
    PassphraseRequired,
    UnsafePath,
    Io,
    Unavailable,
}

impl VaultErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "vault_not_found",
            Self::AlreadyExists => "vault_exists",
            Self::MissingHeader => "missing_header",
            Self::Corrupt => "corrupt",
            Self::Tampered => "tampered",
            Self::UnsupportedVersion => "unsupported_version",
            Self::WrongPassphrase => "wrong_passphrase",
            Self::WrongRecoveryKey => "wrong_recovery_key",
            Self::NoRecoveryKey => "no_recovery_key",
            Self::InvalidRecoveryKey => "invalid_recovery_key",
            Self::Locked => "locked",
            Self::Conflict => "conflict",
            Self::InvalidArgument => "invalid_argument",
            Self::ForbiddenConsumer => "forbidden_consumer",
            Self::SecretNotFound => "secret_not_found",
            Self::ConsumerNotAllowed => "consumer_not_allowed",
            Self::HandleDenied => "handle_denied",
            Self::LimitExceeded => "limit_exceeded",
            Self::PassphraseRequired => "passphrase_required",
            Self::UnsafePath => "unsafe_path",
            Self::Io => "io",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug)]
pub enum VaultError {
    /// No vault exists in the directory.
    NotFound,
    /// The target already holds vault data; nothing is overwritten.
    AlreadyExists,
    /// Record files exist but `vault.json` is missing; restore it from a backup.
    MissingHeader,
    /// `vault.json` is unreadable, truncated, or structurally invalid.
    CorruptHeader(String),
    /// A record file is missing, truncated, or structurally invalid.
    CorruptRecord(String),
    /// Authenticated decryption or MAC verification failed: tampering or corruption.
    Integrity(IntegrityTarget),
    /// The on-disk format version is not supported by this build.
    UnsupportedVersion {
        found: u64,
    },
    /// The passphrase does not unwrap the vault key (or the passphrase slot is damaged).
    WrongPassphrase,
    /// The recovery key does not unwrap the vault key (or the recovery slot is damaged).
    WrongRecoveryKey,
    /// The vault was created without a recovery key slot.
    NoRecoveryKey,
    /// The recovery key text is malformed or fails its checksum.
    InvalidRecoveryKey(&'static str),
    /// The operation needs an unlocked vault.
    Locked,
    /// `vault.json` changed on disk since it was loaded; unlock again.
    Conflict,
    InvalidPassphrase(&'static str),
    InvalidName(String),
    InvalidKind(String),
    InvalidConsumer(String),
    /// Model prompts, agents, sessions, logs, and shell environments never receive secrets.
    ForbiddenConsumer(String),
    InvalidKdfParams(String),
    InvalidSecret(&'static str),
    InvalidTtl(&'static str),
    SecretNotFound(String),
    ConsumerNotAllowed {
        name: String,
        consumer: String,
    },
    HandleDenied(HandleDenial),
    LimitExceeded(&'static str),
    /// Resetting the passphrase without the old one requires a recovery-key unlock.
    PassphraseRequired,
    /// A vault path is a symbolic link, junction, or unexpected file type.
    UnsafePath(PathBuf),
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// A header was renamed into place, but its directory could not be synced.
    /// The write may be committed; re-unlock before using a service session again.
    CommitUncertain {
        path: PathBuf,
        source: io::Error,
    },
    /// The operating-system RNG failed; no weaker randomness is substituted.
    Randomness(String),
    /// Key derivation could not run (e.g. invalid parameters for this platform).
    Kdf(String),
    /// A cryptographic primitive rejected its input (not expected in practice).
    Crypto(&'static str),
}

impl VaultError {
    pub fn kind(&self) -> VaultErrorKind {
        match self {
            Self::NotFound => VaultErrorKind::NotFound,
            Self::AlreadyExists => VaultErrorKind::AlreadyExists,
            Self::MissingHeader => VaultErrorKind::MissingHeader,
            Self::CorruptHeader(_) | Self::CorruptRecord(_) => VaultErrorKind::Corrupt,
            Self::Integrity(_) => VaultErrorKind::Tampered,
            Self::UnsupportedVersion { .. } => VaultErrorKind::UnsupportedVersion,
            Self::WrongPassphrase => VaultErrorKind::WrongPassphrase,
            Self::WrongRecoveryKey => VaultErrorKind::WrongRecoveryKey,
            Self::NoRecoveryKey => VaultErrorKind::NoRecoveryKey,
            Self::InvalidRecoveryKey(_) => VaultErrorKind::InvalidRecoveryKey,
            Self::Locked => VaultErrorKind::Locked,
            Self::Conflict => VaultErrorKind::Conflict,
            Self::InvalidPassphrase(_)
            | Self::InvalidName(_)
            | Self::InvalidKind(_)
            | Self::InvalidConsumer(_)
            | Self::InvalidKdfParams(_)
            | Self::InvalidSecret(_)
            | Self::InvalidTtl(_) => VaultErrorKind::InvalidArgument,
            Self::ForbiddenConsumer(_) => VaultErrorKind::ForbiddenConsumer,
            Self::SecretNotFound(_) => VaultErrorKind::SecretNotFound,
            Self::ConsumerNotAllowed { .. } => VaultErrorKind::ConsumerNotAllowed,
            Self::HandleDenied(_) => VaultErrorKind::HandleDenied,
            Self::LimitExceeded(_) => VaultErrorKind::LimitExceeded,
            Self::PassphraseRequired => VaultErrorKind::PassphraseRequired,
            Self::UnsafePath(_) => VaultErrorKind::UnsafePath,
            Self::Io { .. } | Self::CommitUncertain { .. } => VaultErrorKind::Io,
            Self::Randomness(_) | Self::Kdf(_) | Self::Crypto(_) => VaultErrorKind::Unavailable,
        }
    }

    /// The cross-service category used for API mapping.
    pub fn code(&self) -> ServiceErrorCode {
        match self.kind() {
            VaultErrorKind::NotFound | VaultErrorKind::SecretNotFound => ServiceErrorCode::NotFound,
            VaultErrorKind::AlreadyExists | VaultErrorKind::Conflict => ServiceErrorCode::Conflict,
            VaultErrorKind::UnsupportedVersion => ServiceErrorCode::IncompatibleVersion,
            VaultErrorKind::WrongPassphrase
            | VaultErrorKind::WrongRecoveryKey
            | VaultErrorKind::Locked
            | VaultErrorKind::ForbiddenConsumer
            | VaultErrorKind::ConsumerNotAllowed
            | VaultErrorKind::HandleDenied
            | VaultErrorKind::PassphraseRequired
            | VaultErrorKind::UnsafePath => ServiceErrorCode::PermissionDenied,
            VaultErrorKind::InvalidArgument
            | VaultErrorKind::InvalidRecoveryKey
            | VaultErrorKind::NoRecoveryKey
            | VaultErrorKind::LimitExceeded => ServiceErrorCode::InvalidArgument,
            VaultErrorKind::MissingHeader | VaultErrorKind::Corrupt | VaultErrorKind::Tampered => {
                ServiceErrorCode::Internal
            }
            VaultErrorKind::Io | VaultErrorKind::Unavailable => ServiceErrorCode::Unavailable,
        }
    }

    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("no vault exists on this drive"),
            Self::AlreadyExists => f.write_str("a vault already exists here; nothing was changed"),
            Self::MissingHeader => f.write_str(
                "vault records exist but the vault header (vault.json) is missing; restore it from a backup",
            ),
            Self::CorruptHeader(detail) => write!(f, "vault header is corrupt: {detail}"),
            Self::CorruptRecord(detail) => write!(f, "vault record is corrupt: {detail}"),
            Self::Integrity(target) => write!(
                f,
                "{} failed authentication; the vault was modified or corrupted",
                target.as_str()
            ),
            Self::UnsupportedVersion { found } => write!(
                f,
                "vault format version {found} is not supported by this build (supported: {})",
                crate::FORMAT_VERSION
            ),
            Self::WrongPassphrase => f.write_str("wrong passphrase"),
            Self::WrongRecoveryKey => f.write_str("wrong recovery key"),
            Self::NoRecoveryKey => f.write_str("this vault has no recovery key"),
            Self::InvalidRecoveryKey(reason) => write!(f, "invalid recovery key: {reason}"),
            Self::Locked => f.write_str("the vault is locked"),
            Self::Conflict => {
                f.write_str("the vault changed on disk since it was unlocked; unlock it again")
            }
            Self::InvalidPassphrase(reason) => write!(f, "invalid passphrase: {reason}"),
            Self::InvalidName(reason) => write!(f, "invalid secret name: {reason}"),
            Self::InvalidKind(kind) => write!(f, "unknown secret kind `{kind}`"),
            Self::InvalidConsumer(reason) => write!(f, "invalid consumer scope: {reason}"),
            Self::ForbiddenConsumer(scope) => write!(
                f,
                "consumer `{scope}` can never receive secrets (model prompts, agents, sessions, logs, and shell environments are excluded)"
            ),
            Self::InvalidKdfParams(reason) => write!(f, "invalid key-derivation parameters: {reason}"),
            Self::InvalidSecret(reason) => write!(f, "invalid secret value: {reason}"),
            Self::InvalidTtl(reason) => write!(f, "invalid handle lifetime: {reason}"),
            Self::SecretNotFound(name) => write!(f, "secret `{name}` does not exist"),
            Self::ConsumerNotAllowed { name, consumer } => write!(
                f,
                "secret `{name}` is not available to consumer `{consumer}`"
            ),
            Self::HandleDenied(reason) => write!(f, "secret handle denied: {}", reason.as_str()),
            Self::LimitExceeded(what) => write!(f, "limit exceeded: {what}"),
            Self::PassphraseRequired => f.write_str(
                "the current passphrase is required (only a recovery-key unlock may reset it)",
            ),
            Self::UnsafePath(path) => write!(
                f,
                "refusing vault path `{}`: it is a link or not a regular file/directory",
                path.display()
            ),
            Self::Io {
                action,
                path,
                source,
            } => write!(f, "cannot {action} `{}`: {source}", path.display()),
            Self::CommitUncertain { path, source } => write!(
                f,
                "vault header `{}` was replaced but durability is uncertain; unlock again: {source}",
                path.display()
            ),
            Self::Randomness(reason) => {
                write!(f, "operating-system randomness is unavailable: {reason}")
            }
            Self::Kdf(reason) => write!(f, "key derivation failed: {reason}"),
            Self::Crypto(what) => write!(f, "cryptographic operation failed: {what}"),
        }
    }
}

impl std::error::Error for VaultError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::CommitUncertain { source, .. } => Some(source),
            _ => None,
        }
    }
}
