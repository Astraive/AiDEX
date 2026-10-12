//! Encrypted backups and restores (spec §64, §65).
//!
//! A backup is a byte-for-byte copy of the committed `vault.json` plus every record
//! file it references. It stays encrypted under the same keys: whoever can unlock
//! the source can unlock the backup, with the same passphrase or recovery key. A
//! backup does not help when both the passphrase and the recovery key are lost.

use crate::{
    LockedVault, MAX_SECRETS, UnlockedVault, Vault, VaultError,
    crypto::{sha256, verify_mac},
    error::IntegrityTarget,
    format::{
        BlobId,
        header::Header,
        index::Index,
        record::{RecordLocation, open_record},
    },
    fsio::VaultDir,
};
use aidex_core::EntityId;
use std::path::Path;

/// Result of [`UnlockedVault::backup_to`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackupReport {
    pub vault_id: EntityId,
    /// Index generation captured by the backup.
    pub generation: u64,
    /// Record files copied and verified.
    pub records: usize,
}

impl UnlockedVault {
    /// Copy a consistent encrypted snapshot into `dest` and verify it.
    ///
    /// `dest` is created if missing and must not already hold vault data. Records are
    /// copied first and the header last, so an interrupted backup is never mistaken for
    /// a complete one. Verification re-reads the copy and authenticates the header MAC,
    /// the index, and every record with this session's keys.
    pub fn backup_to(&self, dest: &Path) -> Result<BackupReport, VaultError> {
        let source = self.vault_dir();
        let header_bytes = source.read_header()?.ok_or(VaultError::Conflict)?;
        if sha256(&header_bytes) != self.header_digest() {
            return Err(VaultError::Conflict);
        }
        let target = VaultDir::create_or_existing(dest)?;
        if target.root() == source.root() || target.has_vault_data()? {
            return Err(VaultError::AlreadyExists);
        }
        let blobs: Vec<&BlobId> = self
            .index()
            .records
            .iter()
            .map(|entry| &entry.blob)
            .collect();
        let copied = copy_records(source, &target, &blobs)
            .and_then(|()| target.write_header(&header_bytes))
            .and_then(|()| self.verify_copy(&target, &header_bytes));
        if let Err(error) = copied {
            remove_partial(&target, &blobs);
            return Err(error);
        }
        Ok(BackupReport {
            vault_id: self.vault_id().clone(),
            generation: self.index().generation,
            records: blobs.len(),
        })
    }

    fn verify_copy(&self, target: &VaultDir, expected: &[u8]) -> Result<(), VaultError> {
        let bytes = target.read_header()?.ok_or(VaultError::MissingHeader)?;
        if bytes != expected {
            return Err(VaultError::CorruptHeader(
                "backup header differs from the source".into(),
            ));
        }
        let header = Header::parse(&bytes)?;
        if !verify_mac(&self.keys().mac, &header.mac_input(), &header.mac) {
            return Err(VaultError::Integrity(IntegrityTarget::Header));
        }
        let index = Index::open(&header, &self.keys().index)?;
        for entry in &index.records {
            let bytes = target.read_record(&entry.blob)?.ok_or_else(|| {
                VaultError::CorruptRecord("a record is missing from the backup".into())
            })?;
            let location = RecordLocation {
                vault_id: &header.vault_id,
                record_id: &entry.record_id,
                revision: entry.revision,
                blob: &entry.blob,
            };
            drop(open_record(&self.keys().master, &location, &bytes)?);
        }
        Ok(())
    }
}

impl Vault {
    /// Restore a backup from `src` into `target` and return it locked.
    ///
    /// Refuses to overwrite: `target` must not hold vault data. All well-formed record
    /// files and the header are copied (header last) and compared byte-for-byte with
    /// the source. Cryptographic verification needs the passphrase or recovery key and
    /// happens on unlock; call [`UnlockedVault::verify`] to check every record.
    pub fn restore_from(src: &Path, target: &Path) -> Result<LockedVault, VaultError> {
        let source = Vault::open(src)?;
        let source_dir = VaultDir::existing(src)?;
        let destination = VaultDir::create_or_existing(target)?;
        if destination.root() == source_dir.root() || destination.has_vault_data()? {
            return Err(VaultError::AlreadyExists);
        }
        let header_bytes = source_dir.read_header()?.ok_or(VaultError::MissingHeader)?;
        if Header::parse(&header_bytes)?.vault_id != *source.vault_id() {
            return Err(VaultError::Conflict);
        }
        let blobs = source_dir.record_blobs(Some(MAX_SECRETS))?;
        let blob_refs: Vec<&BlobId> = blobs.iter().collect();
        let restored = copy_records(&source_dir, &destination, &blob_refs)
            .and_then(|()| destination.write_header(&header_bytes))
            .and_then(|()| {
                for blob in &blobs {
                    if destination.read_record(blob)? != source_dir.read_record(blob)? {
                        return Err(VaultError::CorruptRecord(
                            "restored record differs from the backup".into(),
                        ));
                    }
                }
                Vault::open(target)
            });
        match restored {
            Ok(locked) if locked.vault_id() == source.vault_id() => Ok(locked),
            Ok(_) => {
                remove_partial(&destination, &blob_refs);
                Err(VaultError::Conflict)
            }
            Err(error) => {
                remove_partial(&destination, &blob_refs);
                Err(error)
            }
        }
    }
}

fn copy_records(source: &VaultDir, target: &VaultDir, blobs: &[&BlobId]) -> Result<(), VaultError> {
    for blob in blobs {
        let bytes = source.read_record(blob)?.ok_or_else(|| {
            VaultError::CorruptRecord("a referenced record file is missing".into())
        })?;
        target.write_record(blob, &bytes)?;
    }
    Ok(())
}

/// Best-effort cleanup of a failed backup or restore (never touches a source).
fn remove_partial(target: &VaultDir, blobs: &[&BlobId]) {
    let _ = std::fs::remove_file(target.header_path());
    for blob in blobs {
        let _ = target.remove_record(blob);
    }
}
