//! Key-encryption-key derivation: Argon2id for passphrases, HKDF-SHA256 for the
//! high-entropy recovery key. Parameters live in the vault header.

use crate::{
    RecoveryKey, SecretString, VaultError,
    crypto::{KEY_LEN, Key, SALT_LEN, hkdf_expand},
};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

/// Argon2 version 0x13 (decimal 19), recorded in the header for explicitness.
pub(crate) const ARGON2_VERSION: u32 = 0x13;
/// Minimum passphrase length in Unicode scalar values.
pub const MIN_PASSPHRASE_CHARS: usize = 8;
/// Maximum passphrase length in UTF-8 bytes.
pub const MAX_PASSPHRASE_BYTES: usize = 1024;

const RECOVERY_KEK_INFO: &[u8] = b"aidex/vault/kek/recovery/v1";

/// Argon2id cost parameters for a passphrase key slot.
///
/// The default (64 MiB, 3 passes, 1 lane) targets roughly a quarter to half a second on
/// a current laptop in an optimized build; see `FORMAT.md` for the rationale. Vaults
/// keep the parameters they were created with; headers outside the accepted bounds
/// are rejected so a modified header cannot force huge allocations.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct KdfParams {
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

impl KdfParams {
    pub const DEFAULT_MEMORY_KIB: u32 = 64 * 1024;
    pub const DEFAULT_ITERATIONS: u32 = 3;
    pub const DEFAULT_PARALLELISM: u32 = 1;
    /// 64 MiB: cap memory before the unauthenticated header can be verified.
    pub const MAX_MEMORY_KIB: u32 = Self::DEFAULT_MEMORY_KIB;
    /// At most the default three passes over that memory.
    pub const MAX_ITERATIONS: u32 = Self::DEFAULT_ITERATIONS;
    pub const MAX_PARALLELISM: u32 = 4;

    /// Validate Argon2id parameters. Values far below the default are accepted (tests
    /// and constrained devices) but weaken offline brute-force resistance.
    pub fn new(memory_kib: u32, iterations: u32, parallelism: u32) -> Result<Self, VaultError> {
        if !(1..=Self::MAX_PARALLELISM).contains(&parallelism) {
            return Err(VaultError::InvalidKdfParams(format!(
                "parallelism must be 1 to {}",
                Self::MAX_PARALLELISM
            )));
        }
        if !(1..=Self::MAX_ITERATIONS).contains(&iterations) {
            return Err(VaultError::InvalidKdfParams(format!(
                "iterations must be 1 to {}",
                Self::MAX_ITERATIONS
            )));
        }
        if memory_kib < 8 * parallelism || memory_kib > Self::MAX_MEMORY_KIB {
            return Err(VaultError::InvalidKdfParams(format!(
                "memory must be 8 KiB per lane up to {} KiB",
                Self::MAX_MEMORY_KIB
            )));
        }
        Ok(Self {
            memory_kib,
            iterations,
            parallelism,
        })
    }

    pub fn memory_kib(&self) -> u32 {
        self.memory_kib
    }

    pub fn iterations(&self) -> u32 {
        self.iterations
    }

    pub fn parallelism(&self) -> u32 {
        self.parallelism
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self {
            memory_kib: Self::DEFAULT_MEMORY_KIB,
            iterations: Self::DEFAULT_ITERATIONS,
            parallelism: Self::DEFAULT_PARALLELISM,
        }
    }
}

/// Reject passphrases that are too short, too long, or contain control characters.
///
/// Passphrases are used as their exact UTF-8 bytes; no Unicode normalization is applied.
pub(crate) fn validate_passphrase(passphrase: &SecretString) -> Result<(), VaultError> {
    let text = passphrase.expose();
    if text.len() > MAX_PASSPHRASE_BYTES {
        return Err(VaultError::InvalidPassphrase(
            "must be at most 1024 bytes of UTF-8",
        ));
    }
    if text.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(VaultError::InvalidPassphrase(
            "must be at least 8 characters",
        ));
    }
    if text.chars().any(char::is_control) {
        return Err(VaultError::InvalidPassphrase(
            "must not contain control characters such as newlines",
        ));
    }
    Ok(())
}

/// Argon2id(passphrase, salt, params) → 256-bit key-encryption key.
pub(crate) fn passphrase_kek(
    passphrase: &SecretString,
    salt: &[u8; SALT_LEN],
    params: &KdfParams,
) -> Result<Key, VaultError> {
    let argon_params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(KEY_LEN),
    )
    .map_err(|error| VaultError::Kdf(error.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut output = Zeroizing::new([0_u8; KEY_LEN]);
    argon2
        .hash_password_into(passphrase.expose().as_bytes(), salt, output.as_mut())
        .map_err(|error| VaultError::Kdf(error.to_string()))?;
    Ok(Key::from_bytes(output))
}

/// HKDF-SHA256(salt, recovery key) → 256-bit key-encryption key. The recovery key is
/// 256 bits of OS randomness, so a memory-hard KDF adds nothing.
pub(crate) fn recovery_kek(key: &RecoveryKey, salt: &[u8; SALT_LEN]) -> Result<Key, VaultError> {
    hkdf_expand(Some(salt), key.bytes(), &[RECOVERY_KEK_INFO])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VaultErrorKind;

    fn tiny() -> KdfParams {
        KdfParams::new(8, 1, 1).unwrap()
    }

    #[test]
    fn default_parameters_are_the_documented_production_values() {
        let params = KdfParams::default();
        assert_eq!(
            (
                params.memory_kib(),
                params.iterations(),
                params.parallelism()
            ),
            (65_536, 3, 1)
        );
    }

    #[test]
    fn parameters_outside_bounds_are_rejected() {
        for (m, t, p) in [
            (7, 1, 1),
            (16, 1, 4),
            (KdfParams::MAX_MEMORY_KIB + 1, 1, 1),
            (8, 0, 1),
            (8, KdfParams::MAX_ITERATIONS + 1, 1),
            (8, 1, 0),
            (1024, 1, KdfParams::MAX_PARALLELISM + 1),
        ] {
            assert_eq!(
                KdfParams::new(m, t, p).unwrap_err().kind(),
                VaultErrorKind::InvalidArgument,
                "{m} {t} {p}"
            );
        }
    }

    #[test]
    fn passphrase_derivation_depends_on_passphrase_salt_and_params() {
        let salt = [7_u8; SALT_LEN];
        let passphrase = SecretString::from("correct horse battery".to_owned());
        let a = passphrase_kek(&passphrase, &salt, &tiny()).unwrap();
        let again = passphrase_kek(&passphrase, &salt, &tiny()).unwrap();
        assert!(a.ct_eq(&again));
        let other_salt = passphrase_kek(&passphrase, &[8_u8; SALT_LEN], &tiny()).unwrap();
        let other_params =
            passphrase_kek(&passphrase, &salt, &KdfParams::new(16, 1, 1).unwrap()).unwrap();
        let other_text = SecretString::from("correct horse battery!".to_owned());
        let other_pass = passphrase_kek(&other_text, &salt, &tiny()).unwrap();
        for different in [other_salt, other_params, other_pass] {
            assert!(!a.ct_eq(&different));
        }
    }

    #[test]
    fn weak_or_malformed_passphrases_are_rejected() {
        for text in ["short", "seven77", "line\nbreak here", &"x".repeat(1025)] {
            let error = validate_passphrase(&SecretString::from(text.to_owned())).unwrap_err();
            assert_eq!(error.kind(), VaultErrorKind::InvalidArgument);
        }
        validate_passphrase(&SecretString::from("eight888".to_owned())).unwrap();
        validate_passphrase(&SecretString::from("pässwörter".to_owned())).unwrap();
    }
}
