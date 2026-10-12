//! Typed RPC adapters for the encrypted artifacts of native sessions.
//!
//! Artifacts live in the native store (`aidex-native-sessions`), sealed with
//! the vault-held record key: every method needs an unlocked vault and fails
//! with `vault_locked` otherwise. Producers (harness tools) create artifacts;
//! these methods list, read, delete, and track plan status.
use crate::{
    errors::RpcError,
    methods::native::native_error,
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_native_sessions::{
    ARTIFACT_LIST_LIMIT, ARTIFACT_PAGE_BYTES, ArtifactChunk, ArtifactEncoding, PlanStatus,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    session_id: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdParams {
    id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadParams {
    id: String,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanParams {
    id: String,
    status: PlanStatus,
}

/// `artifact.read` result: one page, `utf8` text or `base64` bytes.
#[derive(Clone, Debug, Serialize)]
pub struct ArtifactContent {
    pub id: String,
    pub encoding: ArtifactEncoding,
    pub content: String,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub total_bytes: u64,
    pub sha256: String,
}

impl ArtifactContent {
    fn encode(chunk: ArtifactChunk) -> Result<Self, RpcError> {
        let content = match chunk.encoding {
            ArtifactEncoding::Utf8 => String::from_utf8(chunk.bytes)
                .map_err(|_| RpcError::internal("artifact page is not valid UTF-8"))?,
            ArtifactEncoding::Base64 => STANDARD.encode(&chunk.bytes),
        };
        Ok(Self {
            id: chunk.id,
            encoding: chunk.encoding,
            content,
            offset: chunk.offset,
            next_offset: chunk.next_offset,
            total_bytes: chunk.total_bytes,
            sha256: chunk.sha256,
        })
    }
}

/// Input mistakes are `invalid_params`; everything else maps like the other
/// native-store methods (`not_found`, `vault_locked`, `conflict`).
fn artifact_error(message: String) -> RpcError {
    if message.starts_with("invalid artifact") {
        RpcError::invalid_params(aidex_events::redact_text(&message).into_owned())
    } else {
        native_error(message)
    }
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register("artifact.list", move |_, p| {
        let p: ListParams = parse_params(p)?;
        let limit = p.limit.unwrap_or(ARTIFACT_LIST_LIMIT);
        if !(1..=ARTIFACT_LIST_LIMIT).contains(&limit) {
            return Err(RpcError::invalid_params(format!(
                "page limit must be 1-{ARTIFACT_LIST_LIMIT}"
            )));
        }
        let page = s
            .native()
            .store()
            .list_artifacts(&p.session_id, p.offset.unwrap_or(0), limit)
            .map_err(artifact_error)?;
        to_result(&json!({
            "artifacts": page.artifacts,
            "total_artifacts": page.total,
            "offset": page.offset,
            "next_offset": page.next_offset,
            "truncated": page.next_offset.is_some(),
        }))
    })?;
    let s = services.clone();
    router.register("artifact.get", move |_, p| {
        let p: IdParams = parse_params(p)?;
        to_result(&s.native().store().artifact(&p.id).map_err(artifact_error)?)
    })?;
    let s = services.clone();
    router.register("artifact.read", move |_, p| {
        let p: ReadParams = parse_params(p)?;
        let limit = p.limit.unwrap_or(ARTIFACT_PAGE_BYTES);
        if limit == 0 {
            return Err(RpcError::invalid_params("read limit must be positive"));
        }
        let chunk = s
            .native()
            .store()
            .read_artifact(&p.id, p.offset.unwrap_or(0), limit)
            .map_err(artifact_error)?;
        to_result(&ArtifactContent::encode(chunk)?)
    })?;
    let s = services.clone();
    router.register("artifact.delete", move |_, p| {
        let p: IdParams = parse_params(p)?;
        let deleted = s
            .native()
            .store()
            .delete_artifact(&p.id)
            .map_err(artifact_error)?;
        to_result(&json!({ "deleted": deleted }))
    })?;
    let s = services.clone();
    router.register("artifact.plan.update", move |_, p| {
        let p: PlanParams = parse_params(p)?;
        to_result(
            &s.native()
                .store()
                .set_plan_status(&p.id, p.status)
                .map_err(artifact_error)?,
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use aidex_native_sessions::{ArtifactKind, INLINE_ARTIFACT_BYTES, NewArtifact};
    use aidex_vault::SecretString;
    use serde_json::Value;

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
                .init(&SecretString::from("fixture-passphrase".to_owned()), false)
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
            let ctx = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
            Self {
                _drive: drive,
                services,
                router,
                ctx,
                session,
            }
        }
        fn create(&self, name: &str, kind: ArtifactKind, bytes: Vec<u8>) -> String {
            self.services
                .native()
                .store()
                .create_artifact(
                    &self.session,
                    None,
                    NewArtifact {
                        name: name.into(),
                        kind,
                        mime: kind.default_mime().into(),
                        language: None,
                        bytes,
                    },
                )
                .unwrap()
                .id
        }
        fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
            self.router.call(&self.ctx, method, params)
        }
        fn code(&self, method: &str, params: Value) -> i32 {
            let error = self.call(method, params).unwrap_err();
            assert!(error.kind().is_some(), "a documented error code");
            error.code
        }
        /// Assemble every `artifact.read` page into the original bytes.
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
                let page = self.call("artifact.read", params).unwrap();
                assert!(serde_json::to_vec(&page).unwrap().len() < 900 * 1024);
                let content = page["content"].as_str().unwrap();
                match page["encoding"].as_str().unwrap() {
                    "utf8" => bytes.extend_from_slice(content.as_bytes()),
                    "base64" => bytes.extend(STANDARD.decode(content).unwrap()),
                    other => panic!("unknown encoding {other}"),
                }
                assert!(page["total_bytes"].as_u64().unwrap() >= bytes.len() as u64);
                if page["next_offset"].is_null() {
                    return bytes;
                }
                offset = page["next_offset"].clone();
            }
        }
    }

    #[test]
    fn list_get_and_paged_reads_round_trip_text_and_binary() {
        let h = Harness::new();
        let text = "# Report\nnaïve café ✓\n".repeat(5000);
        let doc = h.create(
            "report.md",
            ArtifactKind::Document,
            text.clone().into_bytes(),
        );
        let mut binary: Vec<u8> = (0..=255u8)
            .cycle()
            .take(INLINE_ARTIFACT_BYTES + 7)
            .collect();
        binary[0] = 0xff;
        let data = h.create("dump.bin", ArtifactKind::Data, binary.clone());

        let listed = h
            .call("artifact.list", json!({"session_id": h.session}))
            .unwrap();
        assert_eq!(listed["total_artifacts"], 2);
        assert_eq!(listed["truncated"], false);
        assert_eq!(listed["artifacts"][0]["id"], data.as_str());
        assert_eq!(listed["artifacts"][1]["name"], "report.md");
        assert_eq!(listed["artifacts"][1]["kind"], "document");
        assert_eq!(listed["artifacts"][1]["encoding"], "utf8");
        assert!(listed["artifacts"][1]["plan_status"].is_null());
        let first = h
            .call(
                "artifact.list",
                json!({"session_id": h.session, "offset": 0, "limit": 1}),
            )
            .unwrap();
        assert_eq!(first["artifacts"].as_array().unwrap().len(), 1);
        assert_eq!(first["next_offset"], 1);
        assert_eq!(first["truncated"], true);

        let meta = h.call("artifact.get", json!({"id": doc})).unwrap();
        assert_eq!(meta["size"], text.len());
        assert_eq!(meta["mime"], "text/markdown");
        assert_eq!(meta["session_id"], h.session.as_str());

        let page = h
            .call("artifact.read", json!({"id": doc, "limit": 10}))
            .unwrap();
        assert_eq!(page["encoding"], "utf8");
        assert_eq!(page["offset"], 0);
        assert_eq!(page["total_bytes"], text.len());
        assert_eq!(page["sha256"], meta["sha256"]);
        assert_eq!(h.download(&doc, Some(1000)), text.as_bytes());
        assert_eq!(h.download(&doc, None), text.as_bytes());
        let page = h
            .call("artifact.read", json!({"id": data, "limit": 1 << 20}))
            .unwrap();
        assert_eq!(page["encoding"], "base64");
        assert_eq!(
            page["next_offset"], ARTIFACT_PAGE_BYTES,
            "limits clamp to 64 KiB"
        );
        assert_eq!(h.download(&data, None), binary);
        assert_eq!(h.download(&data, Some(4097)), binary);
    }

    #[test]
    fn plan_status_updates_supersede_and_refuse_other_kinds() {
        let h = Harness::new();
        let first = h.create("plan one", ArtifactKind::Plan, b"# one".to_vec());
        let second = h.create("plan two", ArtifactKind::Plan, b"# two".to_vec());
        let note = h.create("note", ArtifactKind::Document, b"note".to_vec());
        let status =
            |id: &str| h.call("artifact.get", json!({"id": id})).unwrap()["plan_status"].clone();
        assert_eq!(status(&first), "superseded");
        assert_eq!(status(&second), "active");
        let done = h
            .call(
                "artifact.plan.update",
                json!({"id": second, "status": "completed"}),
            )
            .unwrap();
        assert_eq!(done["plan_status"], "completed");
        h.call(
            "artifact.plan.update",
            json!({"id": first, "status": "active"}),
        )
        .unwrap();
        assert_eq!(status(&first), "active");
        assert_eq!(status(&second), "completed");
        assert_eq!(
            h.code(
                "artifact.plan.update",
                json!({"id": note, "status": "completed"})
            ),
            -32004
        );
        assert_eq!(
            h.code(
                "artifact.plan.update",
                json!({"id": first, "status": "abandoned"})
            ),
            -32602
        );
    }

    #[test]
    fn delete_validation_and_missing_entities_map_to_stable_codes() {
        let h = Harness::new();
        let id = h.create("x.rs", ArtifactKind::Code, b"fn x() {}".to_vec());
        assert_eq!(
            h.call("artifact.delete", json!({"id": id})).unwrap(),
            json!({"deleted": true})
        );
        assert_eq!(
            h.call("artifact.delete", json!({"id": id})).unwrap(),
            json!({"deleted": false})
        );
        assert_eq!(h.code("artifact.get", json!({"id": id})), -32002);
        assert_eq!(h.code("artifact.read", json!({"id": id})), -32002);
        assert_eq!(
            h.code("artifact.list", json!({"session_id": "session_missing"})),
            -32002
        );
        let id = h.create("y.rs", ArtifactKind::Code, b"fn y() {}".to_vec());
        for (method, params) in [
            (
                "artifact.list",
                json!({"session_id": h.session, "limit": 0}),
            ),
            (
                "artifact.list",
                json!({"session_id": h.session, "limit": 101}),
            ),
            ("artifact.list", json!({})),
            ("artifact.get", json!({"id": id, "extra": true})),
            ("artifact.read", json!({"id": id, "limit": 0})),
            ("artifact.read", json!({"id": id, "offset": 1000})),
            ("artifact.delete", Value::Null),
        ] {
            assert_eq!(h.code(method, params.clone()), -32602, "{method} {params}");
        }
    }

    #[test]
    fn damaged_storage_is_a_conflict_not_a_caller_mistake() {
        let h = Harness::new();
        let inline = h.create("inline.md", ArtifactKind::Document, b"# body".to_vec());
        let blob = h.create(
            "big.bin",
            ArtifactKind::Data,
            vec![9; INLINE_ARTIFACT_BYTES + 1],
        );
        let db = rusqlite::Connection::open(h.services.paths().sessions().join("native.sqlite3"))
            .unwrap();
        db.execute(
            "UPDATE artifacts SET content=zeroblob(length(content)) WHERE id=?",
            [inline.as_str()],
        )
        .unwrap();
        db.execute(
            "UPDATE artifacts SET blob='../native.sqlite3' WHERE id=?",
            [blob.as_str()],
        )
        .unwrap();
        for id in [&inline, &blob] {
            assert_eq!(h.code("artifact.read", json!({"id": id})), -32004, "{id}");
        }
        assert_eq!(
            h.call("artifact.delete", json!({"id": blob})).unwrap(),
            json!({"deleted": true}),
            "a damaged artifact can still be deleted"
        );
    }

    #[test]
    fn artifacts_are_unavailable_while_the_vault_is_locked() {
        let h = Harness::new();
        let id = h.create("plan", ArtifactKind::Plan, b"# private plan".to_vec());
        h.services.vault().lock();
        for (method, params) in [
            ("artifact.list", json!({"session_id": h.session})),
            ("artifact.get", json!({"id": id})),
            ("artifact.read", json!({"id": id})),
            ("artifact.delete", json!({"id": id})),
            (
                "artifact.plan.update",
                json!({"id": id, "status": "completed"}),
            ),
        ] {
            let error = h.call(method, params).unwrap_err();
            assert_eq!(error.code, -32020, "{method}");
            assert!(!error.message.contains("private plan"));
        }
        h.services
            .vault()
            .unlock(&SecretString::from("fixture-passphrase".to_owned()))
            .unwrap();
        assert_eq!(h.download(&id, None), b"# private plan");
    }
}
