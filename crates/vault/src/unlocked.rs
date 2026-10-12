//! Operations on an unlocked vault: secret metadata, writes, passphrase changes,
//! and handle issuance/resolution. There is deliberately no API that returns a
//! secret value except [`UnlockedVault::resolve`] for a valid scoped handle.

use crate::{
    ConsumerScope, LockedVault, SecretBytes, SecretHandle, SecretKind, SecretMeta, SecretName,
    SecretPolicy, SecretString, UnlockMethod, VaultError,
    crypto::{random_array, sha256},
    error::HandleDenial,
    format::{
        BlobId,
        header::{Header, SlotKdf, SlotKind},
        index::{Index, IndexEntry},
        record::{RecordLocation, open_record, seal_record},
    },
    fsio::VaultDir,
    handles::HandleRegistry,
    kdf::{passphrase_kek, validate_passphrase},
    vault::{VaultKeys, unwrap_slot, wrap_slot},
};
use aidex_core::{EntityId, SecretRef, Timestamp};
use std::{
    collections::HashSet,
    fmt,
    path::Path,
    time::{Duration, Instant},
};

/// Largest secret value in bytes.
pub const MAX_SECRET_BYTES: usize = 64 * 1024;
/// Most secrets one vault holds.
pub const MAX_SECRETS: usize = 4096;

/// An unlocked vault. The master key and derived keys are zeroized when it is
/// dropped or [`locked`](UnlockedVault::lock); its handles die with it.
pub struct UnlockedVault {
    dir: VaultDir,
    header: Header,
    /// SHA-256 of the committed `vault.json` this state was loaded from or wrote.
    digest: [u8; 32],
    keys: VaultKeys,
    index: Index,
    handles: HandleRegistry,
    method: UnlockMethod,
}

/// Result of [`UnlockedVault::verify`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifyReport {
    pub vault_id: EntityId,
    pub generation: u64,
    pub records: usize,
}

impl UnlockedVault {
    pub(crate) fn new(
        dir: VaultDir,
        header: Header,
        digest: [u8; 32],
        keys: VaultKeys,
        index: Index,
        method: UnlockMethod,
    ) -> Self {
        Self {
            dir,
            header,
            digest,
            keys,
            index,
            handles: HandleRegistry::default(),
            method,
        }
    }

    pub fn vault_id(&self) -> &EntityId {
        &self.header.vault_id
    }

    pub fn dir(&self) -> &Path {
        self.dir.root()
    }

    pub fn unlock_method(&self) -> UnlockMethod {
        self.method
    }

    pub fn has_recovery_key(&self) -> bool {
        self.header.recovery.is_some()
    }

    /// Number of stored secrets.
    pub fn len(&self) -> usize {
        self.index.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.records.is_empty()
    }

    /// Metadata for every secret, sorted by name. Contains no values.
    pub fn list(&self) -> Vec<SecretMeta> {
        let mut secrets: Vec<SecretMeta> = self
            .index
            .records
            .iter()
            .map(|entry| entry.meta(&self.header.vault_id))
            .collect();
        secrets.sort_by(|a, b| a.name.cmp(&b.name));
        secrets
    }

    pub fn meta(&self, name: &SecretName) -> Result<SecretMeta, VaultError> {
        self.entry(name)
            .map(|entry| entry.meta(&self.header.vault_id))
    }

    /// Store or replace a secret. Replacing keeps its id (and live handles, if the new
    /// policy still allows their consumer) and writes the value to a fresh record file.
    pub fn put(
        &mut self,
        name: &SecretName,
        kind: SecretKind,
        value: SecretBytes,
        policy: SecretPolicy,
    ) -> Result<SecretMeta, VaultError> {
        if value.is_empty() {
            return Err(VaultError::InvalidSecret("value must not be empty"));
        }
        if value.len() > MAX_SECRET_BYTES {
            return Err(VaultError::InvalidSecret("value must be at most 64 KiB"));
        }
        policy.validate()?;
        let now = Timestamp::now();
        let previous = self.index.by_name(name).cloned();
        if previous.is_none() && self.index.records.len() >= MAX_SECRETS {
            return Err(VaultError::LimitExceeded("at most 4096 secrets per vault"));
        }
        let record_id = match &previous {
            Some(entry) => entry.record_id.clone(),
            None => EntityId::generate("sec")
                .map_err(|error| VaultError::Randomness(error.to_string()))?,
        };
        let revision = match &previous {
            Some(entry) => entry
                .revision
                .checked_add(1)
                .ok_or(VaultError::LimitExceeded("record revision overflow"))?,
            None => 1,
        };
        let entry = IndexEntry {
            record_id,
            name: name.clone(),
            kind,
            created_at: previous
                .as_ref()
                .map_or_else(|| now.clone(), |entry| entry.created_at.clone()),
            updated_at: now,
            policy,
            blob: BlobId::generate()?,
            revision,
        };
        let file = seal_record(&self.keys.master, &self.location(&entry), &value)?;
        drop(value);
        self.dir.write_record(&entry.blob, &file)?;
        let mut records: Vec<IndexEntry> = self
            .index
            .records
            .iter()
            .filter(|existing| &existing.name != name)
            .cloned()
            .collect();
        records.push(entry.clone());
        if let Err(error) = self.commit_index(records) {
            // Remove the new record only if the old header is provably still committed;
            // a header that was renamed into place but not synced may reference it.
            if !matches!(&error, VaultError::CommitUncertain { .. })
                && self.ensure_unchanged_on_disk().is_ok()
            {
                let _ = self.dir.remove_record(&entry.blob);
            }
            return Err(error);
        }
        if let Some(previous) = previous {
            let _ = self.dir.remove_record(&previous.blob);
        }
        Ok(entry.meta(&self.header.vault_id))
    }

    /// Delete a secret; its handles stop resolving immediately.
    pub fn delete(&mut self, name: &SecretName) -> Result<SecretMeta, VaultError> {
        let removed = self.entry(name)?.clone();
        let records = self
            .index
            .records
            .iter()
            .filter(|entry| &entry.name != name)
            .cloned()
            .collect();
        self.commit_index(records)?;
        let _ = self.dir.remove_record(&removed.blob);
        Ok(removed.meta(&self.header.vault_id))
    }

    /// Replace the passphrase after re-checking the current one. Records are not
    /// re-encrypted: only the passphrase slot is rewrapped with a fresh salt.
    pub fn change_passphrase(
        &mut self,
        old: &SecretString,
        new: &SecretString,
    ) -> Result<(), VaultError> {
        let SlotKdf::Argon2id(params) = self.header.passphrase.kdf else {
            return Err(VaultError::CorruptHeader(
                "passphrase slot is not argon2id".into(),
            ));
        };
        let kek = passphrase_kek(old, &self.header.passphrase.salt, &params)?;
        let unwrapped = unwrap_slot(&self.header.vault_id, &self.header.passphrase, &kek)
            .ok_or(VaultError::WrongPassphrase)?;
        if !unwrapped.ct_eq(&self.keys.master) {
            return Err(VaultError::WrongPassphrase);
        }
        self.set_passphrase(new, params)
    }

    /// Set a new passphrase without the old one. Allowed only when this session was
    /// unlocked with the recovery key (the "forgot my passphrase" path).
    pub fn reset_passphrase(&mut self, new: &SecretString) -> Result<(), VaultError> {
        if self.method != UnlockMethod::RecoveryKey {
            return Err(VaultError::PassphraseRequired);
        }
        let params = match self.header.passphrase.kdf {
            SlotKdf::Argon2id(params) => params,
            SlotKdf::HkdfSha256 => crate::KdfParams::default(),
        };
        self.set_passphrase(new, params)
    }

    fn set_passphrase(
        &mut self,
        new: &SecretString,
        params: crate::KdfParams,
    ) -> Result<(), VaultError> {
        validate_passphrase(new)?;
        let salt = random_array()?;
        let slot = wrap_slot(
            &self.header.vault_id,
            SlotKind::Passphrase,
            SlotKdf::Argon2id(params),
            salt,
            &passphrase_kek(new, &salt, &params)?,
            &self.keys.master,
        )?;
        let mut header = self.header.clone();
        header.passphrase = slot;
        self.commit_header(header)
    }

    /// Issue a handle for `consumer` if the secret's policy allows it.
    pub fn issue_handle(
        &mut self,
        name: &SecretName,
        consumer: &ConsumerScope,
        ttl: Duration,
    ) -> Result<SecretHandle, VaultError> {
        let entry = self.entry(name)?;
        if !entry.policy.allows(consumer) {
            return Err(VaultError::ConsumerNotAllowed {
                name: name.to_string(),
                consumer: consumer.to_string(),
            });
        }
        let secret_ref = SecretRef {
            vault_id: self.header.vault_id.clone(),
            secret_id: entry.record_id.clone(),
        };
        self.handles
            .issue(secret_ref, consumer.clone(), ttl, Instant::now())
    }

    /// Resolve a handle presented by `consumer` to the secret value.
    ///
    /// Denied (fail closed) when the handle is unknown, modified, revoked, or expired,
    /// when `consumer` is not the handle's consumer, when the secret was deleted, or
    /// when its policy no longer allows the consumer.
    pub fn resolve(
        &self,
        handle: &SecretHandle,
        consumer: &ConsumerScope,
    ) -> Result<SecretBytes, VaultError> {
        self.resolve_at(handle, consumer, Instant::now())
    }

    pub(crate) fn resolve_at(
        &self,
        handle: &SecretHandle,
        consumer: &ConsumerScope,
        now: Instant,
    ) -> Result<SecretBytes, VaultError> {
        let secret_ref = self
            .handles
            .check(handle, consumer, now)
            .map_err(VaultError::HandleDenied)?;
        let entry = self
            .index
            .by_record_id(&secret_ref.secret_id)
            .ok_or(VaultError::HandleDenied(HandleDenial::SecretDeleted))?;
        if !entry.policy.allows(consumer) {
            return Err(VaultError::HandleDenied(HandleDenial::PolicyRevoked));
        }
        self.read_value(entry)
    }

    /// Revoke one handle; `false` if it was unknown or already revoked.
    pub fn revoke_handle(&mut self, handle_id: &EntityId) -> bool {
        self.handles.revoke(handle_id)
    }

    /// Revoke every handle issued by this session (emergency stop, safe eject).
    /// Returns how many handles were still live.
    pub fn revoke_all_handles(&mut self) -> usize {
        self.handles.revoke_all(Instant::now())
    }

    /// Handles that are neither revoked nor expired.
    pub fn active_handles(&self) -> usize {
        self.handles.active(Instant::now())
    }

    /// Check that the committed header is unchanged and every record decrypts.
    pub fn verify(&self) -> Result<VerifyReport, VaultError> {
        self.ensure_unchanged_on_disk()?;
        for entry in &self.index.records {
            drop(self.read_value(entry)?);
        }
        Ok(VerifyReport {
            vault_id: self.header.vault_id.clone(),
            generation: self.index.generation,
            records: self.index.records.len(),
        })
    }

    /// Lock: zeroize keys and drop all handles.
    pub fn lock(self) -> LockedVault {
        LockedVault::from_parts(self.dir.clone(), self.header.clone(), self.digest)
    }

    // --- crate-internal helpers -------------------------------------------------

    fn entry(&self, name: &SecretName) -> Result<&IndexEntry, VaultError> {
        self.index
            .by_name(name)
            .ok_or_else(|| VaultError::SecretNotFound(name.to_string()))
    }

    fn location<'a>(&'a self, entry: &'a IndexEntry) -> RecordLocation<'a> {
        RecordLocation {
            vault_id: &self.header.vault_id,
            record_id: &entry.record_id,
            revision: entry.revision,
            blob: &entry.blob,
        }
    }

    fn read_value(&self, entry: &IndexEntry) -> Result<SecretBytes, VaultError> {
        let bytes = self.dir.read_record(&entry.blob)?.ok_or_else(|| {
            VaultError::CorruptRecord("a referenced record file is missing".into())
        })?;
        open_record(&self.keys.master, &self.location(entry), &bytes)
    }

    pub(crate) fn header_digest(&self) -> [u8; 32] {
        self.digest
    }

    pub(crate) fn keys(&self) -> &VaultKeys {
        &self.keys
    }

    pub(crate) fn index(&self) -> &Index {
        &self.index
    }

    pub(crate) fn vault_dir(&self) -> &VaultDir {
        &self.dir
    }

    /// Fail with [`VaultError::Conflict`] if `vault.json` changed since it was loaded.
    pub(crate) fn ensure_unchanged_on_disk(&self) -> Result<(), VaultError> {
        match self.dir.read_header()? {
            Some(bytes) if sha256(&bytes) == self.digest => Ok(()),
            _ => Err(VaultError::Conflict),
        }
    }

    fn commit_index(&mut self, records: Vec<IndexEntry>) -> Result<(), VaultError> {
        let generation = self
            .index
            .generation
            .checked_add(1)
            .ok_or(VaultError::LimitExceeded("index generation overflow"))?;
        let index = Index {
            vault_id: self.header.vault_id.clone(),
            generation,
            records,
        };
        let mut header = self.header.clone();
        header.index = index.seal(&self.keys.index)?;
        let result = self.commit_header(header);
        if result.is_ok() || matches!(&result, Err(VaultError::CommitUncertain { .. })) {
            // Rename succeeded even if directory durability could not be confirmed.
            self.index = index;
        }
        result
    }

    /// Re-MAC `header`, check for concurrent changes, and atomically replace `vault.json`.
    fn commit_header(&mut self, mut header: Header) -> Result<(), VaultError> {
        self.ensure_unchanged_on_disk()?;
        header.mac = crate::crypto::mac(&self.keys.mac, &header.mac_input())?;
        let bytes = header.to_bytes()?;
        let result = self.dir.write_header(&bytes);
        let uncertain = matches!(&result, Err(VaultError::CommitUncertain { .. }));
        if result.is_ok() || uncertain {
            // Keep direct crate callers consistent with the now-visible header.
            self.header = header;
            self.digest = sha256(&bytes);
        }
        if uncertain {
            self.revoke_all_handles();
        }
        result
    }

    /// Best-effort removal of temporary files and record files no longer referenced.
    pub(crate) fn collect_garbage(&self) {
        self.dir.remove_temp_files();
        let Ok(on_disk) = self.dir.record_blobs(None) else {
            return;
        };
        let referenced: HashSet<&BlobId> =
            self.index.records.iter().map(|entry| &entry.blob).collect();
        for blob in on_disk.iter().filter(|blob| !referenced.contains(blob)) {
            let _ = self.dir.remove_record(blob);
        }
    }
}

impl fmt::Debug for UnlockedVault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlockedVault")
            .field("vault_id", &self.header.vault_id)
            .field("dir", &self.dir.root())
            .field("secrets", &self.index.records.len())
            .field("keys", &crate::secret::REDACTED)
            .finish_non_exhaustive()
    }
}
