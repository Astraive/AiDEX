//! Typed RPC adapters for the chat attachments of native conversations.
//!
//! Bytes reach the daemon only two ways: from the desktop in chunks
//! (`attachment.upload.*`, sealed as they arrive), or from a registered
//! workspace through its confined root (`attachment.from_workspace`). The
//! daemon never reads any other host path. Names, metadata and bytes are
//! sealed in the native store with the vault-held record key, so every method
//! except `attachment.upload.abort` needs an unlocked vault (`vault_locked`).
//! Refusals carry `data.reason`; their messages never contain a name, a path,
//! or content, and nothing here records an event.
use crate::{
    errors::{ErrorCode, RpcError},
    methods::native::native_error,
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_native_sessions::{
    ATTACHMENT_LIST_LIMIT, ATTACHMENT_PAGE_BYTES, AttachmentChunk, AttachmentEncoding,
    AttachmentKind, AttachmentMeta, AttachmentReason, AttachmentSource, AttachmentState,
    MAX_UPLOAD_BYTES, NewUpload, UPLOAD_CHUNK_BYTES,
};
use aidex_vault::VaultState;
use aidex_workspace::{Access, WorkspaceError};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs::File, sync::Arc};
use zeroize::Zeroizing;

/// Longest `data` of `attachment.upload.chunk`: the base64 of one chunk.
const MAX_CHUNK_BASE64: usize = UPLOAD_CHUNK_BYTES.div_ceil(3) * 4;

/// Where an uploaded attachment came from. Workspace files are attached only
/// through `attachment.from_workspace`, so a client cannot claim that source.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum UploadSource {
    /// A long paste folded into a file: the user's own words.
    Paste,
    /// A file the user picked, dropped, or pasted.
    File,
}

impl From<UploadSource> for AttachmentSource {
    fn from(source: UploadSource) -> Self {
        match source {
            UploadSource::Paste => Self::Paste,
            UploadSource::File => Self::File,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BeginParams {
    session_id: String,
    name: String,
    size: u64,
    sha256: String,
    #[serde(default)]
    mime: Option<String>,
    #[serde(default)]
    source: Option<UploadSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChunkParams {
    upload_id: String,
    offset: u64,
    /// Standard base64 of the chunk.
    data: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadParams {
    upload_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceParams {
    session_id: String,
    workspace_id: String,
    path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    session_id: String,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadParams {
    id: String,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExcludeParams {
    id: String,
    #[serde(default = "excluded_by_default")]
    excluded: bool,
}

fn excluded_by_default() -> bool {
    true
}

/// `Attachment`: the result of `attachment.get`, `attachment.upload.commit`,
/// `attachment.from_workspace` and `attachment.exclude`, and the entries of
/// `attachment.list`.
#[derive(Clone, Serialize)]
pub struct AttachmentView {
    /// `att_<ULID>`.
    pub id: String,
    pub session_id: String,
    /// Display name (a final path component). Never a path.
    pub name: String,
    pub kind: AttachmentKind,
    /// Sniffed MIME type.
    pub mime: String,
    /// Plaintext bytes.
    pub size: u64,
    /// SHA-256 of the plaintext (lowercase hex).
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    pub animated: bool,
    pub state: AttachmentState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<AttachmentSource>,
    /// Unix milliseconds.
    pub created_at: u64,
    /// The run and user message that sent it (`sent` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_index: Option<usize>,
    /// 0-based order within its message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// Kept out of the model context by `attachment.exclude`.
    pub excluded: bool,
}

/// The [`AttachmentView`] of a record.
pub fn attachment_view(meta: AttachmentMeta) -> AttachmentView {
    AttachmentView {
        id: meta.id,
        session_id: meta.session_id,
        name: meta.name,
        kind: meta.kind,
        mime: meta.mime,
        size: meta.size,
        sha256: meta.sha256,
        pages: meta.pages,
        width: meta.width,
        height: meta.height,
        animated: meta.animated,
        state: meta.state,
        source: meta.source,
        created_at: meta.created_at,
        run_id: meta.run_id,
        message_index: meta.message_index,
        position: meta.position,
        excluded: meta.excluded,
    }
}

/// `AttachmentRef`: what a user message in `conversation.get` and
/// `history.list` says about each attachment sent with it.
#[derive(Clone, Serialize)]
pub struct AttachmentRef {
    pub id: String,
    pub name: String,
    pub kind: AttachmentKind,
    pub mime: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub excluded: bool,
}

impl std::fmt::Debug for AttachmentRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttachmentRef")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

/// The [`AttachmentRef`] of a sent attachment.
pub fn attachment_ref(meta: &AttachmentMeta) -> AttachmentRef {
    AttachmentRef {
        id: meta.id.clone(),
        name: meta.name.clone(),
        kind: meta.kind,
        mime: meta.mime.clone(),
        size: meta.size,
        pages: meta.pages,
        width: meta.width,
        height: meta.height,
        excluded: meta.excluded,
    }
}

/// `attachment.read` result: one page, `utf8` text or `base64` bytes.
#[derive(Clone, Serialize)]
pub struct AttachmentContent {
    pub id: String,
    pub encoding: AttachmentEncoding,
    pub content: String,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub total_bytes: u64,
    pub sha256: String,
}

impl AttachmentContent {
    fn encode(chunk: AttachmentChunk) -> Result<Self, RpcError> {
        let content = match chunk.encoding {
            AttachmentEncoding::Utf8 => String::from_utf8(chunk.bytes)
                .map_err(|_| RpcError::internal("attachment page is not valid UTF-8"))?,
            AttachmentEncoding::Base64 => STANDARD.encode(&chunk.bytes),
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

/// Map a store error: a refusal with a reason (`<reason>: <detail>`) is
/// `invalid_params` with `data.reason`, except `busy` (uploads in flight),
/// which is `unavailable`; everything else maps like the other native-store
/// methods (`not_found`, `vault_locked`, `conflict`).
pub(crate) fn attachment_error(message: String) -> RpcError {
    if let Some((reason, detail)) = AttachmentReason::of(&message) {
        let code = match reason {
            AttachmentReason::Busy => ErrorCode::Unavailable,
            _ => ErrorCode::InvalidParams,
        };
        return RpcError::new(code, aidex_events::redact_text(detail).into_owned())
            .with_data(json!({ "reason": reason.as_str() }));
    }
    native_error(message)
}

fn refusal(reason: AttachmentReason, message: &str) -> RpcError {
    RpcError::invalid_params(message).with_data(json!({ "reason": reason.as_str() }))
}

/// [`attachment_error`] for a chunk or commit: an upload that is gone because
/// the vault locked (which aborts every upload) reports `vault_locked`.
fn upload_error(services: &Services, message: String) -> RpcError {
    if message == "attachment upload not found"
        && services.vault().status().state != VaultState::Unlocked
    {
        return native_error("the vault is locked".into());
    }
    attachment_error(message)
}

/// Decode a chunk into memory that is zeroed when dropped.
fn decode_chunk(data: &str) -> Result<Zeroizing<Vec<u8>>, RpcError> {
    if data.len() > MAX_CHUNK_BASE64 {
        return Err(refusal(
            AttachmentReason::BadOffset,
            &format!("chunks are at most {UPLOAD_CHUNK_BYTES} bytes"),
        ));
    }
    let mut out = Zeroizing::new(vec![0u8; base64::decoded_len_estimate(data.len())]);
    let len = STANDARD
        .decode_slice(data.as_bytes(), &mut out[..])
        .map_err(|_| {
            refusal(
                AttachmentReason::InvalidAttachment,
                "chunk data must be standard base64",
            )
        })?;
    out.truncate(len);
    Ok(out)
}

/// A workspace refusal that names neither the path nor the file.
fn workspace_refusal(error: WorkspaceError) -> RpcError {
    let message = match &error {
        WorkspaceError::NotFound(_) => "the project is not registered",
        WorkspaceError::RootUnavailable { .. } => {
            "the project folder is not available on this computer"
        }
        WorkspaceError::PathNotFound(_) => "the file does not exist in the project",
        WorkspaceError::EscapesRoot(_)
        | WorkspaceError::SymlinkLeaf(_)
        | WorkspaceError::Forbidden(_) => "the file is outside the project",
        WorkspaceError::InvalidPath { .. } => "the path is not a valid project-relative path",
        _ => "the project file cannot be read",
    };
    RpcError::service(error.code(), message).with_data(json!({ "reason": error.reason() }))
}

/// Files whose names look like credentials are never attached, whatever
/// their content.
fn refuse_secret_path(path: &str) -> Result<(), RpcError> {
    if aidex_tools::looks_sensitive(path) || aidex_context::exclude::is_secret_path(path) {
        return Err(refusal(
            AttachmentReason::SensitiveContent,
            "files that look like credentials or keys cannot be attached",
        ));
    }
    Ok(())
}

/// A MIME hint for a workspace file from its extension; it only names the
/// type of text files (the kind is always sniffed from the bytes).
fn mime_hint(name: &str) -> Option<&'static str> {
    let (_, extension) = name.rsplit_once('.')?;
    Some(match extension.to_ascii_lowercase().as_str() {
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/x-yaml",
        "toml" => "application/toml",
        "js" | "mjs" | "cjs" | "jsx" => "application/javascript",
        "ts" | "mts" | "cts" | "tsx" => "application/typescript",
        "sh" => "application/x-sh",
        "sql" => "application/sql",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        _ => return None,
    })
}

/// `attachment.from_workspace`: read a file of a registered workspace through
/// its confined root and store it `staged`.
fn from_workspace(services: &Services, p: WorkspaceParams) -> Result<AttachmentMeta, RpcError> {
    let store = services.native().store();
    // An unknown conversation or a locked vault fails before any file is touched.
    store
        .staged_attachments(&p.session_id, &[])
        .map_err(attachment_error)?;
    let id = aidex_core::EntityId::new(p.workspace_id.trim())
        .map_err(|e| RpcError::invalid_params(format!("workspace_id: {e}")))?;
    let workspace = services
        .workspaces()
        .handle(&id)
        .map_err(workspace_refusal)?;
    let path = aidex_workspace::parse_workspace_path(&p.path).map_err(workspace_refusal)?;
    refuse_secret_path(path.as_str())?;
    // Links inside the root are followed, so the target is checked too.
    let resolved = workspace
        .resolve(&path, Access::Read)
        .map_err(workspace_refusal)?;
    let target = workspace.relativize(&resolved).map_err(workspace_refusal)?;
    refuse_secret_path(target.as_str())?;
    let not_a_file = || {
        refusal(
            AttachmentReason::UnsupportedType,
            "only regular files can be attached",
        )
    };
    // Checked before opening (a FIFO would block the open) and again on the
    // opened handle.
    if !std::fs::metadata(&resolved).is_ok_and(|m| m.is_file()) {
        return Err(not_a_file());
    }
    let unreadable = |error: std::io::Error| {
        RpcError::new(
            ErrorCode::Conflict,
            format!("the project file cannot be read: {}", error.kind()),
        )
        .with_data(json!({"reason": "io"}))
    };
    let mut file = File::open(&resolved).map_err(unreadable)?;
    let metadata = file.metadata().map_err(unreadable)?;
    if !metadata.is_file() {
        return Err(not_a_file());
    }
    if metadata.len() > MAX_UPLOAD_BYTES {
        return Err(refusal(
            AttachmentReason::TooLarge,
            &format!("attachments are limited to {MAX_UPLOAD_BYTES} bytes"),
        ));
    }
    let name = path.as_str().rsplit('/').next().unwrap_or(path.as_str());
    store
        .create_attachment_from_reader(
            &p.session_id,
            name,
            mime_hint(name),
            Some(AttachmentSource::Workspace),
            &mut file,
            metadata.len(),
        )
        .map_err(attachment_error)
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register("attachment.upload.begin", move |_, p| {
        let p: BeginParams = parse_params(p)?;
        let ticket = s
            .native()
            .store()
            .begin_attachment_upload(NewUpload {
                session_id: p.session_id,
                name: p.name,
                size: p.size,
                sha256: p.sha256,
                mime: p.mime,
                source: Some(p.source.unwrap_or(UploadSource::File).into()),
            })
            .map_err(attachment_error)?;
        to_result(&ticket)
    })?;
    let s = services.clone();
    router.register("attachment.upload.chunk", move |_, p| {
        let p: ChunkParams = parse_params(p)?;
        let data = decode_chunk(&p.data)?;
        let received = s
            .native()
            .store()
            .append_upload(&p.upload_id, p.offset, &data)
            .map_err(|e| upload_error(&s, e))?;
        to_result(&json!({ "received": received }))
    })?;
    let s = services.clone();
    router.register("attachment.upload.commit", move |_, p| {
        let p: UploadParams = parse_params(p)?;
        let meta = s
            .native()
            .store()
            .commit_upload(&p.upload_id)
            .map_err(|e| upload_error(&s, e))?;
        to_result(&attachment_view(meta))
    })?;
    let s = services.clone();
    router.register("attachment.upload.abort", move |_, p| {
        let p: UploadParams = parse_params(p)?;
        to_result(&json!({ "aborted": s.native().store().abort_upload(&p.upload_id) }))
    })?;
    let s = services.clone();
    router.register("attachment.from_workspace", move |_, p| {
        let p: WorkspaceParams = parse_params(p)?;
        to_result(&attachment_view(from_workspace(&s, p)?))
    })?;
    let s = services.clone();
    router.register("attachment.list", move |_, p| {
        let p: ListParams = parse_params(p)?;
        let limit = p.limit.unwrap_or(ATTACHMENT_LIST_LIMIT);
        if !(1..=ATTACHMENT_LIST_LIMIT).contains(&limit) {
            return Err(RpcError::invalid_params(format!(
                "page limit must be 1-{ATTACHMENT_LIST_LIMIT}"
            )));
        }
        let page = s
            .native()
            .store()
            .list_attachments(&p.session_id, p.offset.unwrap_or(0), limit)
            .map_err(attachment_error)?;
        let attachments: Vec<AttachmentView> =
            page.attachments.into_iter().map(attachment_view).collect();
        to_result(&json!({
            "attachments": attachments,
            "total_attachments": page.total,
            "offset": page.offset,
            "next_offset": page.next_offset,
            "truncated": page.next_offset.is_some(),
        }))
    })?;
    let s = services.clone();
    router.register("attachment.get", move |_, p| {
        let p: IdParams = parse_params(p)?;
        let meta = s
            .native()
            .store()
            .attachment(&p.id)
            .map_err(attachment_error)?;
        to_result(&attachment_view(meta))
    })?;
    let s = services.clone();
    router.register("attachment.read", move |_, p| {
        let p: ReadParams = parse_params(p)?;
        let limit = p.limit.unwrap_or(ATTACHMENT_PAGE_BYTES);
        if limit == 0 {
            return Err(RpcError::invalid_params("read limit must be positive"));
        }
        let chunk = s
            .native()
            .store()
            .read_attachment(&p.id, p.offset.unwrap_or(0), limit)
            .map_err(attachment_error)?;
        to_result(&AttachmentContent::encode(chunk)?)
    })?;
    let s = services.clone();
    router.register("attachment.delete", move |_, p| {
        let p: IdParams = parse_params(p)?;
        let deleted = s
            .native()
            .store()
            .delete_staged_attachment(&p.id)
            .map_err(attachment_error)?;
        to_result(&json!({ "deleted": deleted }))
    })?;
    let s = services.clone();
    router.register("attachment.exclude", move |_, p| {
        let p: ExcludeParams = parse_params(p)?;
        let meta = s
            .native()
            .store()
            .set_attachment_excluded(&p.id, p.excluded)
            .map_err(attachment_error)?;
        to_result(&attachment_view(meta))
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "attachments_tests.rs"]
mod tests;
