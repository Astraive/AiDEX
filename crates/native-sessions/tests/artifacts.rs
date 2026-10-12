use aidex_native_sessions::{
    ARTIFACT_PAGE_BYTES, ArtifactEncoding, ArtifactKind, INLINE_ARTIFACT_BYTES, MAX_ARTIFACT_BYTES,
    NewArtifact, PlanStatus, SessionConfig, SessionStore, digest,
};
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use std::{path::PathBuf, sync::Arc};

const PASSPHRASE: &str = "fixture-passphrase";

struct Fixture {
    dir: tempfile::TempDir,
    vault: Arc<VaultService>,
    store: SessionStore,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let vault = Arc::new(
            VaultService::open(
                dir.path().join("vault"),
                VaultServiceConfig {
                    kdf: KdfParams::new(8, 1, 1).unwrap(),
                    idle_timeout: None,
                },
            )
            .unwrap(),
        );
        vault
            .init(&SecretString::from(PASSPHRASE.to_owned()), false)
            .unwrap();
        let store = SessionStore::open(dir.path().join("native.sqlite3"), vault.clone()).unwrap();
        Self { dir, vault, store }
    }
    fn database(&self) -> PathBuf {
        self.dir.path().join("native.sqlite3")
    }
    fn blob_dir(&self) -> PathBuf {
        self.dir.path().join("native-artifacts")
    }
    fn blob_files(&self) -> Vec<PathBuf> {
        std::fs::read_dir(self.blob_dir())
            .map(|entries| entries.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }
    fn session(&self) -> String {
        self.store
            .create(
                serde_json::from_value::<SessionConfig>(
                    serde_json::json!({"title":"fixture","provider_id":"fixture","model_id":"small"}),
                )
                .unwrap(),
            )
            .unwrap()
            .id
    }
    fn unlock(&self) {
        self.vault
            .unlock(&SecretString::from(PASSPHRASE.to_owned()))
            .unwrap();
    }
}

fn artifact(name: &str, kind: ArtifactKind, bytes: Vec<u8>) -> NewArtifact {
    NewArtifact {
        name: name.into(),
        kind,
        mime: kind.default_mime().into(),
        language: None,
        bytes,
    }
}

/// Read every page of an artifact, checking the paging contract on the way.
fn read_all(store: &SessionStore, id: &str, limit: usize) -> Vec<u8> {
    let mut out = vec![];
    let mut offset = 0;
    loop {
        let page = store.read_artifact(id, offset, limit).unwrap();
        assert_eq!(page.offset, offset);
        assert!(page.bytes.len() <= limit.clamp(4, ARTIFACT_PAGE_BYTES));
        if page.encoding == ArtifactEncoding::Utf8 {
            assert!(
                std::str::from_utf8(&page.bytes).is_ok(),
                "page splits a character"
            );
        }
        out.extend_from_slice(&page.bytes);
        match page.next_offset {
            Some(next) => {
                assert_eq!(next, offset + page.bytes.len() as u64);
                assert!(next > offset);
                offset = next;
            }
            None => {
                assert_eq!(out.len() as u64, page.total_bytes);
                return out;
            }
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn artifacts_are_encrypted_at_rest_and_read_back_in_pages() {
    let f = Fixture::new();
    let session = f.session();
    let run = f
        .store
        .begin(&session, "make a report", "cmd-1", None)
        .unwrap();
    let inline_text = "private inline artifact marker\n".repeat(64);
    let inline = f
        .store
        .create_artifact(
            &session,
            Some(&run.id),
            NewArtifact {
                name: "secret report name".into(),
                kind: ArtifactKind::Document,
                mime: "text/markdown; charset=utf-8".into(),
                language: Some("Markdown".into()),
                bytes: inline_text.clone().into_bytes(),
            },
        )
        .unwrap();
    assert!(inline.id.starts_with("art_"));
    assert_eq!(inline.run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(inline.mime, "text/markdown");
    assert_eq!(inline.language.as_deref(), Some("markdown"));
    assert_eq!(inline.size, inline_text.len() as u64);
    assert_eq!(inline.sha256, digest(inline_text.as_bytes()));
    assert_eq!(inline.encoding, ArtifactEncoding::Utf8);
    assert_eq!(inline.plan_status, None);
    assert!(f.blob_files().is_empty(), "small artifacts stay inline");

    let mut large = b"private blob artifact marker ".repeat(50_000);
    large.push(0xff);
    let blob = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("dump.bin", ArtifactKind::Data, large.clone()),
        )
        .unwrap();
    assert_eq!(blob.encoding, ArtifactEncoding::Base64);
    assert!(blob.size > INLINE_ARTIFACT_BYTES as u64);
    assert_eq!(f.blob_files().len(), 1);

    let database = std::fs::read(f.database()).unwrap();
    for secret in [
        &b"private inline artifact marker"[..],
        b"private blob artifact marker",
        b"secret report name",
        b"dump.bin",
        b"text/markdown",
        inline.sha256.as_bytes(),
        blob.sha256.as_bytes(),
    ] {
        assert!(!contains(&database, secret), "database leaks {secret:?}");
        for file in f.blob_files() {
            assert!(!contains(&std::fs::read(file).unwrap(), secret));
        }
    }

    assert_eq!(read_all(&f.store, &inline.id, 100), inline_text.as_bytes());
    assert_eq!(read_all(&f.store, &blob.id, ARTIFACT_PAGE_BYTES), large);
    assert_eq!(read_all(&f.store, &blob.id, 70_001), large);
    let tail = f
        .store
        .read_artifact(&blob.id, blob.size - 3, 1 << 20)
        .unwrap();
    assert_eq!(tail.bytes, large[large.len() - 3..]);
    assert_eq!(tail.next_offset, None);
    let end = f.store.read_artifact(&blob.id, blob.size, 10).unwrap();
    assert!(end.bytes.is_empty() && end.next_offset.is_none());
    assert!(f.store.read_artifact(&blob.id, blob.size + 1, 10).is_err());
    assert!(f.store.read_artifact(&blob.id, 0, 0).is_err());

    let page = f.store.list_artifacts(&session, 0, 1).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.artifacts[0].id, blob.id, "newest first");
    assert_eq!(page.next_offset, Some(1));
    let rest = f.store.list_artifacts(&session, 1, 100).unwrap();
    assert_eq!(rest.artifacts, vec![inline.clone()]);
    assert_eq!(rest.next_offset, None);
    assert!(f.store.list_artifacts(&session, 0, 0).is_err());
    assert!(f.store.list_artifacts(&session, 0, 101).is_err());
    assert!(f.store.list_artifacts("session_missing", 0, 10).is_err());

    let database = f.database();
    drop(f.store);
    let reopened = SessionStore::open(database, f.vault.clone()).unwrap();
    assert_eq!(reopened.artifact(&inline.id).unwrap(), inline);
    assert_eq!(read_all(&reopened, &blob.id, ARTIFACT_PAGE_BYTES), large);
}

#[test]
fn utf8_pages_never_split_characters() {
    let f = Fixture::new();
    let session = f.session();
    let create = |text: &str| {
        f.store
            .create_artifact(
                &session,
                None,
                artifact(
                    "unicode.md",
                    ArtifactKind::Document,
                    text.as_bytes().to_vec(),
                ),
            )
            .unwrap()
    };
    let small = "aé€😀".repeat(100);
    let meta = create(&small);
    for limit in [1, 2, 3, 5, 7, 64] {
        assert_eq!(
            read_all(&f.store, &meta.id, limit),
            small.as_bytes(),
            "limit {limit}"
        );
    }
    // Pages that straddle the 64 KiB encryption segments.
    let large = "aé€😀".repeat(40_000);
    let spanning = create(&large);
    for limit in [4093, ARTIFACT_PAGE_BYTES] {
        assert_eq!(
            read_all(&f.store, &spanning.id, limit),
            large.as_bytes(),
            "limit {limit}"
        );
    }
    let page = f.store.read_artifact(&meta.id, 0, 2).unwrap();
    assert_eq!(page.bytes, b"a");
    let progress = f.store.read_artifact(&meta.id, 3, 1).unwrap();
    assert_eq!(
        progress.bytes,
        "€".as_bytes(),
        "a page always makes progress"
    );
    assert!(
        f.store.read_artifact(&meta.id, 2, 10).is_err(),
        "an offset inside a character is refused"
    );
}

#[test]
fn size_limits_and_storage_thresholds_hold() {
    let f = Fixture::new();
    let session = f.session();
    let exact_inline = f
        .store
        .create_artifact(
            &session,
            None,
            artifact(
                "inline",
                ArtifactKind::Data,
                vec![b'a'; INLINE_ARTIFACT_BYTES],
            ),
        )
        .unwrap();
    assert!(f.blob_files().is_empty(), "exactly 1 MiB stays inline");
    let max = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("max", ArtifactKind::Other, vec![7u8; MAX_ARTIFACT_BYTES]),
        )
        .unwrap();
    assert_eq!(max.size, MAX_ARTIFACT_BYTES as u64);
    assert_eq!(f.blob_files().len(), 1);
    let last = f
        .store
        .read_artifact(&max.id, max.size - 10, ARTIFACT_PAGE_BYTES)
        .unwrap();
    assert_eq!(last.bytes, vec![7u8; 10]);
    let error = f
        .store
        .create_artifact(
            &session,
            None,
            artifact(
                "too big",
                ArtifactKind::Other,
                vec![0; MAX_ARTIFACT_BYTES + 1],
            ),
        )
        .unwrap_err();
    assert!(error.contains("invalid artifact size"), "{error}");
    assert!(
        f.store
            .update_artifact(&exact_inline.id, vec![0; MAX_ARTIFACT_BYTES + 1])
            .is_err()
    );
    assert_eq!(f.store.list_artifacts(&session, 0, 100).unwrap().total, 2);
    assert_eq!(f.blob_files().len(), 1, "refused writes leave no blobs");
    let empty = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("empty", ArtifactKind::Document, vec![]),
        )
        .unwrap();
    let page = f.store.read_artifact(&empty.id, 0, 10).unwrap();
    assert!(page.bytes.is_empty() && page.next_offset.is_none() && page.total_bytes == 0);
    for bad in [
        NewArtifact {
            mime: "not a mime".into(),
            ..artifact("x", ArtifactKind::Code, vec![1])
        },
        artifact(" ", ArtifactKind::Code, vec![1]),
    ] {
        assert!(f.store.create_artifact(&session, None, bad).is_err());
    }
}

#[test]
fn plans_supersede_the_previous_active_plan() {
    let f = Fixture::new();
    let session = f.session();
    let other_session = f.session();
    let plan = |s: &str, title: &str| {
        f.store
            .create_artifact(
                s,
                None,
                artifact(title, ArtifactKind::Plan, format!("# {title}").into_bytes()),
            )
            .unwrap()
    };
    let first = plan(&session, "first");
    let elsewhere = plan(&other_session, "elsewhere");
    assert_eq!(first.plan_status, Some(PlanStatus::Active));
    let note = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("note", ArtifactKind::Document, b"note".to_vec()),
        )
        .unwrap();
    assert_eq!(note.plan_status, None);
    assert!(
        f.store
            .set_plan_status(&note.id, PlanStatus::Completed)
            .is_err()
    );

    let second = plan(&session, "second");
    let status = |id: &str| f.store.artifact(id).unwrap().plan_status;
    assert_eq!(status(&first.id), Some(PlanStatus::Superseded));
    assert_eq!(status(&second.id), Some(PlanStatus::Active));
    assert_eq!(
        status(&elsewhere.id),
        Some(PlanStatus::Active),
        "other sessions are untouched"
    );

    let completed = f
        .store
        .set_plan_status(&second.id, PlanStatus::Completed)
        .unwrap();
    assert_eq!(completed.plan_status, Some(PlanStatus::Completed));
    assert!(completed.updated_at >= second.updated_at);
    let third = plan(&session, "third");
    assert_eq!(
        status(&second.id),
        Some(PlanStatus::Completed),
        "completed plans stay completed"
    );
    assert_eq!(status(&third.id), Some(PlanStatus::Active));

    f.store
        .set_plan_status(&first.id, PlanStatus::Active)
        .unwrap();
    assert_eq!(status(&first.id), Some(PlanStatus::Active));
    assert_eq!(status(&third.id), Some(PlanStatus::Superseded));
    let updated = f
        .store
        .update_artifact(&first.id, b"# first, revised".to_vec())
        .unwrap();
    assert_eq!(
        updated.plan_status,
        Some(PlanStatus::Active),
        "edits keep the status"
    );
    let active = f
        .store
        .list_artifacts(&session, 0, 100)
        .unwrap()
        .artifacts
        .into_iter()
        .filter(|a| a.plan_status == Some(PlanStatus::Active))
        .count();
    assert_eq!(active, 1);
}

#[test]
fn updates_replace_content_and_move_between_inline_and_blob_storage() {
    let f = Fixture::new();
    let session = f.session();
    let meta = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("draft.rs", ArtifactKind::Code, b"fn draft() {}".to_vec()),
        )
        .unwrap();
    let big = vec![b'x'; INLINE_ARTIFACT_BYTES + 1];
    let grown = f.store.update_artifact(&meta.id, big.clone()).unwrap();
    assert_eq!(
        (grown.id.as_str(), grown.name.as_str()),
        (meta.id.as_str(), "draft.rs")
    );
    assert_eq!(grown.size, big.len() as u64);
    assert_eq!(grown.sha256, digest(&big));
    assert_eq!(grown.created_at, meta.created_at);
    assert_eq!(f.blob_files().len(), 1);
    assert_eq!(read_all(&f.store, &meta.id, ARTIFACT_PAGE_BYTES), big);
    let regrown = f.store.update_artifact(&meta.id, big.clone()).unwrap();
    assert_eq!(regrown.sha256, grown.sha256);
    assert_eq!(f.blob_files().len(), 1, "the previous blob is removed");
    let shrunk = f
        .store
        .update_artifact(&meta.id, b"fn done() {}".to_vec())
        .unwrap();
    assert_eq!(shrunk.size, 12);
    assert!(f.blob_files().is_empty());
    assert_eq!(read_all(&f.store, &meta.id, 5), b"fn done() {}");
    assert!(
        f.store
            .update_artifact("art_missing", b"x".to_vec())
            .is_err()
    );
    assert!(f.store.update_artifact("art_missing", big).is_err());
    assert!(f.blob_files().is_empty(), "failed updates leave no blobs");
}

#[test]
fn deleting_artifacts_and_sessions_removes_rows_and_blobs() {
    let f = Fixture::new();
    let session = f.session();
    let keep = f.session();
    let big = |s: &str| {
        f.store
            .create_artifact(
                s,
                None,
                artifact(
                    "big",
                    ArtifactKind::WebPage,
                    vec![b'w'; INLINE_ARTIFACT_BYTES + 10],
                ),
            )
            .unwrap()
    };
    let small = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("small", ArtifactKind::Code, b"x".to_vec()),
        )
        .unwrap();
    let doomed = big(&session);
    let kept = big(&keep);
    assert_eq!(f.blob_files().len(), 2);
    assert!(f.store.delete_artifact(&small.id).unwrap());
    assert!(!f.store.delete_artifact(&small.id).unwrap());
    assert!(
        f.store
            .artifact(&small.id)
            .unwrap_err()
            .contains("not found")
    );
    assert!(f.store.read_artifact(&small.id, 0, 10).is_err());
    let _ = big(&session);
    assert_eq!(f.blob_files().len(), 3);
    assert!(f.store.delete_artifact(&doomed.id).unwrap());
    assert_eq!(f.blob_files().len(), 2);
    assert!(f.store.delete(&session).unwrap());
    assert_eq!(
        f.blob_files().len(),
        1,
        "session deletion removes its artifacts"
    );
    assert!(f.store.list_artifacts(&session, 0, 10).is_err());
    assert_eq!(
        f.store.list_artifacts(&keep, 0, 10).unwrap().artifacts,
        vec![kept]
    );
}

#[test]
fn a_locked_vault_makes_artifacts_unavailable() {
    let f = Fixture::new();
    let session = f.session();
    let plan = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("plan", ArtifactKind::Plan, b"# plan".to_vec()),
        )
        .unwrap();
    f.vault.lock();
    let locked = |result: Result<(), String>| {
        let error = result.unwrap_err();
        assert!(error.contains("vault is locked"), "{error}");
    };
    locked(
        f.store
            .create_artifact(
                &session,
                None,
                artifact("n", ArtifactKind::Code, b"x".to_vec()),
            )
            .map(drop),
    );
    locked(f.store.list_artifacts(&session, 0, 10).map(drop));
    locked(f.store.artifact(&plan.id).map(drop));
    locked(f.store.read_artifact(&plan.id, 0, 10).map(drop));
    locked(f.store.update_artifact(&plan.id, b"y".to_vec()).map(drop));
    locked(
        f.store
            .set_plan_status(&plan.id, PlanStatus::Completed)
            .map(drop),
    );
    locked(f.store.delete_artifact(&plan.id).map(drop));
    f.unlock();
    assert_eq!(read_all(&f.store, &plan.id, 10), b"# plan");
    assert_eq!(f.store.list_artifacts(&session, 0, 10).unwrap().total, 1);
}

#[test]
fn foreign_runs_and_tampered_bytes_are_refused() {
    let f = Fixture::new();
    let session = f.session();
    let other = f.session();
    let foreign = f.store.begin(&other, "elsewhere", "cmd", None).unwrap();
    let error = f
        .store
        .create_artifact(
            &session,
            Some(&foreign.id),
            artifact("x", ArtifactKind::Code, b"x".to_vec()),
        )
        .unwrap_err();
    assert!(error.contains("run not found"), "{error}");
    assert!(
        f.store
            .create_artifact(
                "session_missing",
                None,
                artifact("x", ArtifactKind::Code, b"x".to_vec())
            )
            .unwrap_err()
            .contains("session not found")
    );
    assert!(
        f.store
            .create_artifact(
                &session,
                None,
                artifact("x", ArtifactKind::Code, vec![1; INLINE_ARTIFACT_BYTES + 1])
            )
            .is_ok()
    );
    let blob = f.blob_files().pop().unwrap();
    let meta = f.store.list_artifacts(&session, 0, 1).unwrap().artifacts[0].clone();
    let mut bytes = std::fs::read(&blob).unwrap();
    bytes[30] ^= 1;
    std::fs::write(&blob, &bytes).unwrap();
    let error = f.store.read_artifact(&meta.id, 0, 10).unwrap_err();
    assert!(error.contains("authentication failed"), "{error}");
    bytes.pop();
    std::fs::write(&blob, &bytes).unwrap();
    assert!(f.store.read_artifact(&meta.id, 0, 10).is_err());
    let db = rusqlite::Connection::open(f.database()).unwrap();
    db.execute(
        "UPDATE artifacts SET blob='../native.sqlite3' WHERE id=?",
        [meta.id.as_str()],
    )
    .unwrap();
    let error = f.store.read_artifact(&meta.id, 0, 10).unwrap_err();
    assert!(
        !error.starts_with("invalid artifact"),
        "storage damage is not a caller mistake: {error}"
    );

    let inline = f
        .store
        .create_artifact(
            &session,
            None,
            artifact("inline", ArtifactKind::Code, b"inline bytes".to_vec()),
        )
        .unwrap();
    db.execute(
        "UPDATE artifacts SET session_id=? WHERE id=?",
        [other.as_str(), inline.id.as_str()],
    )
    .unwrap();
    assert!(
        f.store.artifact(&inline.id).is_err(),
        "rows cannot move between sessions"
    );
}

#[test]
fn artifacts_of_sessions_deleted_by_an_older_daemon_are_purged_on_open() {
    let f = Fixture::new();
    let session = f.session();
    let keep = f.session();
    let new = |s: &str, bytes: Vec<u8>| {
        f.store
            .create_artifact(s, None, artifact("a", ArtifactKind::WebPage, bytes))
            .unwrap()
    };
    let inline = new(&session, b"inline".to_vec());
    let big = new(&session, vec![b'w'; INLINE_ARTIFACT_BYTES + 10]);
    let kept = new(&keep, vec![b'k'; INLINE_ARTIFACT_BYTES + 10]);
    assert_eq!(f.blob_files().len(), 2);
    // An older daemon's `SessionStore::delete` knows nothing of artifacts.
    let (database, blobs) = (f.database(), f.blob_dir());
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute("DELETE FROM runs WHERE session_id=?", [session.as_str()])
        .unwrap();
    db.execute("DELETE FROM sessions WHERE id=?", [session.as_str()])
        .unwrap();
    for id in [&inline.id, &big.id] {
        assert!(
            f.store.artifact(id).unwrap_err().contains("not found"),
            "rows of a deleted session are not readable by id"
        );
        assert!(f.store.read_artifact(id, 0, 10).is_err());
    }
    drop(f.store);
    let reopened = SessionStore::open(&database, f.vault.clone()).unwrap();
    let rows: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM artifacts WHERE session_id=?",
            [session.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(
        std::fs::read_dir(blobs).unwrap().count(),
        1,
        "the orphaned blob is swept"
    );
    assert_eq!(
        read_all(&reopened, &kept.id, ARTIFACT_PAGE_BYTES).len(),
        INLINE_ARTIFACT_BYTES + 10
    );
}

#[test]
fn orphaned_blobs_are_swept_and_older_tables_stay_readable() {
    let f = Fixture::new();
    let session = f.session();
    let meta = f
        .store
        .create_artifact(
            &session,
            None,
            artifact(
                "big",
                ArtifactKind::Data,
                vec![3; INLINE_ARTIFACT_BYTES + 1],
            ),
        )
        .unwrap();
    let (database, blobs) = (f.database(), f.blob_dir());
    let orphan = blobs.join("a".repeat(64));
    std::fs::write(&orphan, b"left behind").unwrap();
    std::fs::write(blobs.join("interrupted.tmp"), b"partial").unwrap();
    std::fs::write(blobs.join("README"), b"not ours").unwrap();
    drop(f.store);
    let reopened = SessionStore::open(database, f.vault.clone()).unwrap();
    assert!(!orphan.exists());
    assert!(!blobs.join("interrupted.tmp").exists());
    assert!(
        blobs.join("README").exists(),
        "unknown files are left alone"
    );
    assert_eq!(
        read_all(&reopened, &meta.id, ARTIFACT_PAGE_BYTES).len(),
        INLINE_ARTIFACT_BYTES + 1
    );

    // A database written before artifacts existed opens and gains the table.
    let legacy = f.dir.path().join("legacy.sqlite3");
    rusqlite::Connection::open(&legacy)
        .unwrap()
        .execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY,status TEXT NOT NULL,payload BLOB NOT NULL); CREATE TABLE runs(id TEXT PRIMARY KEY, session_id TEXT NOT NULL,status TEXT NOT NULL); CREATE TABLE memory(id TEXT PRIMARY KEY,workspace TEXT NOT NULL,payload BLOB NOT NULL);")
        .unwrap();
    let upgraded = SessionStore::open(&legacy, f.vault.clone()).unwrap();
    let created = upgraded
        .create(
            serde_json::from_value(serde_json::json!({"provider_id":"fixture","model_id":"small"}))
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        upgraded.list_artifacts(&created.id, 0, 10).unwrap().total,
        0
    );
}
