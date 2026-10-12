//! Creating, opening, and unlocking a vault directory.

use crate::{
    KdfParams, RecoveryKey, SecretString, UnlockedVault, VaultError,
    crypto::{Key, MAC_LEN, mac, open, random_array, seal, sha256, verify_mac},
    error::IntegrityTarget,
    format::{
        header::{Header, Slot, SlotKdf, SlotKind},
        index::{INDEX_KEY_INFO, Index},
    },
    fsio::VaultDir,
    kdf::{passphrase_kek, recovery_kek, validate_passphrase},
};
use aidex_core::{EntityId, Timestamp};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path};

const MAC_KEY_INFO: &[u8] = b"aidex/vault/header-mac/v1";
/// Generation of the empty index written at creation.
const FIRST_GENERATION: u64 = 1;

/// Entry point for vault directories.
pub struct Vault;

/// Options for [`Vault::create`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CreateOptions {
    /// Argon2id cost for the passphrase slot (default: 64 MiB, t=3, p=1).
    pub kdf: KdfParams,
    /// Also create a recovery-key slot and return the key once.
    pub recovery_key: bool,
}

/// How an [`UnlockedVault`] was unlocked.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnlockMethod {
    Passphrase,
    RecoveryKey,
}

impl Vault {
    /// Whether `dir` contains a vault header (it may still be invalid; see [`Vault::open`]).
    pub fn exists(dir: &Path) -> bool {
        VaultDir::existing(dir)
            .ok()
            .is_some_and(|dir| std::fs::symlink_metadata(dir.header_path()).is_ok())
    }

    /// Create a new vault in `dir` (created if missing; its parent must exist).
    ///
    /// Refuses a directory that already holds a header or record files. The recovery
    /// key, when requested, is returned exactly once and is never stored in plaintext.
    pub fn create(
        dir: &Path,
        passphrase: &SecretString,
        options: CreateOptions,
    ) -> Result<(UnlockedVault, Option<RecoveryKey>), VaultError> {
        validate_passphrase(passphrase)?;
        let dir = VaultDir::create_or_existing(dir)?;
        if dir.has_vault_data()? {
            return Err(VaultError::AlreadyExists);
        }
        let vault_id = EntityId::generate("vault")
            .map_err(|error| VaultError::Randomness(error.to_string()))?;
        let created_at = Timestamp::now();
        let master = Key::random()?;
        let keys = VaultKeys::derive(master)?;

        let passphrase_salt = random_array()?;
        let passphrase_slot = wrap_slot(
            &vault_id,
            SlotKind::Passphrase,
            SlotKdf::Argon2id(options.kdf),
            passphrase_salt,
            &passphrase_kek(passphrase, &passphrase_salt, &options.kdf)?,
            &keys.master,
        )?;
        let (recovery, recovery_slot) = if options.recovery_key {
            let key = RecoveryKey::generate()?;
            let salt = random_array()?;
            let slot = wrap_slot(
                &vault_id,
                SlotKind::Recovery,
                SlotKdf::HkdfSha256,
                salt,
                &recovery_kek(&key, &salt)?,
                &keys.master,
            )?;
            (Some(key), Some(slot))
        } else {
            (None, None)
        };

        let index = Index::empty(vault_id.clone(), FIRST_GENERATION);
        let mut header = Header {
            vault_id,
            created_at,
            passphrase: passphrase_slot,
            recovery: recovery_slot,
            index: index.seal(&keys.index)?,
            mac: [0; MAC_LEN],
        };
        header.mac = mac(&keys.mac, &header.mac_input())?;
        let bytes = header.to_bytes()?;
        dir.write_header(&bytes)?;
        let unlocked = UnlockedVault::new(
            dir,
            header,
            sha256(&bytes),
            keys,
            index,
            UnlockMethod::Passphrase,
        );
        Ok((unlocked, recovery))
    }

    /// Open an existing vault without unlocking it.
    ///
    /// A missing header is [`VaultError::NotFound`] when the directory holds no records,
    /// and [`VaultError::MissingHeader`] when it does (never an empty vault).
    pub fn open(dir: &Path) -> Result<LockedVault, VaultError> {
        let dir = VaultDir::existing(dir)?;
        match dir.read_header()? {
            Some(bytes) => {
                let header = Header::parse(&bytes)?;
                Ok(LockedVault {
                    dir,
                    header,
                    digest: sha256(&bytes),
                })
            }
            None if dir.record_blobs(None)?.is_empty() => Err(VaultError::NotFound),
            None => Err(VaultError::MissingHeader),
        }
    }
}

/// Wrap the master key under `kek` for one key slot.
pub(crate) fn wrap_slot(
    vault_id: &EntityId,
    kind: SlotKind,
    kdf: SlotKdf,
    salt: [u8; crate::crypto::SALT_LEN],
    kek: &Key,
    master: &Key,
) -> Result<Slot, VaultError> {
    let aad = Header::slot_aad(vault_id, kind, &kdf, &salt);
    let (nonce, wrapped_key) = seal(kek, &aad, master.as_bytes())?;
    Ok(Slot {
        kind,
        kdf,
        salt,
        nonce,
        wrapped_key,
    })
}

/// Unwrap the master key from `slot`; `None` when `kek` is wrong or the slot is damaged.
pub(crate) fn unwrap_slot(vault_id: &EntityId, slot: &Slot, kek: &Key) -> Option<Key> {
    let aad = Header::slot_aad(vault_id, slot.kind, &slot.kdf, &slot.salt);
    let plaintext = open(kek, &slot.nonce, &aad, &slot.wrapped_key)?;
    Key::from_slice(&plaintext)
}

/// The master key and the subkeys derived from it. All zeroized on drop.
pub(crate) struct VaultKeys {
    pub(crate) master: Key,
    pub(crate) mac: Key,
    pub(crate) index: Key,
}

impl VaultKeys {
    pub(crate) fn derive(master: Key) -> Result<Self, VaultError> {
        Ok(Self {
            mac: master.derive(&[MAC_KEY_INFO])?,
            index: master.derive(&[INDEX_KEY_INFO])?,
            master,
        })
    }
}

/// A vault whose header was read and structurally validated, but not unlocked.
/// It is a snapshot of the header at open time.
pub struct LockedVault {
    dir: VaultDir,
    header: Header,
    digest: [u8; 32],
}

impl LockedVault {
    pub(crate) fn from_parts(dir: VaultDir, header: Header, digest: [u8; 32]) -> Self {
        Self {
            dir,
            header,
            digest,
        }
    }

    pub fn vault_id(&self) -> &EntityId {
        &self.header.vault_id
    }

    pub fn created_at(&self) -> &Timestamp {
        &self.header.created_at
    }

    pub fn dir(&self) -> &Path {
        self.dir.root()
    }

    pub fn has_recovery_key(&self) -> bool {
        self.header.recovery.is_some()
    }

    /// The Argon2id parameters of the passphrase slot.
    pub fn kdf_params(&self) -> KdfParams {
        match self.header.passphrase.kdf {
            SlotKdf::Argon2id(params) => params,
            // Parsing guarantees the passphrase slot uses Argon2id.
            SlotKdf::HkdfSha256 => KdfParams::default(),
        }
    }

    /// Unlock with the passphrase. Runs Argon2id with the header's parameters.
    pub fn unlock(&self, passphrase: &SecretString) -> Result<UnlockedVault, VaultError> {
        let SlotKdf::Argon2id(params) = self.header.passphrase.kdf else {
            return Err(VaultError::CorruptHeader(
                "passphrase slot is not argon2id".into(),
            ));
        };
        let kek = passphrase_kek(passphrase, &self.header.passphrase.salt, &params)?;
        let master = unwrap_slot(&self.header.vault_id, &self.header.passphrase, &kek)
            .ok_or(VaultError::WrongPassphrase)?;
        self.finish_unlock(master, UnlockMethod::Passphrase)
    }

    /// Unlock with the recovery key shown at creation.
    pub fn unlock_with_recovery(&self, key: &RecoveryKey) -> Result<UnlockedVault, VaultError> {
        let slot = self
            .header
            .recovery
            .as_ref()
            .ok_or(VaultError::NoRecoveryKey)?;
        let kek = recovery_kek(key, &slot.salt)?;
        let master =
            unwrap_slot(&self.header.vault_id, slot, &kek).ok_or(VaultError::WrongRecoveryKey)?;
        self.finish_unlock(master, UnlockMethod::RecoveryKey)
    }

    /// Verify the header MAC and decrypt the index with the unwrapped master key.
    fn finish_unlock(
        &self,
        master: Key,
        method: UnlockMethod,
    ) -> Result<UnlockedVault, VaultError> {
        let keys = VaultKeys::derive(master)?;
        if !verify_mac(&keys.mac, &self.header.mac_input(), &self.header.mac) {
            return Err(VaultError::Integrity(IntegrityTarget::Header));
        }
        let index = Index::open(&self.header, &keys.index)?;
        let unlocked = UnlockedVault::new(
            self.dir.clone(),
            self.header.clone(),
            self.digest,
            keys,
            index,
            method,
        );
        unlocked.collect_garbage();
        Ok(unlocked)
    }
}

impl fmt::Debug for LockedVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LockedVault")
            .field("vault_id", &self.header.vault_id)
            .field("dir", &self.dir.root())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exhaustive and in memory (no disk round trip per case): flipping bit 0 or bit 5
    /// of any header byte must make parsing or unlocking fail.
    #[test]
    fn every_single_bit_flip_in_the_header_prevents_unlock() {
        let root = tempfile::tempdir().unwrap();
        let passphrase = SecretString::from("correct horse battery staple".to_owned());
        let options = CreateOptions {
            kdf: KdfParams::new(8, 1, 1).unwrap(),
            recovery_key: true,
        };
        let (vault, _) = Vault::create(root.path(), &passphrase, options).unwrap();
        drop(vault);
        let dir = VaultDir::existing(root.path()).unwrap();
        let original = dir.read_header().unwrap().unwrap();
        let unlock = |bytes: &[u8]| -> Result<UnlockedVault, VaultError> {
            let header = Header::parse(bytes)?;
            LockedVault::from_parts(dir.clone(), header, sha256(bytes)).unlock(&passphrase)
        };
        assert!(unlock(&original).is_ok());
        for position in 0..original.len() {
            for bit in [0x01_u8, 0x20] {
                let mut tampered = original.clone();
                tampered[position] ^= bit;
                assert!(
                    unlock(&tampered).is_err(),
                    "flipping bit {bit:#04x} of byte {position} went unnoticed"
                );
            }
        }
    }
}
