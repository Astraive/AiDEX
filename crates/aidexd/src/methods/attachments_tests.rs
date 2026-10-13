use super::*;
use crate::{
    lifecycle::ShutdownSignal,
    router::{CallContext, ClientInfo},
};
use aidex_native_sessions::{
    ATTACHMENT_TEXT_PAGE_BYTES, MAX_ATTACHMENTS_PER_MESSAGE, MAX_STAGED_PER_SESSION,
    MAX_TEXT_ATTACHMENT_BYTES, NewAttachment, RunOptions, digest,
};
use aidex_vault::{
    SecretString,
    testing::{WriteFailure, fail_next_write},
};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const PASSPHRASE: &str = "fixture-passphrase";

struct Harness {
    _drive: tempfile::TempDir,
    services: Arc<Services>,
    router: Router,
    ctx: CallContext,
    session: String,
}

impl Harness {
    fn new() -> Self {
        let drive = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        services
            .vault()
            .init(&SecretString::from(PASSPHRASE.to_owned()), false)
            .unwrap();
        let session = services
            .native()
            .store()
            .create(
                serde_json::from_value(json!({"provider_id":"fixture","model_id":"small"}))
                    .unwrap(),
            )
            .unwrap()
            .id;
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        crate::methods::native::register(&mut router, &services).unwrap();
        crate::methods::vault::register(&mut router, &services).unwrap();
        crate::methods::emergency::register(&mut router, &services).unwrap();
        let ctx = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        Self {
            _drive: drive,
            services,
            router,
            ctx,
            session,
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.router.call(&self.ctx, method, params)
    }

    fn error(&self, method: &str, params: Value) -> RpcError {
        let error = self.call(method, params).unwrap_err();
        assert!(error.kind().is_some(), "a documented error code");
        error
    }

    fn begin(&self, name: &str, bytes: &[u8]) -> Result<String, RpcError> {
        self.call(
            "attachment.upload.begin",
            json!({"session_id": self.session, "name": name, "size": bytes.len(), "sha256": digest(bytes)}),
        )
        .map(|ticket| {
            assert_eq!(ticket["chunk_bytes"], UPLOAD_CHUNK_BYTES);
            ticket["upload_id"].as_str().unwrap().to_owned()
        })
    }

    fn chunk(&self, upload: &str, offset: usize, bytes: &[u8]) -> Result<Value, RpcError> {
        self.call(
            "attachment.upload.chunk",
            json!({"upload_id": upload, "offset": offset, "data": STANDARD.encode(bytes)}),
        )
    }

    fn send_chunks(&self, upload: &str, bytes: &[u8]) {
        for (n, chunk) in bytes.chunks(UPLOAD_CHUNK_BYTES).enumerate() {
            let offset = n * UPLOAD_CHUNK_BYTES;
            let received = self.chunk(upload, offset, chunk).unwrap();
            assert_eq!(received["received"], offset + chunk.len());
        }
    }

    /// Upload `bytes` and commit them.
    fn upload(&self, name: &str, bytes: &[u8], mime: Option<&str>) -> Result<Value, RpcError> {
        let mut params = json!({"session_id": self.session, "name": name, "size": bytes.len(), "sha256": digest(bytes)});
        if let Some(mime) = mime {
            params["mime"] = json!(mime);
        }
        let upload = self.call("attachment.upload.begin", params)?["upload_id"]
            .as_str()
            .unwrap()
            .to_owned();
        self.send_chunks(&upload, bytes);
        self.call("attachment.upload.commit", json!({"upload_id": upload}))
    }

    fn uploaded(&self, name: &str, bytes: &[u8]) -> String {
        self.upload(name, bytes, None).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Assemble every `attachment.read` page; every page fits one RPC line.
    fn download(&self, id: &str, limit: Option<usize>) -> Vec<u8> {
        let mut bytes = vec![];
        let mut offset = Value::Null;
        loop {
            let mut params = json!({"id": id});
            if !offset.is_null() {
                params["offset"] = offset;
            }
            if let Some(limit) = limit {
                params["limit"] = json!(limit);
            }
            let page = self.call("attachment.read", params).unwrap();
            assert!(serde_json::to_vec(&page).unwrap().len() < crate::rpc::MAX_MESSAGE_BYTES);
            let content = page["content"].as_str().unwrap();
            match page["encoding"].as_str().unwrap() {
                "utf8" => {
                    assert!(content.len() <= ATTACHMENT_TEXT_PAGE_BYTES);
                    bytes.extend_from_slice(content.as_bytes());
                }
                "base64" => bytes.extend(STANDARD.decode(content).unwrap()),
                other => panic!("unknown encoding {other}"),
            }
            if page["next_offset"].is_null() {
                assert_eq!(page["total_bytes"], bytes.len());
                return bytes;
            }
            offset = page["next_offset"].clone();
        }
    }

    fn attachment_dir(&self) -> PathBuf {
        self.services.paths().sessions().join("native-attachments")
    }

    fn temporary_files(&self) -> usize {
        std::fs::read_dir(self.attachment_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
                    .count()
            })
            .unwrap_or(0)
    }

    fn unlock(&self) {
        self.services
            .vault()
            .unlock(&SecretString::from(PASSPHRASE.to_owned()))
            .unwrap();
    }

    /// Send staged attachments with a new user message (store level).
    fn send(&self, ids: &[&str], command: &str) -> aidex_native_sessions::Run {
        self.services
            .native()
            .store()
            .begin_with(
                &self.session,
                "",
                command,
                None,
                &RunOptions {
                    attachments: ids.iter().map(|id| (*id).to_owned()).collect(),
                    ..RunOptions::default()
                },
            )
            .unwrap()
    }
}

fn reason(error: &RpcError) -> &str {
    error
        .data
        .as_ref()
        .and_then(|d| d["reason"].as_str())
        .unwrap_or_default()
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    b.extend(13u32.to_be_bytes());
    b.extend(b"IHDR");
    b.extend(width.to_be_bytes());
    b.extend(height.to_be_bytes());
    b.extend([8, 6, 0, 0, 0]);
    b.extend([0; 4]);
    b.extend(0u32.to_be_bytes());
    b.extend(b"IDAT");
    b.extend([0; 4]);
    b
}

fn pdf(pages: usize) -> Vec<u8> {
    let mut b = b"%PDF-1.4\n".to_vec();
    for n in 0..pages {
        b.extend(format!("{} 0 obj << /Type /Page >> endobj\n", n + 1).into_bytes());
    }
    b.extend(b"%%EOF\n");
    b
}

#[test]
fn uploads_round_trip_text_images_and_pdfs_through_paged_reads() {
    let h = Harness::new();
    let text = "naïve café ✓ line\n".repeat(40_000);
    assert!(text.len() > 2 * UPLOAD_CHUNK_BYTES);
    let notes = h
        .upload(
            "C:\\Users\\me\\notes.md",
            text.as_bytes(),
            Some("text/markdown"),
        )
        .unwrap();
    assert_eq!(notes["name"], "notes.md");
    assert_eq!(notes["kind"], "text");
    assert_eq!(notes["mime"], "text/markdown");
    assert_eq!(notes["size"], text.len());
    assert_eq!(notes["sha256"], digest(text.as_bytes()));
    assert_eq!(notes["state"], "staged");
    assert_eq!(notes["source"], "file");
    assert_eq!(notes["excluded"], false);
    assert!(notes.get("message_index").is_none());
    let image = png(640, 480);
    let shot = h
        .call(
            "attachment.upload.begin",
            json!({"session_id": h.session, "name": "shot.png", "size": image.len(), "sha256": digest(&image), "mime": "text/plain", "source": "paste"}),
        )
        .unwrap();
    let upload = shot["upload_id"].as_str().unwrap();
    h.send_chunks(upload, &image);
    let shot = h
        .call("attachment.upload.commit", json!({"upload_id": upload}))
        .unwrap();
    assert_eq!(shot["kind"], "image", "the kind comes from the bytes");
    assert_eq!(shot["mime"], "image/png");
    assert_eq!(
        (shot["width"].clone(), shot["height"].clone()),
        (json!(640), json!(480))
    );
    assert_eq!(shot["source"], "paste");
    let doc = h.upload("spec.pdf", &pdf(3), None).unwrap();
    assert_eq!(
        (doc["kind"].clone(), doc["pages"].clone()),
        (json!("pdf"), json!(3))
    );

    let listed = h
        .call("attachment.list", json!({"session_id": h.session}))
        .unwrap();
    assert_eq!(listed["total_attachments"], 3);
    assert_eq!(listed["truncated"], false);
    assert_eq!(listed["attachments"][0]["id"], doc["id"]);
    assert_eq!(listed["attachments"][2]["id"], notes["id"]);
    let first = h
        .call(
            "attachment.list",
            json!({"session_id": h.session, "offset": 1, "limit": 1}),
        )
        .unwrap();
    assert_eq!(first["attachments"][0]["id"], shot["id"]);
    assert_eq!(
        (first["next_offset"].clone(), first["truncated"].clone()),
        (json!(2), json!(true))
    );
    assert_eq!(
        h.call("attachment.get", json!({"id": notes["id"]}))
            .unwrap(),
        notes
    );

    let id = notes["id"].as_str().unwrap();
    let page = h
        .call("attachment.read", json!({"id": id, "limit": 1 << 20}))
        .unwrap();
    assert_eq!(page["encoding"], "utf8");
    assert!(page["next_offset"].as_u64().unwrap() <= ATTACHMENT_TEXT_PAGE_BYTES as u64);
    assert_eq!(page["sha256"], notes["sha256"]);
    assert_eq!(h.download(id, None), text.as_bytes());
    assert_eq!(h.download(id, Some(1001)), text.as_bytes());
    let shot_id = shot["id"].as_str().unwrap();
    let page = h.call("attachment.read", json!({"id": shot_id})).unwrap();
    assert_eq!(page["encoding"], "base64");
    assert_eq!(h.download(shot_id, None), image);
    assert_eq!(h.download(doc["id"].as_str().unwrap(), Some(7)), pdf(3));
    assert_eq!(
        h.temporary_files(),
        0,
        "committed uploads leave no temporary file"
    );
}

#[test]
fn text_pages_of_escape_heavy_files_fit_one_message() {
    let h = Harness::new();
    let text: Vec<u8> = b"\n\x1b".repeat(MAX_TEXT_ATTACHMENT_BYTES / 2);
    let meta = h
        .services
        .native()
        .store()
        .create_attachment(
            &h.session,
            NewAttachment {
                name: "ansi.log".into(),
                mime: None,
                source: None,
                bytes: Zeroizing::new(text.clone()),
            },
        )
        .unwrap();
    assert_eq!(meta.kind, AttachmentKind::Text);
    assert_eq!(h.download(&meta.id, Some(usize::MAX)), text);
}

#[test]
fn chunks_are_sequential_idempotent_and_verified_at_commit() {
    let h = Harness::new();
    let bytes: Vec<u8> = "0123456789abcdef".repeat(40_000).into_bytes();
    let upload = h.begin("data.txt", &bytes).unwrap();
    let first = &bytes[..UPLOAD_CHUNK_BYTES];
    let second = &bytes[UPLOAD_CHUNK_BYTES..2 * UPLOAD_CHUNK_BYTES];
    let skipped = h.chunk(&upload, UPLOAD_CHUNK_BYTES, second).unwrap_err();
    assert_eq!((skipped.code, reason(&skipped)), (-32602, "bad_offset"));
    let short = h.chunk(&upload, 0, &first[..1000]).unwrap_err();
    assert_eq!(
        reason(&short),
        "bad_offset",
        "only the last chunk may be short"
    );
    assert_eq!(
        h.chunk(&upload, 0, first).unwrap()["received"],
        UPLOAD_CHUNK_BYTES
    );
    assert_eq!(
        h.chunk(&upload, 0, first).unwrap()["received"],
        UPLOAD_CHUNK_BYTES,
        "a retried chunk is acknowledged without writing"
    );
    let oversized = h
        .call(
            "attachment.upload.chunk",
            json!({"upload_id": upload, "offset": UPLOAD_CHUNK_BYTES, "data": STANDARD.encode(vec![b'a'; UPLOAD_CHUNK_BYTES + 3])}),
        )
        .unwrap_err();
    assert_eq!(reason(&oversized), "bad_offset");
    let garbage = h
        .call(
            "attachment.upload.chunk",
            json!({"upload_id": upload, "offset": UPLOAD_CHUNK_BYTES, "data": "not base64!"}),
        )
        .unwrap_err();
    assert_eq!(
        (garbage.code, reason(&garbage)),
        (-32602, "invalid_attachment")
    );
    let early = h.error("attachment.upload.commit", json!({"upload_id": upload}));
    assert_eq!(
        reason(&early),
        "bad_offset",
        "an incomplete upload stays open"
    );
    h.chunk(&upload, UPLOAD_CHUNK_BYTES, second).unwrap();
    h.chunk(
        &upload,
        2 * UPLOAD_CHUNK_BYTES,
        &bytes[2 * UPLOAD_CHUNK_BYTES..],
    )
    .unwrap();
    let meta = h
        .call("attachment.upload.commit", json!({"upload_id": upload}))
        .unwrap();
    assert_eq!(meta["size"], bytes.len());
    assert_eq!(
        h.error("attachment.upload.commit", json!({"upload_id": upload}))
            .code,
        -32002,
        "a committed upload is gone"
    );
    assert_eq!(
        h.error(
            "attachment.upload.chunk",
            json!({"upload_id": "upl_missing", "offset": 0, "data": ""})
        )
        .code,
        -32002
    );

    // A declared hash that does not match the bytes.
    let upload = h
        .call(
            "attachment.upload.begin",
            json!({"session_id": h.session, "name": "x.txt", "size": 5, "sha256": digest(b"other")}),
        )
        .unwrap()["upload_id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.chunk(&upload, 0, b"hello").unwrap();
    let mismatch = h.error("attachment.upload.commit", json!({"upload_id": upload}));
    assert_eq!(
        (mismatch.code, reason(&mismatch)),
        (-32602, "hash_mismatch")
    );

    let upload = h.begin("abandoned.txt", b"hello").unwrap();
    assert!(h.temporary_files() > 0);
    assert_eq!(
        h.call("attachment.upload.abort", json!({"upload_id": upload}))
            .unwrap(),
        json!({"aborted": true})
    );
    assert_eq!(
        h.call("attachment.upload.abort", json!({"upload_id": upload}))
            .unwrap(),
        json!({"aborted": false})
    );
    assert_eq!(h.temporary_files(), 0);
    assert_eq!(
        h.call("attachment.list", json!({"session_id": h.session}))
            .unwrap()["total_attachments"],
        1
    );
}

#[test]
fn begin_and_commit_refusals_carry_stable_reasons() {
    let h = Harness::new();
    let begin = |params: Value| h.error("attachment.upload.begin", params);
    let base =
        json!({"session_id": h.session, "name": "a.txt", "size": 5, "sha256": digest(b"hello")});
    let with = |key: &str, value: Value| {
        let mut params = base.clone();
        params[key] = value;
        params
    };
    assert_eq!(
        begin(with("session_id", json!("session_missing"))).code,
        -32002
    );
    for (params, expected) in [
        (with("size", json!(0)), "too_large"),
        (with("size", json!(MAX_UPLOAD_BYTES + 1)), "too_large"),
        (with("name", json!("bad\u{202E}gnp.exe")), "invalid_name"),
        (with("name", json!("  ")), "invalid_name"),
        (with("sha256", json!("abc")), "invalid_attachment"),
    ] {
        let error = begin(params.clone());
        assert_eq!((error.code, reason(&error)), (-32602, expected), "{params}");
        assert!(!error.message.contains("gnp.exe"), "{}", error.message);
    }
    for params in [
        with("source", json!("workspace")),
        with("extra", json!(true)),
        with("size", json!(-1)),
    ] {
        assert_eq!(begin(params.clone()).code, -32602, "{params}");
    }

    // At most 8 uploads in flight per daemon.
    let uploads: Vec<String> = (0..8)
        .map(|n| h.begin(&format!("f{n}.txt"), b"hello").unwrap())
        .collect();
    let busy = begin(base.clone());
    assert_eq!((busy.code, reason(&busy)), (-32006, "busy"));
    for upload in &uploads {
        h.call("attachment.upload.abort", json!({"upload_id": upload}))
            .unwrap();
    }

    // At most 40 staged attachments per conversation.
    for n in 0..MAX_STAGED_PER_SESSION {
        h.services
            .native()
            .store()
            .create_attachment(
                &h.session,
                NewAttachment {
                    name: format!("n{n}.txt"),
                    mime: None,
                    source: None,
                    bytes: Zeroizing::new(b"note".to_vec()),
                },
            )
            .unwrap();
    }
    let full = begin(base.clone());
    assert_eq!((full.code, reason(&full)), (-32602, "too_many_attachments"));
}

#[test]
fn commit_sniffs_kinds_and_refuses_what_cannot_be_attached() {
    let h = Harness::new();
    for (name, bytes, expected) in [
        ("blob.bin", vec![0u8, 1, 2, 3], "unsupported_type"),
        ("latin1.txt", vec![b'c', b'a', b'f', 0xE9], "unsupported_type"),
        (
            "keys.txt",
            b"notes\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n"
                .to_vec(),
            "sensitive_content",
        ),
        ("long.pdf", pdf(101), "too_many_pages"),
        ("big.txt", vec![b'a'; MAX_TEXT_ATTACHMENT_BYTES + 1], "too_large"),
        ("huge.png", png(8001, 10), "too_large"),
    ] {
        let error = h.upload(name, &bytes, None).unwrap_err();
        assert_eq!((error.code, reason(&error)), (-32602, expected), "{name}");
        assert!(!error.message.contains(name), "{}", error.message);
    }
    assert_eq!(h.temporary_files(), 0, "refused uploads leave no file");
    assert_eq!(
        h.call("attachment.list", json!({"session_id": h.session}))
            .unwrap()["total_attachments"],
        0
    );
}

#[test]
fn staged_attachments_delete_sent_ones_stay_and_can_be_excluded() {
    let h = Harness::new();
    let staged = h.uploaded("draft.txt", b"draft");
    let kept = h.uploaded("kept.txt", b"kept");
    assert_eq!(
        h.call("attachment.delete", json!({"id": staged})).unwrap(),
        json!({"deleted": true})
    );
    assert_eq!(
        h.call("attachment.delete", json!({"id": staged})).unwrap(),
        json!({"deleted": false})
    );
    assert_eq!(
        h.error("attachment.get", json!({"id": staged})).code,
        -32002
    );
    let not_sent = h.error("attachment.exclude", json!({"id": kept}));
    assert_eq!(reason(&not_sent), "invalid_attachment");

    let run = h.send(&[&kept], "with-file");
    let sent = h.call("attachment.get", json!({"id": kept})).unwrap();
    assert_eq!(sent["state"], "sent");
    assert_eq!(sent["message_index"], 0);
    assert_eq!(sent["position"], 0);
    assert_eq!(sent["run_id"], run.id.as_str());
    let refused = h.error("attachment.delete", json!({"id": kept}));
    assert_eq!(
        (refused.code, reason(&refused)),
        (-32602, "attachment_sent")
    );

    let excluded = h.call("attachment.exclude", json!({"id": kept})).unwrap();
    assert_eq!(excluded["excluded"], true);
    let view = h
        .call("conversation.get", json!({"id": h.session}))
        .unwrap();
    assert_eq!(view["messages"][0]["content"], "[Attached files]");
    assert_eq!(view["messages"][0]["attachments"][0]["id"], kept.as_str());
    assert_eq!(view["messages"][0]["attachments"][0]["excluded"], true);
    let restored = h
        .call("attachment.exclude", json!({"id": kept, "excluded": false}))
        .unwrap();
    assert_eq!(restored["excluded"], false);
    let view = h
        .call("conversation.get", json!({"id": h.session}))
        .unwrap();
    assert!(
        view["messages"][0]["attachments"][0]
            .get("excluded")
            .is_none()
    );

    // Deleting the conversation deletes its attachments.
    h.services
        .native()
        .store()
        .update(&h.session, |s| {
            for run in &mut s.runs {
                run.status = aidex_native_sessions::RunStatus::Completed;
            }
            Ok(())
        })
        .unwrap();
    h.call("conversation.delete", json!({"id": h.session}))
        .unwrap();
    assert_eq!(h.error("attachment.get", json!({"id": kept})).code, -32002);
    assert_eq!(
        h.error("attachment.list", json!({"session_id": h.session}))
            .code,
        -32002
    );
}

#[test]
fn malformed_reads_and_list_pages_are_invalid_params() {
    let h = Harness::new();
    let id = h.uploaded("é.txt", "é".repeat(10).as_bytes());
    for (method, params) in [
        (
            "attachment.list",
            json!({"session_id": h.session, "limit": 0}),
        ),
        (
            "attachment.list",
            json!({"session_id": h.session, "limit": ATTACHMENT_LIST_LIMIT + 1}),
        ),
        ("attachment.list", json!({})),
        ("attachment.get", json!({"id": id, "extra": true})),
        ("attachment.read", json!({"id": id, "limit": 0})),
        ("attachment.read", json!({"id": id, "offset": 1000})),
        ("attachment.read", json!({"id": id, "offset": 1})),
        ("attachment.delete", Value::Null),
        ("attachment.upload.abort", json!({})),
    ] {
        assert_eq!(
            h.error(method, params.clone()).code,
            -32602,
            "{method} {params}"
        );
    }
    assert_eq!(
        h.error("attachment.read", json!({"id": "att_missing"}))
            .code,
        -32002
    );
}

#[test]
fn attachments_are_unavailable_while_the_vault_is_locked() {
    let h = Harness::new();
    let id = h.uploaded("private-name.txt", b"private words");
    let upload = h.begin("later.txt", b"hello").unwrap();
    h.services.vault().lock();
    for (method, params) in [
        (
            "attachment.upload.begin",
            json!({"session_id": h.session, "name": "a.txt", "size": 5, "sha256": digest(b"hello")}),
        ),
        (
            "attachment.upload.chunk",
            json!({"upload_id": upload, "offset": 0, "data": STANDARD.encode(b"hello")}),
        ),
        ("attachment.upload.commit", json!({"upload_id": upload})),
        ("attachment.list", json!({"session_id": h.session})),
        ("attachment.get", json!({"id": id})),
        ("attachment.read", json!({"id": id})),
        ("attachment.delete", json!({"id": id})),
        ("attachment.exclude", json!({"id": id})),
        (
            "attachment.from_workspace",
            json!({"session_id": h.session, "workspace_id": "workspace_x", "path": "a.txt"}),
        ),
    ] {
        let error = h.call(method, params).unwrap_err();
        assert_eq!(error.code, -32020, "{method}: {}", error.message);
        assert!(!error.message.contains("private"), "{}", error.message);
    }
    assert_eq!(
        h.call("attachment.upload.abort", json!({"upload_id": upload}))
            .unwrap(),
        json!({"aborted": false}),
        "abort needs no key; the lock already ended the upload"
    );
    assert_eq!(h.temporary_files(), 0);
    h.unlock();
    assert_eq!(h.download(&id, None), b"private words");
}

#[test]
fn locking_the_vault_mid_upload_ends_every_upload() {
    let h = Harness::new();
    let bytes = vec![b'x'; UPLOAD_CHUNK_BYTES + 10];
    // Sessions the vault ends on its own are cleaned up in the background.
    let await_cleanup = |lock: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.temporary_files() > 0 {
            assert!(Instant::now() < deadline, "{lock} kept the partial file");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(h.services.vault().status().state, VaultState::Locked);
    };
    for lock in ["vault.lock", "emergency.stop", "idle", "uncertain commit"] {
        let upload = h.begin("big.txt", &bytes).unwrap();
        h.chunk(&upload, 0, &bytes[..UPLOAD_CHUNK_BYTES]).unwrap();
        assert!(h.temporary_files() > 0);
        match lock {
            "vault.lock" => {
                h.call("vault.lock", Value::Null).unwrap();
                assert_eq!(h.temporary_files(), 0, "{lock} deletes partial files");
            }
            "emergency.stop" => {
                let report = h
                    .call("emergency.stop", json!({"lock_vault": true}))
                    .unwrap();
                assert_eq!(report["vault_locked"], true);
                assert_eq!(h.temporary_files(), 0, "{lock} deletes partial files");
            }
            "idle" => {
                let vault = h.services.vault();
                vault.set_idle_timeout(Some(Duration::from_millis(50)));
                await_cleanup(lock);
                vault.set_idle_timeout(None);
            }
            // The vault drops its session after an uncertain header commit.
            _ => {
                h.call(
                    "vault.secret.set",
                    json!({"name": "dropped", "value": "v", "consumers": ["provider:openai"]}),
                )
                .unwrap();
                fail_next_write(h.services.vault().dir(), WriteFailure::AfterRename);
                let error = h
                    .call("vault.secret.delete", json!({"name": "dropped"}))
                    .unwrap_err();
                assert_eq!(error.data.unwrap()["session_dropped"], true);
                await_cleanup(lock);
            }
        }
        let next = h
            .chunk(&upload, UPLOAD_CHUNK_BYTES, &bytes[UPLOAD_CHUNK_BYTES..])
            .unwrap_err();
        assert_eq!(next.code, -32020, "{lock}: {}", next.message);
        assert_eq!(h.temporary_files(), 0, "{lock}");
        assert_eq!(
            h.call("attachment.upload.abort", json!({"upload_id": upload}))
                .unwrap(),
            json!({"aborted": false}),
            "{lock}"
        );
        if lock == "emergency.stop" {
            h.call("emergency.resume", Value::Null).unwrap();
        }
        h.unlock();
        let restarted = h
            .chunk(&upload, UPLOAD_CHUNK_BYTES, &bytes[UPLOAD_CHUNK_BYTES..])
            .unwrap_err();
        assert_eq!(restarted.code, -32002, "{lock}: the upload must start over");
    }
}

#[test]
fn workspace_files_are_read_through_the_confined_root() {
    let h = Harness::new();
    let ws = tempfile::tempdir().unwrap();
    let root = ws.path();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("docs/notes.md"), "# Notes\nproject words\n").unwrap();
    std::fs::write(root.join("logo.png"), png(32, 32)).unwrap();
    std::fs::write(root.join(".env"), "TOKEN=secret-value\n").unwrap();
    std::fs::create_dir_all(root.join(".ssh")).unwrap();
    std::fs::write(root.join(".ssh/config"), "Host x\n").unwrap();
    std::fs::write(root.join("server.pem"), "certificate\n").unwrap();
    std::fs::write(root.join("blob.bin"), [0u8, 1, 2]).unwrap();
    let big = std::fs::File::create(root.join("big.txt")).unwrap();
    big.set_len(MAX_UPLOAD_BYTES + 1).unwrap();
    drop(big);
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("outside.txt"), "outside").unwrap();
    let workspace = h.services.workspaces().add_host(root, None).unwrap();
    let attach = |path: &str| {
        h.call(
            "attachment.from_workspace",
            json!({"session_id": h.session, "workspace_id": workspace.id, "path": path}),
        )
    };

    let notes = attach("docs/notes.md").unwrap();
    assert_eq!(notes["name"], "notes.md");
    assert_eq!(notes["kind"], "text");
    assert_eq!(notes["mime"], "text/markdown");
    assert_eq!(notes["source"], "workspace");
    assert_eq!(notes["state"], "staged");
    let id = notes["id"].as_str().unwrap();
    assert_eq!(h.download(id, None), b"# Notes\nproject words\n");
    assert_eq!(attach("logo.png").unwrap()["kind"], "image");

    for (path, code, expected) in [
        (".env", -32602, "sensitive_content"),
        (".ssh/config", -32602, "sensitive_content"),
        ("server.pem", -32602, "sensitive_content"),
        ("blob.bin", -32602, "unsupported_type"),
        ("docs", -32602, "unsupported_type"),
        ("big.txt", -32602, "too_large"),
        ("missing.txt", -32002, "path_not_found"),
        ("../outside.txt", -32001, "invalid_path"),
    ] {
        let error = attach(path).unwrap_err();
        assert_eq!((error.code, reason(&error)), (code, expected), "{path}");
        let file = path.rsplit('/').next().unwrap();
        assert!(!error.message.contains(file), "{path}: {}", error.message);
    }
    let absolute = outside.path().join("outside.txt");
    assert!(attach(&absolute.to_string_lossy()).is_err());

    // A link inside the root to a secret file is refused by its target.
    let linked = {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join(".env"), root.join("docs/readme.txt")).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(root.join(".env"), root.join("docs/readme.txt"))
                .is_ok()
        }
    };
    if linked {
        let error = attach("docs/readme.txt").unwrap_err();
        assert_eq!(reason(&error), "sensitive_content");
    }

    let unknown = h
        .call(
            "attachment.from_workspace",
            json!({"session_id": h.session, "workspace_id": aidex_core::EntityId::generate("workspace").unwrap().to_string(), "path": "docs/notes.md"}),
        )
        .unwrap_err();
    assert_eq!(unknown.code, -32002);
    let no_session = h
        .call(
            "attachment.from_workspace",
            json!({"session_id": "session_missing", "workspace_id": workspace.id, "path": "docs/notes.md"}),
        )
        .unwrap_err();
    assert_eq!(no_session.code, -32002);
    assert_eq!(
        h.call("attachment.list", json!({"session_id": h.session}))
            .unwrap()["total_attachments"],
        2
    );
}

#[test]
fn send_params_check_attachment_count_and_child_runs() {
    let h = Harness::new();
    let id = h.uploaded("a.txt", b"hello");
    let send = |params: Value| h.call("message.send", params);
    let many: Vec<String> = (0..=MAX_ATTACHMENTS_PER_MESSAGE)
        .map(|_| id.clone())
        .collect();
    let error = send(
        json!({"session_id": h.session, "content": "", "command_id": "c1", "attachments": many}),
    )
    .unwrap_err();
    assert_eq!(
        (error.code, reason(&error)),
        (-32602, "attachment_unavailable")
    );
    let error = send(json!({"session_id": h.session, "content": "hi", "command_id": "c2", "parent_run_id": "run_x", "attachments": [id]}))
        .unwrap_err();
    assert_eq!(
        (error.code, reason(&error)),
        (-32602, "attachment_unavailable")
    );
    assert_eq!(
        h.call("attachment.get", json!({"id": id})).unwrap()["state"],
        "staged"
    );
}

#[test]
fn store_reasons_map_to_invalid_params_without_echoing_names() {
    let error = attachment_error("attachment_unavailable: an attachment was already sent".into());
    assert_eq!(
        (error.code, reason(&error)),
        (-32602, "attachment_unavailable")
    );
    assert_eq!(error.message, "an attachment was already sent");
    let error = attachment_error("busy: too many uploads".into());
    assert_eq!((error.code, reason(&error)), (-32006, "busy"));
    assert_eq!(attachment_error("the vault is locked".into()).code, -32020);
    assert_eq!(attachment_error("attachment not found".into()).code, -32002);
    assert_eq!(
        attachment_error("attachment authentication failed".into()).code,
        -32004
    );
}

#[test]
fn sent_attachments_appear_on_their_message_run_and_history() {
    use aidex_providers::{
        FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
        ProviderId,
    };
    let h = Harness::new();
    let provider = ProviderId::new("fixture").unwrap();
    let fake = Arc::new(
        FakeProvider::new(provider.clone(), Locality::Local)
            .with_model(ModelDescriptor::new(
                ModelRef::new(provider, ModelId::new("small").unwrap()),
                Locality::Local,
                65_536,
            ))
            .with_script(FakeScript::new().then(FakeTurn::text("read it"))),
    );
    h.services
        .models()
        .registry()
        .register_shared(fake.clone())
        .unwrap();
    let notes = h.uploaded("notes.txt", b"attachment words");
    let image = h.uploaded("shot.png", &png(64, 64));
    let send = |params: Value| h.call("message.send", params);
    let message = |command: &str, ids: &[&str]| json!({"session_id": h.session, "content": "", "command_id": command, "attachments": ids});

    // The model cannot read images: refused before anything is stored.
    let refused = send(message("c0", &[&notes, &image])).unwrap_err();
    assert_eq!(
        (refused.code, reason(&refused)),
        (-32602, "attachments_unsupported"),
        "{}",
        refused.message
    );
    let unknown = send(message("c0", &["att_missing"])).unwrap_err();
    assert_eq!(
        (unknown.code, reason(&unknown)),
        (-32602, "attachment_unavailable")
    );
    assert!(fake.requests().is_empty());
    for id in [&notes, &image] {
        assert_eq!(
            h.call("attachment.get", json!({"id": id})).unwrap()["state"],
            "staged"
        );
    }

    let run = send(message("c1", &[&notes])).unwrap();
    assert_eq!(run["mode"], "chat");
    assert_eq!(run["permission_mode"], "supervised");
    assert_eq!(run["message_index"], 0);
    let id = run["id"].as_str().unwrap().to_owned();
    assert_eq!(
        send(message("c1", &[&notes])).unwrap()["id"],
        id.as_str(),
        "a retry after a lost response returns the same run"
    );
    let until = Instant::now() + Duration::from_secs(10);
    while h
        .services
        .native()
        .store()
        .run(&id)
        .unwrap()
        .status
        .active()
    {
        assert!(Instant::now() < until, "the run finishes");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(send(message("c1", &[&notes])).unwrap()["id"], id.as_str());
    let request = &fake.requests()[0];
    assert!(
        request
            .messages
            .iter()
            .any(|m| m.content.contains("attachment words")
                && m.content.contains("source=attachment")),
        "text attachments reach the model as wrapped data"
    );

    let view = h
        .call("conversation.get", json!({"id": h.session}))
        .unwrap();
    let first = &view["messages"][0];
    assert_eq!(first["content"], "[Attached files]");
    assert_eq!(
        first["attachments"],
        json!([{"id": notes, "name": "notes.txt", "kind": "text", "mime": "text/plain", "size": 16}])
    );
    assert!(view["messages"][1].get("attachments").is_none());
    assert_eq!(view["runs"][0]["mode"], "chat");
    let history = h
        .call("history.list", json!({"session_id": h.session}))
        .unwrap();
    assert_eq!(history["messages"][0]["attachments"], first["attachments"]);
    let scoped = h.call("history.list", json!({"run_id": id})).unwrap();
    assert!(scoped["messages"][0].get("attachments").is_none());
    let sent = h.call("attachment.get", json!({"id": notes})).unwrap();
    assert_eq!(
        (sent["state"].clone(), sent["run_id"].clone()),
        (json!("sent"), json!(id))
    );
    let again = send(json!({"session_id": h.session, "content": "more", "command_id": "c2", "attachments": [notes]}))
        .unwrap_err();
    assert_eq!(reason(&again), "attachment_unavailable");
}
