use aidex_native_sessions::{
    ATTACHMENT_ONLY_CONTENT, ATTACHMENT_PAGE_BYTES, ATTACHMENT_TEXT_PAGE_BYTES, ArtifactKind,
    AttachmentEncoding, AttachmentKind, AttachmentReason, AttachmentSource, AttachmentState,
    ForkRequest, INLINE_ATTACHMENT_BYTES, MAX_INFLIGHT_UPLOADS, MAX_STAGED_BYTES,
    MAX_STAGED_PER_SESSION, MAX_UPLOAD_BYTES, Mode, NewArtifact, NewAttachment, NewUpload,
    PermissionMode, RunOptions, RunStatus, SessionConfig, SessionStore, UPLOAD_CHUNK_BYTES, digest,
};
use aidex_providers::{ChatMessage, Role, ToolCall};
use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

const PASSPHRASE: &str = "fixture-passphrase";
const SECRET_NAME: &str = "quarterly-secret-plan.txt";
const SECRET_TEXT: &str = "private attachment marker text\n";

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
    fn attachment_dir(&self) -> PathBuf {
        self.dir.path().join("native-attachments")
    }
    fn artifact_dir(&self) -> PathBuf {
        self.dir.path().join("native-artifacts")
    }
    fn session(&self) -> String {
        self.session_with(
            serde_json::json!({"title":"fixture","provider_id":"fixture","model_id":"small"}),
        )
    }
    fn session_with(&self, config: serde_json::Value) -> String {
        self.store
            .create(serde_json::from_value::<SessionConfig>(config).unwrap())
            .unwrap()
            .id
    }
    fn unlock(&self) {
        self.vault
            .unlock(&SecretString::from(PASSPHRASE.to_owned()))
            .unwrap();
    }
    fn reopen(self) -> Self {
        let Self { dir, vault, store } = self;
        drop(store);
        let store = SessionStore::open(dir.path().join("native.sqlite3"), vault.clone()).unwrap();
        Self { dir, vault, store }
    }
    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.database()).unwrap()
    }
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn tmp_files(dir: &Path) -> usize {
    files(dir).iter().filter(|n| n.ends_with(".tmp")).count()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn reason(error: &str) -> AttachmentReason {
    AttachmentReason::of(error)
        .unwrap_or_else(|| panic!("no reason in {error:?}"))
        .0
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    b.extend(13u32.to_be_bytes());
    b.extend(b"IHDR");
    b.extend(width.to_be_bytes());
    b.extend(height.to_be_bytes());
    b.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b.extend(0u32.to_be_bytes());
    b.extend(b"IDAT");
    b.extend([0; 4]);
    b
}

/// A one-page PDF padded with a stream of `padding` bytes.
fn pdf(padding: usize) -> Vec<u8> {
    let mut b = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n".to_vec();
    b.extend(format!("4 0 obj\n<< /Length {padding} >>\nstream\n").bytes());
    b.extend((0..padding).map(|i| (i % 251) as u8));
    b.extend(b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n");
    b
}

fn new_upload(session: &str, name: &str, bytes: &[u8]) -> NewUpload {
    NewUpload {
        session_id: session.into(),
        name: name.into(),
        size: bytes.len() as u64,
        sha256: digest(bytes),
        mime: Some("text/markdown".into()),
        source: Some(AttachmentSource::File),
    }
}

/// Upload `bytes` in protocol-sized chunks and commit.
fn upload(
    store: &SessionStore,
    session: &str,
    name: &str,
    bytes: &[u8],
) -> Result<aidex_native_sessions::AttachmentMeta, String> {
    let ticket = store.begin_attachment_upload(new_upload(session, name, bytes))?;
    assert_eq!(ticket.chunk_bytes, UPLOAD_CHUNK_BYTES);
    let mut offset = 0u64;
    for chunk in bytes.chunks(ticket.chunk_bytes) {
        offset = store.append_upload(&ticket.upload_id, offset, chunk)?;
    }
    store.commit_upload(&ticket.upload_id)
}

fn read_all(store: &SessionStore, id: &str, limit: usize) -> Vec<u8> {
    let mut out = vec![];
    let mut offset = 0;
    loop {
        let page = store.read_attachment(id, offset, limit).unwrap();
        assert_eq!(page.offset, offset);
        let cap = if page.encoding == AttachmentEncoding::Utf8 {
            ATTACHMENT_TEXT_PAGE_BYTES
        } else {
            ATTACHMENT_PAGE_BYTES
        };
        assert!(page.bytes.len() <= limit.min(cap).max(4));
        if page.encoding == AttachmentEncoding::Utf8 {
            assert!(
                std::str::from_utf8(&page.bytes).is_ok(),
                "page splits a character"
            );
        }
        out.extend_from_slice(&page.bytes);
        match page.next_offset {
            Some(next) => {
                assert!(next > offset);
                offset = next;
            }
            None => {
                assert_eq!(out.len() as u64, page.total_bytes);
                assert_eq!(digest(&out), page.sha256);
                return out;
            }
        }
    }
}

fn options<S: AsRef<str>>(ids: &[S]) -> RunOptions {
    RunOptions {
        attachments: ids.iter().map(|s| s.as_ref().to_string()).collect(),
        ..RunOptions::default()
    }
}

#[test]
fn uploads_are_sealed_at_rest_and_read_back() {
    let f = Fixture::new();
    let session = f.session();
    let text = SECRET_TEXT.repeat(1000);
    let note = upload(
        &f.store,
        &session,
        &format!("C:\\Users\\me\\{SECRET_NAME}"),
        text.as_bytes(),
    )
    .unwrap();
    assert_eq!(
        (
            note.kind,
            note.mime.as_str(),
            note.name.as_str(),
            note.state
        ),
        (
            AttachmentKind::Text,
            "text/markdown",
            SECRET_NAME,
            AttachmentState::Staged
        )
    );
    assert_eq!(note.source, Some(AttachmentSource::File));
    assert!(note.id.starts_with("att_"));
    let image = upload(&f.store, &session, "photo.png", &png(1920, 1080)).unwrap();
    assert_eq!(
        (image.kind, image.mime.as_str(), image.width, image.height),
        (AttachmentKind::Image, "image/png", Some(1920), Some(1080))
    );
    let document = pdf(INLINE_ATTACHMENT_BYTES + 300_000);
    let big = upload(&f.store, &session, "big.pdf", &document).unwrap();
    assert_eq!((big.kind, big.pages), (AttachmentKind::Pdf, Some(1)));
    assert_eq!(
        files(&f.attachment_dir()).len(),
        1,
        "only the large PDF is a blob"
    );
    assert_eq!(tmp_files(&f.attachment_dir()), 0);

    assert_eq!(read_all(&f.store, &note.id, usize::MAX), text.as_bytes());
    assert_eq!(read_all(&f.store, &big.id, 100_000), document);
    assert_eq!(
        &*f.store.attachment_bytes(&big.id).unwrap().bytes,
        &document[..]
    );
    let page = f.store.read_attachment(&image.id, 0, usize::MAX).unwrap();
    assert_eq!(
        (page.encoding, page.bytes, page.next_offset),
        (AttachmentEncoding::Base64, png(1920, 1080), None)
    );
    let listed = f.store.list_attachments(&session, 0, 200).unwrap();
    assert_eq!(listed.total, 3);
    assert_eq!(listed.attachments[0].id, big.id, "newest first");
    assert_eq!(f.store.attachment(&note.id).unwrap(), note);

    let database = std::fs::read(f.database()).unwrap();
    let blob = std::fs::read(f.attachment_dir().join(&files(&f.attachment_dir())[0])).unwrap();
    for bytes in [&database, &blob] {
        assert!(
            !contains(bytes, b"quarterly-secret-plan"),
            "names are sealed"
        );
        assert!(
            !contains(bytes, b"private attachment marker"),
            "bytes are sealed"
        );
        assert!(!contains(bytes, b"text/markdown"), "metadata is sealed");
    }
    assert!(!contains(&blob, &document[200..400]));
    assert!(
        !format!("{note:?}").contains("quarterly"),
        "Debug hides the name"
    );
}

#[test]
fn chunks_are_sequential_fixed_size_and_idempotent() {
    let f = Fixture::new();
    let session = f.session();
    let bytes: Vec<u8> = "abcdefghij".repeat(60_000).into_bytes();
    let ticket = f
        .store
        .begin_attachment_upload(new_upload(&session, "notes.txt", &bytes))
        .unwrap();
    let id = ticket.upload_id.as_str();
    let chunk = UPLOAD_CHUNK_BYTES;
    let bad = |result: Result<u64, String>| reason(&result.unwrap_err());
    assert_eq!(
        bad(f.store.append_upload(id, 0, &bytes[..chunk - 1])),
        AttachmentReason::BadOffset
    );
    assert_eq!(
        bad(f
            .store
            .append_upload(id, chunk as u64, &bytes[chunk..2 * chunk])),
        AttachmentReason::BadOffset
    );
    assert_eq!(
        f.store.append_upload(id, 0, &bytes[..chunk]).unwrap(),
        chunk as u64
    );
    assert_eq!(
        f.store.append_upload(id, 0, &bytes[..chunk]).unwrap(),
        chunk as u64,
        "a retried chunk is acknowledged"
    );
    assert_eq!(
        bad(f.store.append_upload(id, 0, &bytes[..10])),
        AttachmentReason::BadOffset
    );
    assert_eq!(
        reason(&f.store.commit_upload(id).unwrap_err()),
        AttachmentReason::BadOffset,
        "an incomplete upload cannot commit and stays open"
    );
    assert_eq!(
        f.store
            .append_upload(id, chunk as u64, &bytes[chunk..2 * chunk])
            .unwrap(),
        2 * chunk as u64
    );
    assert_eq!(
        bad(f.store.append_upload(
            id,
            2 * chunk as u64,
            &[bytes.as_slice(), b"x"].concat()[2 * chunk..]
        )),
        AttachmentReason::BadOffset
    );
    assert_eq!(
        f.store
            .append_upload(id, 2 * chunk as u64, &bytes[2 * chunk..])
            .unwrap(),
        bytes.len() as u64
    );
    assert_eq!(
        bad(f.store.append_upload(id, bytes.len() as u64, b"x")),
        AttachmentReason::BadOffset
    );
    let meta = f.store.commit_upload(id).unwrap();
    assert_eq!(read_all(&f.store, &meta.id, 70_000), bytes);
    assert!(f.store.commit_upload(id).unwrap_err().contains("not found"));
    assert!(
        f.store
            .append_upload(id, 0, &bytes[..chunk])
            .unwrap_err()
            .contains("not found")
    );
}

#[test]
fn hash_mismatch_kind_refusals_and_abort_leave_nothing_behind() {
    let f = Fixture::new();
    let session = f.session();
    let mut wrong = new_upload(&session, "a.txt", b"hello");
    wrong.sha256 = digest(b"other");
    let ticket = f.store.begin_attachment_upload(wrong).unwrap();
    f.store
        .append_upload(&ticket.upload_id, 0, b"hello")
        .unwrap();
    assert_eq!(tmp_files(&f.attachment_dir()), 1);
    assert_eq!(
        reason(&f.store.commit_upload(&ticket.upload_id).unwrap_err()),
        AttachmentReason::HashMismatch
    );
    assert_eq!(tmp_files(&f.attachment_dir()), 0);

    for (bytes, expected) in [
        (b"BM\0\0binary".to_vec(), AttachmentReason::UnsupportedType),
        (
            b"-----BEGIN RSA PRIVATE KEY-----\nMIIE\n".to_vec(),
            AttachmentReason::SensitiveContent,
        ),
        (png(9000, 10), AttachmentReason::TooLarge),
    ] {
        assert_eq!(
            reason(&upload(&f.store, &session, "x", &bytes).unwrap_err()),
            expected
        );
    }
    assert_eq!(tmp_files(&f.attachment_dir()), 0);
    assert_eq!(f.store.list_attachments(&session, 0, 10).unwrap().total, 0);

    let ticket = f
        .store
        .begin_attachment_upload(new_upload(&session, "a.txt", b"hello"))
        .unwrap();
    assert_eq!(tmp_files(&f.attachment_dir()), 1);
    assert!(f.store.abort_upload(&ticket.upload_id));
    assert!(!f.store.abort_upload(&ticket.upload_id));
    assert_eq!(tmp_files(&f.attachment_dir()), 0);

    let mut empty = new_upload(&session, "a.txt", b"");
    empty.size = 0;
    assert_eq!(
        reason(&f.store.begin_attachment_upload(empty).unwrap_err()),
        AttachmentReason::TooLarge
    );
    let mut huge = new_upload(&session, "a.txt", b"x");
    huge.size = MAX_UPLOAD_BYTES + 1;
    assert_eq!(
        reason(&f.store.begin_attachment_upload(huge).unwrap_err()),
        AttachmentReason::TooLarge
    );
    assert_eq!(
        reason(
            &f.store
                .begin_attachment_upload(new_upload(&session, "bad\u{202E}name", b"x"))
                .unwrap_err()
        ),
        AttachmentReason::InvalidName
    );
    let mut sha = new_upload(&session, "a.txt", b"x");
    sha.sha256 = "zz".into();
    assert_eq!(
        reason(&f.store.begin_attachment_upload(sha).unwrap_err()),
        AttachmentReason::InvalidAttachment
    );
    assert!(
        f.store
            .begin_attachment_upload(new_upload("session_missing", "a.txt", b"x"))
            .unwrap_err()
            .contains("session not found")
    );
    assert_eq!(tmp_files(&f.attachment_dir()), 0);
}

#[test]
fn idle_uploads_expire_and_in_flight_caps_hold() {
    let f = Fixture::new();
    let session = f.session();
    let ticket = f
        .store
        .begin_attachment_upload(new_upload(&session, "a.txt", b"hello"))
        .unwrap();
    f.store.set_upload_idle_timeout(Duration::ZERO);
    let other = f.session();
    // Any upload call evicts expired uploads first.
    let _ = f
        .store
        .begin_attachment_upload(new_upload(&other, "b.txt", b"x"));
    assert!(
        f.store
            .append_upload(&ticket.upload_id, 0, b"hello")
            .unwrap_err()
            .contains("not found")
    );
    assert_eq!(tmp_files(&f.attachment_dir()), 0);
    f.store.set_upload_idle_timeout(Duration::from_secs(600));

    let mut tickets = vec![];
    for n in 0..MAX_INFLIGHT_UPLOADS {
        tickets.push(
            f.store
                .begin_attachment_upload(new_upload(&session, &format!("{n}.txt"), b"x"))
                .unwrap(),
        );
    }
    assert_eq!(
        reason(
            &f.store
                .begin_attachment_upload(new_upload(&session, "9.txt", b"x"))
                .unwrap_err()
        ),
        AttachmentReason::Busy
    );
    assert_eq!(f.store.abort_all_uploads(), MAX_INFLIGHT_UPLOADS);
    assert_eq!(tmp_files(&f.attachment_dir()), 0);
    let mut big = new_upload(&session, "big.pdf", b"x");
    big.size = MAX_UPLOAD_BYTES;
    for _ in 0..4 {
        f.store.begin_attachment_upload(big.clone()).unwrap();
    }
    assert_eq!(
        reason(
            &f.store
                .begin_attachment_upload(new_upload(&session, "a.txt", b"x"))
                .unwrap_err()
        ),
        AttachmentReason::Busy
    );
    f.store.abort_all_uploads();

    // Drive-wide: staged plus in-flight declared bytes are capped.
    let staged = f
        .store
        .create_attachment(
            &session,
            NewAttachment {
                name: "n.txt".into(),
                mime: None,
                source: None,
                bytes: Zeroizing::new(b"x".to_vec()),
            },
        )
        .unwrap();
    f.db().execute("INSERT INTO attachments(id,session_id,state,created_at,size,meta) VALUES('att_fake',?,'staged',?,?,x'00')", rusqlite::params![session, staged.created_at as i64, 255 * 1024 * 1024i64]).unwrap();
    assert_eq!(
        reason(&f.store.begin_attachment_upload(big.clone()).unwrap_err()),
        AttachmentReason::TooManyAttachments
    );
    let listed = f.store.list_attachments(&session, 0, 10).unwrap();
    assert_eq!(
        (listed.total, listed.attachments.len()),
        (2, 1),
        "an unreadable row is skipped, not fatal"
    );
    f.db()
        .execute("DELETE FROM attachments WHERE id='att_fake'", [])
        .unwrap();

    // Per session: staged plus in-flight uploads of the session.
    for n in 1..MAX_STAGED_PER_SESSION - 1 {
        f.store
            .create_attachment(
                &session,
                NewAttachment {
                    name: format!("{n}.txt"),
                    mime: None,
                    source: None,
                    bytes: Zeroizing::new(b"x".to_vec()),
                },
            )
            .unwrap();
    }
    let last = f
        .store
        .begin_attachment_upload(new_upload(&session, "last.txt", b"x"))
        .unwrap();
    assert_eq!(
        reason(
            &f.store
                .begin_attachment_upload(new_upload(&session, "more.txt", b"x"))
                .unwrap_err()
        ),
        AttachmentReason::TooManyAttachments
    );
    assert_eq!(
        reason(
            &f.store
                .create_attachment(
                    &session,
                    NewAttachment {
                        name: "w.txt".into(),
                        mime: None,
                        source: None,
                        bytes: Zeroizing::new(b"x".to_vec())
                    }
                )
                .unwrap_err()
        ),
        AttachmentReason::TooManyAttachments
    );
    f.store.append_upload(&last.upload_id, 0, b"x").unwrap();
    f.store.commit_upload(&last.upload_id).unwrap();
    assert!(
        f.store
            .begin_attachment_upload(new_upload(&other, "fine.txt", b"x"))
            .is_ok()
    );
}

#[test]
fn a_locked_vault_stops_uploads_and_abort_still_works() {
    let f = Fixture::new();
    let session = f.session();
    let bytes = vec![b'a'; UPLOAD_CHUNK_BYTES + 10];
    let chunk = UPLOAD_CHUNK_BYTES;

    // vault.lock: the next chunk fails and the upload's file is gone.
    let ticket = f
        .store
        .begin_attachment_upload(new_upload(&session, "a.txt", &bytes))
        .unwrap();
    f.store
        .append_upload(&ticket.upload_id, 0, &bytes[..chunk])
        .unwrap();
    f.vault.lock();
    let error = f
        .store
        .append_upload(&ticket.upload_id, chunk as u64, &bytes[chunk..])
        .unwrap_err();
    assert!(error.contains("vault is locked"), "{error}");
    assert_eq!(tmp_files(&f.attachment_dir()), 0);
    assert!(
        !f.store.abort_upload(&ticket.upload_id),
        "abort needs no key"
    );
    assert!(
        f.store
            .begin_attachment_upload(new_upload(&session, "a.txt", &bytes))
            .unwrap_err()
            .contains("vault is locked")
    );
    assert!(
        f.store
            .attachment("att_x")
            .unwrap_err()
            .contains("vault is locked")
    );
    assert!(
        f.store
            .list_attachments(&session, 0, 10)
            .unwrap_err()
            .contains("vault is locked")
    );
    f.unlock();

    // The daemon's lock handler (and emergency stop) call abort_all_uploads.
    let ticket = f
        .store
        .begin_attachment_upload(new_upload(&session, "a.txt", &bytes))
        .unwrap();
    f.store
        .append_upload(&ticket.upload_id, 0, &bytes[..chunk])
        .unwrap();
    f.vault.lock();
    assert_eq!(f.store.abort_all_uploads(), 1);
    assert_eq!(tmp_files(&f.attachment_dir()), 0);
    f.unlock();

    // The idle auto-lock bypasses the daemon: the next chunk still fails closed.
    let ticket = f
        .store
        .begin_attachment_upload(new_upload(&session, "a.txt", &bytes))
        .unwrap();
    f.store
        .append_upload(&ticket.upload_id, 0, &bytes[..chunk])
        .unwrap();
    f.vault.set_idle_timeout(Some(Duration::from_millis(1)));
    std::thread::sleep(Duration::from_millis(30));
    let error = f.store.commit_upload(&ticket.upload_id).unwrap_err();
    assert_eq!(
        reason(&error),
        AttachmentReason::BadOffset,
        "incomplete before any key is needed"
    );
    let error = f
        .store
        .append_upload(&ticket.upload_id, chunk as u64, &bytes[chunk..])
        .unwrap_err();
    assert!(error.contains("vault is locked"), "{error}");
    assert_eq!(tmp_files(&f.attachment_dir()), 0);
    assert!(!f.store.abort_upload(&ticket.upload_id));
    f.vault.set_idle_timeout(None);
    f.unlock();
    assert!(upload(&f.store, &session, "a.txt", &bytes).is_ok());
}

#[test]
fn messages_link_staged_attachments_in_one_transaction() {
    let f = Fixture::new();
    let session = f.session_with(serde_json::json!({"provider_id":"fixture","model_id":"small","mode":"agent","workspace_id":"workspace_1","permission_mode":"auto_accept_edits"}));
    let a = upload(&f.store, &session, "a.png", &png(10, 10)).unwrap();
    let b = upload(&f.store, &session, "b.txt", b"notes").unwrap();
    let c = upload(&f.store, &session, "c.txt", b"later").unwrap();
    let other = f.session();
    let foreign = upload(&f.store, &other, "f.txt", b"elsewhere").unwrap();
    let unavailable = |opts: RunOptions, command: &str| {
        reason(
            &f.store
                .begin_with(&session, "hi", command, None, &opts)
                .unwrap_err(),
        )
    };
    assert_eq!(
        unavailable(options(&[&a.id, &foreign.id]), "c1"),
        AttachmentReason::AttachmentUnavailable
    );
    assert_eq!(
        unavailable(options(&[&a.id, &a.id]), "c1"),
        AttachmentReason::AttachmentUnavailable
    );
    assert_eq!(
        unavailable(options(&["att_missing"]), "c1"),
        AttachmentReason::AttachmentUnavailable
    );
    let many: Vec<String> = (0..21).map(|n| format!("att_{n}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(
        unavailable(options(&many), "c1"),
        AttachmentReason::AttachmentUnavailable
    );
    assert_eq!(
        f.store.attachment(&a.id).unwrap().state,
        AttachmentState::Staged,
        "a refused send changes nothing"
    );
    assert!(f.store.get(&session).unwrap().messages.is_empty());
    assert!(
        f.store.begin(&session, " ", "blank", None).is_err(),
        "blank text needs attachments"
    );

    // The harness's pre-check applies the same rules and changes nothing.
    let checked = f
        .store
        .staged_attachments(&session, &[b.id.clone(), a.id.clone()])
        .unwrap();
    assert_eq!(
        checked
            .iter()
            .map(|m| (m.id.as_str(), m.kind))
            .collect::<Vec<_>>(),
        vec![
            (b.id.as_str(), AttachmentKind::Text),
            (a.id.as_str(), AttachmentKind::Image)
        ]
    );
    for ids in [
        vec![a.id.clone(), foreign.id.clone()],
        vec![a.id.clone(), a.id.clone()],
        many.iter().map(|s| s.to_string()).collect(),
    ] {
        assert_eq!(
            reason(&f.store.staged_attachments(&session, &ids).unwrap_err()),
            AttachmentReason::AttachmentUnavailable
        );
    }
    assert!(
        f.store
            .staged_attachments("session_missing", std::slice::from_ref(&a.id))
            .unwrap_err()
            .contains("not found")
    );

    // Order follows the request, not the upload.
    let run = f
        .store
        .begin_with(&session, "  ", "send-1", None, &options(&[&b.id, &a.id]))
        .unwrap();
    assert_eq!(run.attachments, vec![b.id.clone(), a.id.clone()]);
    assert_eq!(
        (run.mode, run.permission_mode, run.workspace_id.as_deref()),
        (
            Some(Mode::Agent),
            Some(PermissionMode::AutoAcceptEdits),
            Some("workspace_1")
        )
    );
    let detail = f.store.get(&session).unwrap();
    assert_eq!(
        detail.messages[0].content, ATTACHMENT_ONLY_CONTENT,
        "no blank user message is stored"
    );
    assert_eq!(
        run.transcript.last().unwrap().content,
        ATTACHMENT_ONLY_CONTENT
    );
    let linked = f.store.attachments_for_message(&session, 0).unwrap();
    assert_eq!(
        linked.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        vec![b.id.as_str(), a.id.as_str()]
    );
    for (position, meta) in linked.iter().enumerate() {
        assert_eq!(meta.state, AttachmentState::Sent);
        assert_eq!(
            (meta.run_id.as_deref(), meta.command_id.as_deref()),
            (Some(run.id.as_str()), Some("send-1"))
        );
        assert_eq!(
            (meta.message_index, meta.position),
            (Some(0), Some(position as u32))
        );
    }

    // A retry after a lost response returns the same run; nothing is re-checked.
    let again = f
        .store
        .begin_with(&session, "  ", "send-1", None, &options(&[&b.id, &a.id]))
        .unwrap();
    assert_eq!(again.id, run.id);
    assert!(
        f.store
            .begin_with(&session, "  ", "send-1", None, &options(&[&a.id, &b.id]))
            .unwrap_err()
            .contains("different arguments")
    );
    f.store
        .update(&session, |s| {
            s.runs[0].status = RunStatus::Completed;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        unavailable(options(&[&a.id]), "send-2"),
        AttachmentReason::AttachmentUnavailable
    );
    assert_eq!(
        reason(
            &f.store
                .staged_attachments(&session, std::slice::from_ref(&a.id))
                .unwrap_err()
        ),
        AttachmentReason::AttachmentUnavailable,
        "sent ids fail the pre-check (a retry skips it)"
    );
    assert_eq!(
        reason(
            &f.store
                .begin_with(
                    &session,
                    "child",
                    "child-1",
                    Some(run.id.clone()),
                    &options(&[&c.id])
                )
                .unwrap_err()
        ),
        AttachmentReason::AttachmentUnavailable
    );
    let second = f
        .store
        .begin_with(&session, "and this", "send-3", None, &options(&[&c.id]))
        .unwrap();
    assert_eq!(second.message_index, Some(1));
    let by_message = f.store.message_attachments(&session).unwrap();
    assert_eq!(by_message.keys().copied().collect::<Vec<_>>(), vec![0, 1]);
    assert_eq!(by_message[&1][0].id, c.id);

    // Sent attachments live with the conversation; staged ones can be removed.
    assert_eq!(
        reason(&f.store.delete_staged_attachment(&a.id).unwrap_err()),
        AttachmentReason::AttachmentSent
    );
    let staged = upload(
        &f.store,
        &session,
        "d.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 1),
    )
    .unwrap();
    assert_eq!(files(&f.attachment_dir()).len(), 1);
    assert!(f.store.delete_staged_attachment(&staged.id).unwrap());
    assert!(!f.store.delete_staged_attachment(&staged.id).unwrap());
    assert!(files(&f.attachment_dir()).is_empty());
    let excluded = f.store.set_attachment_excluded(&a.id, true).unwrap();
    assert!(excluded.excluded && !excluded.sendable());
    assert!(f.store.attachments_for_message(&session, 0).unwrap()[1].excluded);
}

#[test]
fn fingerprints_without_attachments_are_unchanged() {
    let f = Fixture::new();
    let session = f.session();
    let skills = RunOptions {
        skills: vec!["drive:review".into()],
        web_search: true,
        ..RunOptions::default()
    };
    f.store
        .begin_with(&session, "hi", "skills", None, &skills)
        .unwrap();
    f.store
        .update(&session, |s| {
            s.runs[0].status = RunStatus::Completed;
            Ok(())
        })
        .unwrap();
    f.store.begin(&session, "plain", "plain", None).unwrap();
    let detail = f.store.get(&session).unwrap();
    let parent: Option<String> = None;
    assert_eq!(
        detail.commands["skills"].fingerprint,
        digest(format!("hi\0{parent:?}\0{:?}\0{}", skills.skills, true).as_bytes())
    );
    assert_eq!(
        detail.commands["plain"].fingerprint,
        digest(format!("plain\0{parent:?}").as_bytes())
    );
    assert!(
        !RunOptions {
            attachments: vec!["att_1".into()],
            ..RunOptions::default()
        }
        .is_empty()
    );
    let value = serde_json::to_value(&detail.runs[1]).unwrap();
    assert!(
        value.get("attachments").is_none(),
        "empty caches are not serialized"
    );
}

#[test]
fn deleting_a_conversation_removes_its_attachments_and_uploads() {
    let f = Fixture::new();
    let session = f.session();
    let keep = f.session();
    let big = upload(
        &f.store,
        &session,
        "big.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 5),
    )
    .unwrap();
    upload(&f.store, &session, "small.txt", b"small").unwrap();
    let kept = upload(
        &f.store,
        &keep,
        "kept.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 6),
    )
    .unwrap();
    f.store
        .begin_attachment_upload(new_upload(&session, "pending.txt", b"x"))
        .unwrap();
    assert_eq!(files(&f.attachment_dir()).len(), 3);
    assert!(f.store.delete(&session).unwrap());
    assert_eq!(
        files(&f.attachment_dir()).len(),
        1,
        "blobs and the pending upload file are gone"
    );
    assert!(
        f.store
            .attachment(&big.id)
            .unwrap_err()
            .contains("not found")
    );
    let rows: i64 = f
        .db()
        .query_row(
            "SELECT COUNT(*) FROM attachments WHERE session_id=?",
            [session.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(
        read_all(&f.store, &kept.id, ATTACHMENT_PAGE_BYTES).len() as u64,
        kept.size
    );
}

#[test]
fn an_older_daemon_deleting_a_conversation_is_cleaned_up_on_open_without_touching_artifacts() {
    let f = Fixture::new();
    let session = f.session();
    let keep = f.session();
    let big = upload(
        &f.store,
        &session,
        "big.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 5),
    )
    .unwrap();
    f.store
        .begin_with(&session, "", "send", None, &options(&[&big.id]))
        .unwrap();
    let artifact = f
        .store
        .create_artifact(
            &keep,
            None,
            NewArtifact {
                name: "a".into(),
                kind: ArtifactKind::Data,
                mime: "application/json".into(),
                language: None,
                bytes: vec![b'1'; INLINE_ATTACHMENT_BYTES + 3],
            },
        )
        .unwrap();
    let kept = upload(
        &f.store,
        &keep,
        "kept.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 7),
    )
    .unwrap();
    let artifacts_before = files(&f.artifact_dir());
    assert_eq!(artifacts_before.len(), 1);
    assert_eq!(files(&f.attachment_dir()).len(), 2);
    // The PR 2 daemon's delete knows nothing of attachments.
    let db = f.db();
    db.execute("DELETE FROM runs WHERE session_id=?", [session.as_str()])
        .unwrap();
    db.execute("DELETE FROM sessions WHERE id=?", [session.as_str()])
        .unwrap();
    assert!(
        f.store
            .attachment(&big.id)
            .unwrap_err()
            .contains("not found")
    );
    // Each sweep keeps to its own directory: a stray copy of an attachment
    // blob in the artifacts directory is swept there, and vice versa.
    let attachment_blob = files(&f.attachment_dir())
        .into_iter()
        .find(|n| n.len() == 64)
        .unwrap();
    std::fs::copy(
        f.attachment_dir().join(&attachment_blob),
        f.artifact_dir().join(&attachment_blob),
    )
    .unwrap();
    std::fs::copy(
        f.artifact_dir().join(&artifacts_before[0]),
        f.attachment_dir().join(&artifacts_before[0]),
    )
    .unwrap();
    std::fs::write(f.attachment_dir().join("interrupted.tmp"), b"sealed").unwrap();
    std::fs::write(f.attachment_dir().join("README"), b"not ours").unwrap();
    let f = f.reopen();
    let rows: i64 = f
        .db()
        .query_row(
            "SELECT COUNT(*) FROM attachments WHERE session_id=?",
            [session.as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
    assert_eq!(
        files(&f.artifact_dir()),
        artifacts_before,
        "the artifacts directory keeps only artifact blobs"
    );
    let left = files(&f.attachment_dir());
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.contains(&"README".to_string()));
    assert_eq!(
        read_all(&f.store, &kept.id, ATTACHMENT_PAGE_BYTES).len() as u64,
        kept.size
    );
    assert_eq!(
        f.store.read_artifact(&artifact.id, 0, 10).unwrap().bytes,
        b"1111111111"
    );
}

#[test]
fn stale_staged_attachments_are_purged_on_open_with_the_vault_locked() {
    let f = Fixture::new();
    let session = f.session();
    let stale = upload(
        &f.store,
        &session,
        "stale.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 5),
    )
    .unwrap();
    let sent = upload(
        &f.store,
        &session,
        "sent.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 9),
    )
    .unwrap();
    let fresh = upload(&f.store, &session, "fresh.txt", b"fresh").unwrap();
    f.store
        .begin_with(&session, "x", "send", None, &options(&[&sent.id]))
        .unwrap();
    let day = 25 * 60 * 60 * 1000i64;
    f.db()
        .execute(
            "UPDATE attachments SET created_at=created_at-? WHERE id IN (?,?)",
            rusqlite::params![day, stale.id, sent.id],
        )
        .unwrap();
    f.vault.lock();
    let f = f.reopen();
    let ids: Vec<String> = {
        let db = f.db();
        let mut stmt = db
            .prepare("SELECT id FROM attachments ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut expected = vec![sent.id.clone(), fresh.id.clone()];
    expected.sort();
    assert_eq!(ids, expected, "only the stale staged row is purged");
    assert_eq!(files(&f.attachment_dir()).len(), 1, "with its blob");
    f.unlock();
    assert_eq!(
        f.store.attachment(&fresh.id).unwrap().state,
        AttachmentState::Staged
    );
}

#[test]
fn a_running_daemon_expires_stale_staged_attachments_when_due() {
    let f = Fixture::new();
    let session = f.session();
    let stale = upload(
        &f.store,
        &session,
        "stale.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 5),
    )
    .unwrap();
    let fresh = upload(&f.store, &session, "fresh.txt", b"fresh").unwrap();
    let day = 25 * 60 * 60 * 1000i64;
    // A draft the client lost a day ago, holding every staged byte.
    let leak = |id: &str, age: i64| {
        f.db().execute("INSERT INTO attachments(id,session_id,state,created_at,size,meta) VALUES(?,?,'staged',?,?,x'00')", rusqlite::params![id, session, fresh.created_at as i64 - age, MAX_STAGED_BYTES as i64]).unwrap();
    };
    leak("att_leaked", day);
    f.db()
        .execute(
            "UPDATE attachments SET created_at=created_at-? WHERE id=?",
            rusqlite::params![day, stale.id],
        )
        .unwrap();
    let ids = || -> Vec<String> {
        let db = f.db();
        let mut stmt = db
            .prepare("SELECT id FROM attachments ORDER BY id")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let begin = || {
        f.store
            .begin_attachment_upload(new_upload(&session, "a.txt", b"x"))
    };
    assert_eq!(
        reason(&begin().unwrap_err()),
        AttachmentReason::TooManyAttachments,
        "open expired them; the next expiry is not due yet"
    );
    assert_eq!(ids().len(), 3);

    f.store.set_staged_expiry_interval(Duration::ZERO);
    let ticket = begin().unwrap();
    assert!(f.store.abort_upload(&ticket.upload_id));
    assert_eq!(ids(), vec![fresh.id.clone()], "only the stale rows go");
    assert!(
        files(&f.attachment_dir()).is_empty(),
        "with their blob, without a reopen"
    );

    // Workspace attachments and lists expire them too; fresh rows still count.
    leak("att_leaked", day);
    let created = f
        .store
        .create_attachment(
            &session,
            NewAttachment {
                name: "w.txt".into(),
                mime: None,
                source: None,
                bytes: Zeroizing::new(b"w".to_vec()),
            },
        )
        .unwrap();
    leak("att_leaked", day);
    let listed = f.store.list_attachments(&session, 0, 10).unwrap();
    assert_eq!((listed.total, listed.attachments.len()), (2, 2));
    let mut expected = vec![fresh.id.clone(), created.id];
    expected.sort();
    assert_eq!(ids(), expected);
    leak("att_recent", 0);
    assert_eq!(
        reason(&begin().unwrap_err()),
        AttachmentReason::TooManyAttachments
    );
}

#[test]
fn rows_whose_columns_disagree_with_the_sealed_meta_fail_closed() {
    let f = Fixture::new();
    let session = f.session();
    let other = f.session();
    let meta = upload(&f.store, &session, "a.txt", b"hello").unwrap();
    let blob = upload(
        &f.store,
        &session,
        "b.pdf",
        &pdf(INLINE_ATTACHMENT_BYTES + 5),
    )
    .unwrap();
    f.db()
        .execute(
            "UPDATE attachments SET state='sent' WHERE id=?",
            [meta.id.as_str()],
        )
        .unwrap();
    assert!(
        f.store
            .attachment(&meta.id)
            .unwrap_err()
            .contains("mismatch")
    );
    assert!(f.store.read_attachment(&meta.id, 0, 10).is_err());
    assert_eq!(
        f.store
            .list_attachments(&session, 0, 10)
            .unwrap()
            .attachments
            .len(),
        1
    );
    assert_eq!(
        reason(
            &f.store
                .begin_with(&session, "x", "c", None, &options(&[&meta.id]))
                .unwrap_err()
        ),
        AttachmentReason::AttachmentUnavailable
    );
    f.db()
        .execute(
            "UPDATE attachments SET state='staged',session_id=? WHERE id=?",
            [other.as_str(), meta.id.as_str()],
        )
        .unwrap();
    assert!(
        f.store.attachment(&meta.id).is_err(),
        "rows cannot move between sessions"
    );
    let path = f.attachment_dir().join(&files(&f.attachment_dir())[0]);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[100] ^= 1;
    std::fs::write(&path, &bytes).unwrap();
    assert!(
        f.store
            .attachment_bytes(&blob.id)
            .unwrap_err()
            .contains("authentication failed")
    );
}

#[test]
fn text_pages_fit_the_rpc_line_after_json_escaping() {
    let f = Fixture::new();
    let session = f.session();
    for unit in ["\n", "\u{1b}", "é", "𝄞"] {
        let text = unit.repeat(1024 * 1024 / unit.len());
        let meta = f
            .store
            .create_attachment(
                &session,
                NewAttachment {
                    name: "log.txt".into(),
                    mime: Some("text/plain".into()),
                    source: Some(AttachmentSource::Workspace),
                    bytes: Zeroizing::new(text.clone().into_bytes()),
                },
            )
            .unwrap();
        let mut offset = 0;
        let mut out = String::new();
        loop {
            let page = f
                .store
                .read_attachment(&meta.id, offset, usize::MAX)
                .unwrap();
            assert_eq!(page.encoding, AttachmentEncoding::Utf8);
            assert!(page.bytes.len() <= ATTACHMENT_TEXT_PAGE_BYTES);
            let text = String::from_utf8(page.bytes).unwrap();
            assert!(serde_json::to_string(&text).unwrap().len() <= 900 * 1024);
            out.push_str(&text);
            match page.next_offset {
                Some(next) => offset = next,
                None => break,
            }
        }
        assert_eq!(out, text);
        assert!(
            !f.store
                .read_attachment(&meta.id, 0, 3)
                .unwrap()
                .bytes
                .is_empty(),
            "a page always makes progress"
        );
    }
    let image = f
        .store
        .create_attachment(
            &session,
            NewAttachment {
                name: "p.png".into(),
                mime: None,
                source: None,
                bytes: Zeroizing::new(png(4, 4)),
            },
        )
        .unwrap();
    assert_eq!(
        reason(&f.store.read_attachment(&image.id, 999, 10).unwrap_err()),
        AttachmentReason::InvalidAttachment
    );
    let note = f
        .store
        .create_attachment(
            &session,
            NewAttachment {
                name: "n.txt".into(),
                mime: None,
                source: None,
                bytes: Zeroizing::new("é".as_bytes().to_vec()),
            },
        )
        .unwrap();
    assert_eq!(
        reason(&f.store.read_attachment(&note.id, 1, 10).unwrap_err()),
        AttachmentReason::InvalidAttachment
    );
}

#[test]
fn readers_are_bounded_and_sniffed_like_uploads() {
    let f = Fixture::new();
    let session = f.session();
    let meta = f
        .store
        .create_attachment_from_reader(
            &session,
            "src/notes.md",
            Some("text/markdown"),
            Some(AttachmentSource::Workspace),
            &mut "# notes".as_bytes(),
            7,
        )
        .unwrap();
    assert_eq!(
        (meta.name.as_str(), meta.mime.as_str(), meta.source),
        (
            "notes.md",
            "text/markdown",
            Some(AttachmentSource::Workspace)
        )
    );
    let mut endless = std::io::repeat(b'a');
    let error = f
        .store
        .create_attachment_from_reader(&session, "big.txt", None, None, &mut endless, 0)
        .unwrap_err();
    assert_eq!(reason(&error), AttachmentReason::TooLarge);
    let error = f
        .store
        .create_attachment_from_reader(
            &session,
            "id_rsa.txt",
            None,
            None,
            &mut "-----BEGIN OPENSSH PRIVATE KEY-----\n".as_bytes(),
            0,
        )
        .unwrap_err();
    assert_eq!(reason(&error), AttachmentReason::SensitiveContent);
    let image = png(32, 16);
    let meta = f
        .store
        .create_attachment_from_reader(&session, "i.png", None, None, &mut image.as_slice(), 1)
        .unwrap();
    assert_eq!((meta.kind, meta.width), (AttachmentKind::Image, Some(32)));
}

fn call(id: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "web_search".into(),
        arguments: "{}".into(),
        provider_metadata: None,
    }
}

#[test]
fn forks_copy_history_and_reseal_attachments_on_their_messages() {
    let f = Fixture::new();
    let source = f.session();
    let first = upload(&f.store, &source, "first.png", &png(10, 20)).unwrap();
    let second_bytes = pdf(INLINE_ATTACHMENT_BYTES + 77);
    let second = upload(&f.store, &source, "second.pdf", &second_bytes).unwrap();
    let third = upload(&f.store, &source, "third.txt", b"third").unwrap();
    let staged = upload(&f.store, &source, "staged.txt", b"not sent").unwrap();
    let complete = |f: &Fixture| {
        f.store
            .update(&source, |s| {
                for run in &mut s.runs {
                    run.status = RunStatus::Completed;
                }
                Ok(())
            })
            .unwrap()
    };
    // Turn 1 (index 0) sends an image, then a failed tool-call turn leaves an
    // unanswered call and an empty placeholder mid-history.
    f.store
        .begin_with(&source, "look", "t1", None, &options(&[&first.id]))
        .unwrap();
    f.store
        .update(&source, |s| {
            let mut asked = ChatMessage::assistant("");
            asked.tool_calls = vec![call("call_1")];
            s.messages.push(asked);
            s.messages.push(ChatMessage::assistant(""));
            Ok(())
        })
        .unwrap();
    complete(&f);
    let turn2 = f
        .store
        .begin_with(&source, "", "t2", None, &options(&[&third.id, &second.id]))
        .unwrap();
    assert_eq!(turn2.message_index, Some(3));
    f.store
        .update(&source, |s| {
            s.messages.push(ChatMessage::assistant(""));
            Ok(())
        })
        .unwrap();
    complete(&f);
    f.store
        .update(&source, |s| {
            s.runtime_ref = Some(serde_json::json!({"thread":"t"}));
            Ok(())
        })
        .unwrap();

    let config: SessionConfig = serde_json::from_value(serde_json::json!({"provider_id":"fixture","model_id":"tools","mode":"agent","workspace_id":"workspace_1","title":"old"})).unwrap();
    let request = ForkRequest {
        configuration: config.clone(),
        title: Some("Continued".into()),
        command_id: Some("fork-1".into()),
    };
    let fork = f.store.fork(&source, request.clone()).unwrap();
    assert_eq!(fork.configuration.title, "Continued");
    assert_eq!(fork.configuration.mode, Mode::Agent);
    assert_eq!(
        fork.messages.len(),
        4,
        "only the trailing placeholder is dropped"
    );
    assert_eq!(fork.messages[1].role, Role::Assistant);
    assert!(
        fork.messages[1].tool_calls.is_empty(),
        "the unanswered call is stripped in place"
    );
    assert_eq!(fork.messages[3].content, ATTACHMENT_ONLY_CONTENT);
    assert!(fork.runs.is_empty() && fork.commands.is_empty() && fork.runtime_ref.is_none());
    let origin = fork.forked_from.clone().unwrap();
    assert_eq!(
        (origin.session_id.as_str(), origin.message_count),
        (source.as_str(), 4)
    );

    let copies = f.store.message_attachments(&fork.id).unwrap();
    assert_eq!(copies.keys().copied().collect::<Vec<_>>(), vec![0, 3]);
    assert_eq!(
        copies[&3]
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        vec!["third.txt", "second.pdf"]
    );
    for (index, list) in &copies {
        assert_eq!(
            fork.messages[*index].role,
            Role::User,
            "attachments stay on user messages"
        );
        for copy in list {
            assert!(
                ![&first.id, &second.id, &third.id].contains(&&copy.id),
                "copies get new ids"
            );
            assert_eq!(
                (
                    copy.session_id.as_str(),
                    copy.run_id.as_deref(),
                    copy.command_id.as_deref()
                ),
                (fork.id.as_str(), None, None)
            );
        }
    }
    let copy_pdf = &copies[&3][1];
    assert_eq!(
        &*f.store.attachment_bytes(&copy_pdf.id).unwrap().bytes,
        &second_bytes[..]
    );
    assert_eq!(
        f.store
            .attachment_bytes(&copies[&0][0].id)
            .unwrap()
            .bytes
            .len() as u64,
        first.size
    );
    assert_eq!(
        f.store.list_attachments(&fork.id, 0, 10).unwrap().total,
        3,
        "staged attachments are not copied"
    );
    assert_eq!(
        f.store.attachment(&staged.id).unwrap().state,
        AttachmentState::Staged
    );
    assert_eq!(
        files(&f.attachment_dir()).len(),
        2,
        "the large PDF is re-sealed into its own blob"
    );

    // Retried forks return the first result; changed arguments are refused.
    assert_eq!(f.store.fork(&source, request.clone()).unwrap().id, fork.id);
    let mut changed = request.clone();
    changed.title = Some("Other".into());
    assert!(
        f.store
            .fork(&source, changed)
            .unwrap_err()
            .contains("different arguments")
    );
    // Deleting the source leaves the fork intact.
    assert!(f.store.delete(&source).unwrap());
    assert_eq!(
        &*f.store.attachment_bytes(&copy_pdf.id).unwrap().bytes,
        &second_bytes[..]
    );

    let busy = f.session();
    f.store.begin(&busy, "running", "r", None).unwrap();
    let error = f
        .store
        .fork(
            &busy,
            ForkRequest {
                configuration: config,
                title: None,
                command_id: None,
            },
        )
        .unwrap_err();
    assert!(error.contains("active runs"), "{error}");
}

/// `SessionDetail` and `Run` exactly as the PR 2 daemon (main at bfdd359)
/// declares them: unknown fields must not break its decoding.
mod legacy {
    use aidex_native_sessions::{RunStatus, SessionConfig};
    use aidex_providers::{ChatMessage, ToolCall};
    use serde::Deserialize;
    use std::collections::BTreeMap;

    #[derive(Deserialize)]
    #[allow(dead_code)]
    pub struct Run {
        pub id: String,
        pub session_id: String,
        pub parent_run_id: Option<String>,
        pub status: RunStatus,
        pub steps: u32,
        pub error: Option<String>,
        pub created_at: u64,
        pub updated_at: u64,
        #[serde(default)]
        pub transcript: Vec<ChatMessage>,
        #[serde(default)]
        pub pending_calls: Vec<ToolCall>,
        #[serde(default)]
        pub effective_max_tokens: Option<u32>,
        #[serde(default)]
        pub estimated_input_tokens: Option<u32>,
        #[serde(default)]
        pub model_used: Option<String>,
        #[serde(default)]
        pub runtime_ref: Option<serde_json::Value>,
        #[serde(default)]
        pub skills: Vec<String>,
        #[serde(default)]
        pub web_search: bool,
        #[serde(default)]
        pub message_index: Option<usize>,
    }
    #[derive(Deserialize)]
    #[allow(dead_code)]
    pub struct CommandRecord {
        pub fingerprint: String,
        pub run_id: String,
    }
    #[derive(Deserialize)]
    #[allow(dead_code)]
    pub struct SessionDetail {
        pub id: String,
        #[serde(flatten)]
        pub configuration: SessionConfig,
        pub pinned: bool,
        pub created_at: u64,
        pub messages: Vec<ChatMessage>,
        pub runs: Vec<Run>,
        pub approvals: Vec<serde_json::Value>,
        pub checkpoints: Vec<serde_json::Value>,
        #[serde(default)]
        pub commands: BTreeMap<String, CommandRecord>,
        #[serde(default)]
        pub runtime_ref: Option<serde_json::Value>,
    }
}

#[test]
fn records_written_with_attachments_stay_readable_by_the_previous_daemon() {
    let f = Fixture::new();
    let source = f.session();
    let image = upload(&f.store, &source, "i.png", &png(2, 2)).unwrap();
    f.store
        .begin_with(&source, "", "send", None, &options(&[&image.id]))
        .unwrap();
    f.store
        .update(&source, |s| {
            s.runs[0].status = RunStatus::Completed;
            Ok(())
        })
        .unwrap();
    let config: SessionConfig =
        serde_json::from_value(serde_json::json!({"provider_id":"fixture","model_id":"small"}))
            .unwrap();
    let fork = f
        .store
        .fork(
            &source,
            ForkRequest {
                configuration: config,
                title: None,
                command_id: Some("f".into()),
            },
        )
        .unwrap();
    for id in [&source, &fork.id] {
        let detail = f.store.get(id).unwrap();
        let value = serde_json::to_value(&detail).unwrap();
        let legacy: legacy::SessionDetail = serde_json::from_value(value).unwrap();
        assert_eq!(legacy.messages.len(), 1);
        for message in &legacy.messages {
            assert!(
                message.role != Role::User || !message.content.trim().is_empty(),
                "no blank user message reaches an older daemon"
            );
        }
    }
    let value = serde_json::to_value(f.store.get(&source).unwrap()).unwrap();
    assert_eq!(
        value["runs"][0]["attachments"],
        serde_json::json!([image.id])
    );
    assert_eq!(value["runs"][0]["mode"], "chat");
    let legacy: legacy::SessionDetail = serde_json::from_value(value).unwrap();
    assert_eq!(legacy.runs[0].message_index, Some(0));
}
