//! Backup/recovery semantics: encrypted, verified snapshots that never overwrite.

mod common;

use aidex_vault::{MAX_RECORD_FILES, MAX_SECRETS, SecretBytes, SecretKind, Vault, VaultErrorKind};
use common::*;
use std::fs;

#[test]
fn backups_are_encrypted_verified_snapshots_that_restore_elsewhere() {
    let root = tempfile::tempdir().unwrap();
    let (dir, mut vault, recovery) = create_with_secret(root.path());
    let backup = root.path().join("backup");
    let report = vault.backup_to(&backup).unwrap();
    assert_eq!(report.vault_id, *vault.vault_id());
    assert_eq!(report.records, 1);
    assert_eq!(files(&backup).len(), 2, "header + one record");

    // Later changes to the live vault do not affect the snapshot.
    vault
        .put(
            &name("added-later"),
            SecretKind::Generic,
            SecretBytes::from(b"later".to_vec()),
            policy(&[]),
        )
        .unwrap();
    drop(vault);

    let new_drive = tempfile::tempdir().unwrap();
    let target = vault_dir(new_drive.path());
    let locked = Vault::restore_from(&backup, &target).unwrap();
    assert_eq!(locked.vault_id(), &report.vault_id);
    let mut restored = locked.unlock(&passphrase(PASSPHRASE)).unwrap();
    assert_eq!(restored.verify().unwrap().records, 1);
    assert_eq!(reveal(&mut restored, NAME), VALUE.as_bytes());
    assert!(restored.meta(&name("added-later")).is_err());
    drop(restored);
    Vault::open(&target)
        .unwrap()
        .unlock_with_recovery(&recovery)
        .unwrap();
    // The original is untouched by backup and restore.
    let original = unlock(&dir);
    assert_eq!(original.len(), 2);
}

#[test]
fn backup_and_restore_refuse_to_overwrite_a_vault() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    let other_root = tempfile::tempdir().unwrap();
    let (other_dir, other, _) = create_with_secret(other_root.path());
    drop(other);
    let other_header = fs::read(header_path(&other_dir)).unwrap();

    let error = vault.backup_to(&other_dir).unwrap_err();
    assert_eq!(error.kind(), VaultErrorKind::AlreadyExists);
    assert_eq!(fs::read(header_path(&other_dir)).unwrap(), other_header);
    assert_eq!(
        vault.backup_to(&dir).unwrap_err().kind(),
        VaultErrorKind::AlreadyExists
    );

    let backup = root.path().join("backup");
    vault.backup_to(&backup).unwrap();
    let error = Vault::restore_from(&backup, &other_dir).unwrap_err();
    assert_eq!(error.kind(), VaultErrorKind::AlreadyExists);
    assert_eq!(fs::read(header_path(&other_dir)).unwrap(), other_header);
    assert_eq!(
        Vault::restore_from(&backup, &dir).unwrap_err().kind(),
        VaultErrorKind::AlreadyExists
    );
    let error =
        Vault::restore_from(&root.path().join("nothing"), &root.path().join("x")).unwrap_err();
    assert_eq!(error.kind(), VaultErrorKind::NotFound);
}

#[test]
fn damaged_backups_are_detected_on_restore_or_unlock() {
    let root = tempfile::tempdir().unwrap();
    let (_dir, vault, _) = create_with_secret(root.path());
    let backup = root.path().join("backup");
    vault.backup_to(&backup).unwrap();
    drop(vault);

    let truncated = root.path().join("truncated");
    copy_dir(&backup, &truncated);
    let header = fs::read(header_path(&truncated)).unwrap();
    fs::write(header_path(&truncated), &header[..header.len() / 3]).unwrap();
    let target = root.path().join("restore-a");
    assert_eq!(
        Vault::restore_from(&truncated, &target).unwrap_err().kind(),
        VaultErrorKind::Corrupt
    );
    assert!(
        !Vault::exists(&target),
        "a failed restore leaves no header behind"
    );

    let damaged = root.path().join("damaged");
    copy_dir(&backup, &damaged);
    let record = record_files(&damaged).pop().unwrap();
    let mut bytes = fs::read(&record).unwrap();
    let position = bytes.len() - 10;
    bytes[position] = if bytes[position] == b'A' { b'B' } else { b'A' };
    fs::write(&record, bytes).unwrap();
    let target = root.path().join("restore-b");
    let restored = Vault::restore_from(&damaged, &target).unwrap();
    let unlocked = restored.unlock(&passphrase(PASSPHRASE)).unwrap();
    assert!(
        unlocked.verify().is_err(),
        "verify authenticates every record"
    );
}

#[test]
fn restore_allows_orphan_headroom_and_bounds_record_files_before_copying() {
    let root = tempfile::tempdir().unwrap();
    // A raw live vault directory: one indexed secret, plus well-named files standing
    // in for the rest of a full vault (MAX_SECRETS) and for the orphans interrupted
    // commits preserved beside it (the remaining headroom).
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);
    let original_header = fs::read(header_path(&dir)).unwrap();
    let records = dir.join("records");
    for id in 0..MAX_RECORD_FILES - 1 {
        fs::write(records.join(format!("{id:032x}.json")), b"orphan").unwrap();
    }
    assert!(
        record_files(&dir).len() > MAX_SECRETS + 1,
        "beyond the old bound"
    );
    let allowed = root.path().join("allowed");
    Vault::restore_from(&dir, &allowed).unwrap();
    assert_eq!(record_files(&allowed).len(), MAX_RECORD_FILES);
    assert_eq!(fs::read(header_path(&allowed)).unwrap(), original_header);
    // The first unlock keeps the indexed secret and collects the orphans.
    let unlocked = Vault::open(&allowed)
        .unwrap()
        .unlock(&passphrase(PASSPHRASE))
        .unwrap();
    assert_eq!(unlocked.verify().unwrap().records, 1);
    assert_eq!(record_files(&allowed).len(), 1);
    drop(unlocked);

    fs::write(
        records.join(format!("{:032x}.json", MAX_RECORD_FILES)),
        b"extra",
    )
    .unwrap();
    let rejected = root.path().join("rejected");
    assert_eq!(
        Vault::restore_from(&dir, &rejected).unwrap_err().kind(),
        VaultErrorKind::LimitExceeded
    );
    assert!(!Vault::exists(&rejected));
    assert!(files(&rejected).is_empty());
    assert_eq!(fs::read(header_path(&dir)).unwrap(), original_header);
    assert_eq!(record_files(&dir).len(), MAX_RECORD_FILES + 1);
}
