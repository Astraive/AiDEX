//! Acceptance: wrong passphrase, tampering, missing metadata, and interrupted writes
//! fail safely without plaintext fallback.

mod common;

use aidex_vault::{IntegrityTarget, SecretBytes, SecretKind, Vault, VaultError, VaultErrorKind};
use common::*;
use serde_json::json;
use std::{fs, time::Duration};

/// A labelled in-place edit of the parsed `vault.json`.
type Edit = Box<dyn Fn(&mut serde_json::Value)>;

fn unlock_error(dir: &std::path::Path) -> VaultError {
    match Vault::open(dir) {
        Ok(locked) => locked
            .unlock(&passphrase(PASSPHRASE))
            .expect_err("tampered vault must not unlock"),
        Err(error) => error,
    }
}

fn resolve_error(dir: &std::path::Path, secret: &str) -> VaultError {
    let mut vault = unlock(dir);
    let handle = vault
        .issue_handle(&name(secret), &openai(), Duration::from_secs(60))
        .unwrap();
    vault
        .resolve(&handle, &openai())
        .expect_err("tampered record must not resolve")
}

#[test]
fn wrong_passphrase_is_refused_without_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);
    let before: Vec<_> = files(&dir).iter().map(|f| fs::read(f).unwrap()).collect();
    for wrong in [
        "correct horse battery stapl",
        "Correct horse battery staple",
        "",
    ] {
        let error = Vault::open(&dir)
            .unwrap()
            .unlock(&passphrase(wrong))
            .unwrap_err();
        assert_eq!(error.kind(), VaultErrorKind::WrongPassphrase, "{wrong}");
        assert!(!error.to_string().contains(wrong) || wrong.is_empty());
    }
    let after: Vec<_> = files(&dir).iter().map(|f| fs::read(f).unwrap()).collect();
    assert_eq!(before, after, "a failed unlock writes nothing");
}

/// Sampled through real files (every 9th byte, so every field is hit); the exhaustive
/// in-memory variant is the `vault` unit test `every_single_bit_flip_in_the_header_prevents_unlock`.
#[test]
fn bit_flips_in_the_header_file_are_detected() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);
    let original = fs::read(header_path(&dir)).unwrap();
    for position in (0..original.len()).step_by(9) {
        let mut tampered = original.clone();
        tampered[position] ^= 0x01;
        fs::write(header_path(&dir), &tampered).unwrap();
        let error = unlock_error(&dir);
        assert!(
            matches!(
                error.kind(),
                VaultErrorKind::Corrupt
                    | VaultErrorKind::Tampered
                    | VaultErrorKind::WrongPassphrase
                    | VaultErrorKind::UnsupportedVersion
            ),
            "byte {position}: {error}"
        );
    }
    fs::write(header_path(&dir), &original).unwrap();
    let mut vault = unlock(&dir);
    assert_eq!(reveal(&mut vault, NAME), VALUE.as_bytes());
}

#[test]
fn unauthenticated_header_cannot_request_excessive_kdf_work() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);
    let original = fs::read(header_path(&dir)).unwrap();
    for (memory, iterations, lanes) in [
        (1024 * 1024, 64, 16), // Previously permitted attacker-selected maxima.
        (65_537, 1, 1),
        (65_536, 4, 1),
        (65_536, 3, 5),
    ] {
        fs::write(header_path(&dir), &original).unwrap();
        edit_header(&dir, |header| {
            header["slots"][0]["kdf"]["memory_kib"] = json!(memory);
            header["slots"][0]["kdf"]["iterations"] = json!(iterations);
            header["slots"][0]["kdf"]["parallelism"] = json!(lanes);
        });
        let before = fs::read(header_path(&dir)).unwrap();
        // Rejection must happen at parsing, not after an expensive unlock attempt.
        assert_eq!(
            Vault::open(&dir).unwrap_err().kind(),
            VaultErrorKind::Corrupt
        );
        assert_eq!(fs::read(header_path(&dir)).unwrap(), before);
    }
    fs::write(header_path(&dir), original).unwrap();
    assert!(
        Vault::open(&dir)
            .unwrap()
            .unlock(&passphrase(PASSPHRASE))
            .is_ok()
    );
}

#[test]
fn semantic_header_edits_fail_authentication() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);
    let original = fs::read(header_path(&dir)).unwrap();
    let cases: Vec<(&str, Edit, VaultErrorKind)> = vec![
        (
            "index ciphertext",
            Box::new(|h| {
                let text = h["index"]["ciphertext"].as_str().unwrap().to_owned();
                h["index"]["ciphertext"] = json!(mutate_base64(&text));
            }),
            VaultErrorKind::Tampered,
        ),
        (
            "index generation",
            Box::new(|h| h["index"]["generation"] = json!(99)),
            VaultErrorKind::Tampered,
        ),
        (
            "created_at",
            Box::new(|h| h["created_at"] = json!("2000-01-01T00:00:00.000Z")),
            VaultErrorKind::Tampered,
        ),
        (
            "recovery slot removed",
            Box::new(|h| {
                h["slots"].as_array_mut().unwrap().truncate(1);
            }),
            VaultErrorKind::Tampered,
        ),
        (
            "weakened kdf parameters",
            Box::new(|h| h["slots"][0]["kdf"]["memory_kib"] = json!(16)),
            VaultErrorKind::WrongPassphrase,
        ),
        (
            "wrapped key",
            Box::new(|h| {
                let text = h["slots"][0]["wrapped_key"].as_str().unwrap().to_owned();
                h["slots"][0]["wrapped_key"] = json!(mutate_base64(&text));
            }),
            VaultErrorKind::WrongPassphrase,
        ),
        (
            "vault id",
            Box::new(|h| h["vault_id"] = json!("vault_01JZZZZZZZZZZZZZZZZZZZZZZZ")),
            VaultErrorKind::WrongPassphrase,
        ),
        (
            "mac",
            Box::new(|h| {
                let text = h["mac"].as_str().unwrap().to_owned();
                h["mac"] = json!(mutate_base64(&text));
            }),
            VaultErrorKind::Tampered,
        ),
    ];
    for (label, edit, expected) in cases {
        fs::write(header_path(&dir), &original).unwrap();
        edit_header(&dir, |header| edit(header));
        let error = unlock_error(&dir);
        assert_eq!(error.kind(), expected, "{label}: {error}");
    }
    fs::write(header_path(&dir), &original).unwrap();
    edit_header(&dir, |h| {
        let text = h["index"]["ciphertext"].as_str().unwrap().to_owned();
        h["index"]["ciphertext"] = json!(mutate_base64(&text));
    });
    assert!(matches!(
        unlock_error(&dir),
        VaultError::Integrity(IntegrityTarget::Header)
    ));
}

#[test]
fn tampered_truncated_or_missing_records_never_resolve() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);
    let record = record_files(&dir).pop().unwrap();
    let original = fs::read(&record).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let text = value["ciphertext"].as_str().unwrap().to_owned();
    value["ciphertext"] = json!(mutate_base64(&text));
    fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        resolve_error(&dir, NAME),
        VaultError::Integrity(IntegrityTarget::Record)
    ));

    for position in (0..original.len()).step_by(7) {
        let mut flipped = original.clone();
        flipped[position] ^= 0x01;
        fs::write(&record, &flipped).unwrap();
        let error = resolve_error(&dir, NAME);
        assert!(
            matches!(
                error.kind(),
                VaultErrorKind::Corrupt
                    | VaultErrorKind::Tampered
                    | VaultErrorKind::UnsupportedVersion
            ),
            "byte {position}: {error}"
        );
    }

    fs::write(&record, &original[..original.len() / 2]).unwrap();
    assert_eq!(resolve_error(&dir, NAME).kind(), VaultErrorKind::Corrupt);
    fs::write(&record, b"").unwrap();
    assert_eq!(resolve_error(&dir, NAME).kind(), VaultErrorKind::Corrupt);
    let mut newer: serde_json::Value = serde_json::from_slice(&original).unwrap();
    newer["format_version"] = json!(2);
    fs::write(&record, serde_json::to_vec(&newer).unwrap()).unwrap();
    assert_eq!(
        resolve_error(&dir, NAME).kind(),
        VaultErrorKind::UnsupportedVersion
    );
    fs::remove_file(&record).unwrap();
    assert_eq!(resolve_error(&dir, NAME).kind(), VaultErrorKind::Corrupt);
}

#[test]
fn swapped_record_files_fail_authentication() {
    let root = tempfile::tempdir().unwrap();
    let (dir, mut vault, _) = create_with_secret(root.path());
    vault
        .put(
            &name("other"),
            SecretKind::Generic,
            SecretBytes::from(b"other-value".to_vec()),
            policy(&[openai()]),
        )
        .unwrap();
    drop(vault);
    let records = record_files(&dir);
    assert_eq!(records.len(), 2);
    let (a, b) = (
        fs::read(&records[0]).unwrap(),
        fs::read(&records[1]).unwrap(),
    );
    fs::write(&records[0], &b).unwrap();
    fs::write(&records[1], &a).unwrap();
    for secret in [NAME, "other"] {
        assert!(matches!(
            resolve_error(&dir, secret),
            VaultError::Integrity(IntegrityTarget::Record)
        ));
    }

    // A record from another vault with the same layout is rejected as well.
    let foreign_root = tempfile::tempdir().unwrap();
    let (foreign_dir, foreign, _) = create_with_secret(foreign_root.path());
    drop(foreign);
    let foreign_record = fs::read(record_files(&foreign_dir).pop().unwrap()).unwrap();
    fs::write(&records[0], &foreign_record).unwrap();
    fs::write(&records[1], &foreign_record).unwrap();
    for secret in [NAME, "other"] {
        assert_eq!(resolve_error(&dir, secret).kind(), VaultErrorKind::Tampered);
    }
}

#[test]
fn truncated_garbled_missing_and_unknown_headers_are_explicit_errors() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);
    let original = fs::read(header_path(&dir)).unwrap();
    for (bytes, expected) in [
        (
            original[..original.len() / 2].to_vec(),
            VaultErrorKind::Corrupt,
        ),
        (Vec::new(), VaultErrorKind::Corrupt),
        (b"\x00\xffgarbage".to_vec(), VaultErrorKind::Corrupt),
        (
            b"{\"format\":\"aidex-vault\"}".to_vec(),
            VaultErrorKind::Corrupt,
        ),
    ] {
        fs::write(header_path(&dir), &bytes).unwrap();
        assert_eq!(Vault::open(&dir).unwrap_err().kind(), expected);
    }
    fs::write(header_path(&dir), &original).unwrap();
    edit_header(&dir, |h| h["format_version"] = json!(2));
    let error = Vault::open(&dir).unwrap_err();
    assert!(matches!(error, VaultError::UnsupportedVersion { found: 2 }));

    fs::remove_file(header_path(&dir)).unwrap();
    assert!(!Vault::exists(&dir));
    assert_eq!(
        Vault::open(&dir).unwrap_err().kind(),
        VaultErrorKind::MissingHeader
    );
    // Records without a header are never silently replaced by a new, empty vault.
    let error = Vault::create(
        &dir,
        &passphrase(PASSPHRASE),
        aidex_vault::CreateOptions::default(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), VaultErrorKind::AlreadyExists);
}

#[test]
fn interrupted_writes_leave_the_last_committed_state_readable() {
    let root = tempfile::tempdir().unwrap();
    let (dir, vault, _) = create_with_secret(root.path());
    drop(vault);

    // Produce the exact on-disk state of a crash between "record written" and
    // "header renamed": run the write in a copy, then transplant its new record and
    // its new header (as a temp file that was never renamed) into the original.
    let scratch = tempfile::tempdir().unwrap();
    let copy = vault_dir(scratch.path());
    copy_dir(&dir, &copy);
    let before: Vec<_> = record_files(&copy);
    let mut writer = unlock(&copy);
    writer
        .put(
            &name(NAME),
            SecretKind::ApiKey,
            SecretBytes::from(b"never-committed-value".to_vec()),
            policy(&[openai()]),
        )
        .unwrap();
    drop(writer);
    let new_record = record_files(&copy)
        .into_iter()
        .find(|path| !before.contains(path))
        .unwrap();
    fs::copy(
        &new_record,
        dir.join("records").join(new_record.file_name().unwrap()),
    )
    .unwrap();
    fs::copy(
        header_path(&copy),
        dir.join(".vault.json.0123456789abcdef.tmp"),
    )
    .unwrap();
    fs::write(
        dir.join("records").join(".deadbeef.json.00ff.tmp"),
        b"partial",
    )
    .unwrap();
    // A torn temp header is also harmless.
    fs::write(
        dir.join(".vault.json.fedcba9876543210.tmp"),
        b"{\"format\":",
    )
    .unwrap();

    let mut vault = unlock(&dir);
    assert_eq!(
        reveal(&mut vault, NAME),
        VALUE.as_bytes(),
        "old state is intact"
    );
    drop(vault);
    let leftovers: Vec<_> = files(&dir)
        .into_iter()
        .filter(|path| path.to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temporary files are cleaned: {leftovers:?}"
    );
    assert_eq!(
        record_files(&dir).len(),
        1,
        "the orphaned record is collected"
    );
}

#[test]
fn plaintext_values_and_names_never_reach_the_disk() {
    let root = tempfile::tempdir().unwrap();
    let (dir, mut vault, recovery) = create_with_secret(root.path());
    vault
        .put(
            &name("second-marker-name"),
            SecretKind::Password,
            SecretBytes::from(b"SECOND-PLAINTEXT-MARKER".to_vec()),
            policy(&[github_mcp()]),
        )
        .unwrap();
    vault
        .change_passphrase(&passphrase(PASSPHRASE), &passphrase("another passphrase 2"))
        .unwrap();
    let backup = root.path().join("backup");
    vault.backup_to(&backup).unwrap();
    for needle in [
        VALUE.as_bytes(),
        NAME.as_bytes(),
        b"SECOND-PLAINTEXT-MARKER",
        b"second-marker-name",
        b"provider:openai",
        b"mcp:github",
        PASSPHRASE.as_bytes(),
        b"another passphrase 2",
        recovery.encode().expose().as_bytes(),
    ] {
        assert_absent(&dir, needle);
        assert_absent(&backup, needle);
    }
}
