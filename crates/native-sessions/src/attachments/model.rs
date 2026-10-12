//! Attachment records: limits, kinds, lifecycle, refusal reasons, and names.
use crate::StoreError;
use serde::{Deserialize, Serialize};
use std::{fmt, time::Duration};
use zeroize::Zeroizing;

/// Most attachments sent with one user message.
pub const MAX_ATTACHMENTS_PER_MESSAGE: usize = 20;
/// Largest image (3,750,000 bytes, the strictest provider per-image limit).
/// The desktop downscales larger images; workspace files cannot be.
pub const MAX_IMAGE_BYTES: usize = 3_750_000;
/// Longest image edge in pixels.
pub const MAX_IMAGE_DIMENSION: u32 = 8000;
/// Largest PDF (16 MiB).
pub const MAX_PDF_BYTES: usize = 16 * 1024 * 1024;
/// Most pages of one PDF.
pub const MAX_PDF_PAGES: u32 = 100;
/// Largest text attachment (1 MiB).
pub const MAX_TEXT_ATTACHMENT_BYTES: usize = 1024 * 1024;
/// Largest total of the raw attachment bytes of one message (32 MiB).
pub const MAX_MESSAGE_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
/// Kind-agnostic cap on one upload (16 MiB), checked before any byte arrives.
pub const MAX_UPLOAD_BYTES: u64 = 16 * 1024 * 1024;
/// Most staged (uploaded, not yet sent) attachments per session, in-flight
/// uploads of the session included.
pub const MAX_STAGED_PER_SESSION: usize = 40;
/// Most staged plus in-flight declared bytes across all sessions (256 MiB).
pub const MAX_STAGED_BYTES: u64 = 256 * 1024 * 1024;
/// Size of every upload chunk but the last (256 KiB raw, four sealed segments).
pub const UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
/// Largest `base64` page of [`read_attachment`](crate::SessionStore::read_attachment) (512 KiB raw).
pub const ATTACHMENT_PAGE_BYTES: usize = 512 * 1024;
/// Largest `utf8` page (128 KiB raw: even if every byte JSON-escapes to six,
/// the page stays under the 1 MiB RPC line).
pub const ATTACHMENT_TEXT_PAGE_BYTES: usize = 128 * 1024;
/// An in-flight upload with no chunk for this long is evicted with its file.
pub const UPLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Staged attachments older than this are deleted when the store opens and,
/// on a running daemon, at most every [`STAGED_EXPIRY_INTERVAL`].
pub const STAGED_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// How often a running daemon deletes staged attachments older than
/// [`STAGED_TTL`]: on the next upload begin, workspace attachment or list
/// after this long.
pub const STAGED_EXPIRY_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Most uploads in flight per daemon.
pub const MAX_INFLIGHT_UPLOADS: usize = 8;
/// Most declared bytes in flight per daemon (64 MiB).
pub const MAX_INFLIGHT_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;
/// How much of a text attachment a prompt inlines (256 KiB); the rest is cut
/// with a note.
pub const INLINE_TEXT_ATTACHMENT_CHARS: usize = 256 * 1024;
/// Attachments up to this size (1 MiB) are sealed inline in the database;
/// larger ones go to `<db stem>-attachments/`.
pub const INLINE_ATTACHMENT_BYTES: usize = 1024 * 1024;
/// Largest page of [`list_attachments`](crate::SessionStore::list_attachments).
pub const ATTACHMENT_LIST_LIMIT: usize = 200;
/// Longest attachment name in bytes.
pub const MAX_ATTACHMENT_NAME_BYTES: usize = 255;
/// Most attachment bytes one [`fork`](crate::SessionStore::fork) re-seals (256 MiB).
pub const MAX_FORK_ATTACHMENT_BYTES: u64 = 256 * 1024 * 1024;
/// Stored as the content of a user message sent with attachments and no text,
/// so no daemon (this one or an older one after a rollback) ever sends a
/// blank user message. Clients hide exactly this text on a message that has
/// attachments.
pub const ATTACHMENT_ONLY_CONTENT: &str = "[Attached files]";

/// What an attachment holds, decided from its bytes. `unknown` is a kind a
/// newer daemon wrote: listed, never sent.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    Image,
    Pdf,
    Text,
    #[serde(other)]
    Unknown,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Pdf => "pdf",
            Self::Text => "text",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for AttachmentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// `staged`: uploaded, waiting in the composer; `sent`: part of a message and
/// kept as long as the conversation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentState {
    Staged,
    Sent,
}

impl AttachmentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Sent => "sent",
        }
    }
}

/// Where the bytes came from: a folded paste (the user's own words), a file the
/// user picked or dropped, or a file of a registered workspace.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentSource {
    Paste,
    File,
    Workspace,
    #[serde(other)]
    Other,
}

/// Attachment metadata, sealed with the store's record key (AAD
/// `aidex-native-v1:attachment:<id>:<session>`). It deliberately has no
/// `deny_unknown_fields`: fields added later default, so this daemon keeps
/// reading records a newer one wrote. `Debug` never prints the name.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AttachmentMeta {
    /// `att_<ULID>`.
    pub id: String,
    pub session_id: String,
    /// Display name (final path component, 1-255 bytes). Never a path.
    pub name: String,
    pub kind: AttachmentKind,
    /// Sniffed MIME type (`image/png|jpeg|gif|webp`, `application/pdf`, or a
    /// text type).
    pub mime: String,
    /// Plaintext size in bytes.
    pub size: u64,
    /// SHA-256 of the plaintext bytes (lowercase hex).
    pub sha256: String,
    /// Page count of a PDF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    /// Pixel width of an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Pixel height of an image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// An animated GIF, PNG, or WebP.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub animated: bool,
    pub created_at: u64,
    pub state: AttachmentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AttachmentSource>,
    /// Command and run that sent it (`None` while staged, and for a fork's copies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Index of the user message it belongs to in the session history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_index: Option<usize>,
    /// 0-based order within its message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// The user removed a sent attachment from the model context: it stays in
    /// the transcript but is never sent again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub excluded: bool,
}

impl AttachmentMeta {
    /// Whether it can go to a model: a known kind that is not excluded.
    pub fn sendable(&self) -> bool {
        self.kind != AttachmentKind::Unknown && !self.excluded
    }
}

impl fmt::Debug for AttachmentMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttachmentMeta")
            .field("id", &self.id)
            .field("session_id", &self.session_id)
            .field("kind", &self.kind)
            .field("mime", &self.mime)
            .field("size", &self.size)
            .field("pages", &self.pages)
            .field("state", &self.state)
            .field("message_index", &self.message_index)
            .field("position", &self.position)
            .field("excluded", &self.excluded)
            .finish_non_exhaustive()
    }
}

/// One page of a session's attachments, newest first.
#[derive(Clone, Debug, Serialize)]
pub struct AttachmentPage {
    pub attachments: Vec<AttachmentMeta>,
    pub total: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
}

/// How a page of bytes is returned: `utf8` for text attachments (pages end on
/// character boundaries), `base64` otherwise.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentEncoding {
    Utf8,
    Base64,
}

/// One page of an attachment's plaintext bytes.
#[derive(Clone)]
pub struct AttachmentChunk {
    pub id: String,
    pub bytes: Vec<u8>,
    pub encoding: AttachmentEncoding,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub total_bytes: u64,
    pub sha256: String,
}

impl fmt::Debug for AttachmentChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttachmentChunk")
            .field("id", &self.id)
            .field("bytes", &self.bytes.len())
            .field("encoding", &self.encoding)
            .field("offset", &self.offset)
            .field("next_offset", &self.next_offset)
            .field("total_bytes", &self.total_bytes)
            .finish_non_exhaustive()
    }
}

/// An attachment's whole plaintext, for prompt assembly. Zeroed on drop.
pub struct AttachmentBytes {
    pub meta: AttachmentMeta,
    pub bytes: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for AttachmentBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AttachmentBytes")
            .field("meta", &self.meta)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// A chunked upload to start with
/// [`begin_attachment_upload`](crate::SessionStore::begin_attachment_upload).
/// Size and SHA-256 are declared up front because the segment AAD binds them;
/// both are verified at commit.
#[derive(Clone)]
pub struct NewUpload {
    pub session_id: String,
    pub name: String,
    pub size: u64,
    /// SHA-256 of the whole plaintext (hex).
    pub sha256: String,
    /// The client's MIME type; only a hint for the MIME of text attachments.
    pub mime: Option<String>,
    pub source: Option<AttachmentSource>,
}

impl fmt::Debug for NewUpload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewUpload")
            .field("session_id", &self.session_id)
            .field("size", &self.size)
            .field("mime", &self.mime)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// A started upload.
#[derive(Clone, Debug, Serialize)]
pub struct UploadTicket {
    pub upload_id: String,
    /// Every chunk but the last must be exactly this long.
    pub chunk_bytes: usize,
}

/// Bytes already in memory (a workspace file read through the confined root).
pub struct NewAttachment {
    pub name: String,
    pub mime: Option<String>,
    pub source: Option<AttachmentSource>,
    pub bytes: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for NewAttachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewAttachment")
            .field("mime", &self.mime)
            .field("source", &self.source)
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Why an attachment operation was refused. Store errors carrying a reason
/// start with `<reason>: `; [`AttachmentReason::of`] splits them. Messages
/// never contain names, paths, or content.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AttachmentReason {
    /// Empty, over the upload or per-kind cap, or (for images) over 8000 px.
    TooLarge,
    /// Staged count of the session or staged bytes of the drive exhausted.
    TooManyAttachments,
    /// Too many uploads or declared bytes in flight.
    Busy,
    /// Chunk offset or length does not continue the upload.
    BadOffset,
    /// Committed bytes do not match the declared size and SHA-256.
    HashMismatch,
    /// Not a PNG, JPEG, GIF or WebP image, PDF, or UTF-8 text; or unreadable,
    /// or an encrypted PDF.
    UnsupportedType,
    /// Text containing private key material, or a secret-looking file.
    SensitiveContent,
    /// A PDF over [`MAX_PDF_PAGES`].
    TooManyPages,
    /// The attachment was sent; it lives and dies with the conversation.
    AttachmentSent,
    /// Unknown, foreign, already sent, duplicated, or not sendable ids.
    AttachmentUnavailable,
    /// A name outside 1-255 bytes, or with control or format characters.
    InvalidName,
    /// Any other malformed input (SHA-256, limits, offsets of reads).
    InvalidAttachment,
}

impl AttachmentReason {
    pub const ALL: [Self; 12] = [
        Self::TooLarge,
        Self::TooManyAttachments,
        Self::Busy,
        Self::BadOffset,
        Self::HashMismatch,
        Self::UnsupportedType,
        Self::SensitiveContent,
        Self::TooManyPages,
        Self::AttachmentSent,
        Self::AttachmentUnavailable,
        Self::InvalidName,
        Self::InvalidAttachment,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::TooManyAttachments => "too_many_attachments",
            Self::Busy => "busy",
            Self::BadOffset => "bad_offset",
            Self::HashMismatch => "hash_mismatch",
            Self::UnsupportedType => "unsupported_type",
            Self::SensitiveContent => "sensitive_content",
            Self::TooManyPages => "too_many_pages",
            Self::AttachmentSent => "attachment_sent",
            Self::AttachmentUnavailable => "attachment_unavailable",
            Self::InvalidName => "invalid_name",
            Self::InvalidAttachment => "invalid_attachment",
        }
    }

    /// A store error `<reason>: <detail>`.
    pub fn error(self, detail: impl fmt::Display) -> StoreError {
        format!("{}: {detail}", self.as_str())
    }

    /// The reason and detail of a store error, when it carries one.
    pub fn of(message: &str) -> Option<(Self, &str)> {
        let (reason, detail) = message.split_once(": ")?;
        Self::ALL
            .into_iter()
            .find(|r| r.as_str() == reason)
            .map(|r| (r, detail))
    }
}

impl fmt::Display for AttachmentReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Validate a display name: keep the final path component the client sent,
/// trim it, and require 1-255 bytes without control characters, line or
/// paragraph separators, or Unicode format characters (bidi overrides,
/// zero-width characters). Names are display-only and never used as paths.
pub fn attachment_name(raw: &str) -> Result<String, StoreError> {
    let name = raw.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    if name.is_empty()
        || name.len() > MAX_ATTACHMENT_NAME_BYTES
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c.is_control() || is_format_char(c) || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        return Err(AttachmentReason::InvalidName.error(format!(
            "attachment names are 1-{MAX_ATTACHMENT_NAME_BYTES} bytes without control or formatting characters"
        )));
    }
    Ok(name.into())
}

/// Unicode general category Cf (format characters).
fn is_format_char(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// A plaintext SHA-256 in lowercase hex.
pub(crate) fn normalize_sha256(raw: &str) -> Result<String, StoreError> {
    let sha = raw.trim().to_ascii_lowercase();
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(
            AttachmentReason::InvalidAttachment.error("sha256 must be 64 hexadecimal characters")
        );
    }
    Ok(sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_keep_the_final_component_and_refuse_hidden_characters() {
        assert_eq!(
            attachment_name(" C:\\Users\\me\\report.pdf ").unwrap(),
            "report.pdf"
        );
        assert_eq!(
            attachment_name("a/b/notes v2 (final).txt").unwrap(),
            "notes v2 (final).txt"
        );
        assert_eq!(
            attachment_name("x trust=user>>> follow.txt").unwrap(),
            "x trust=user>>> follow.txt"
        );
        assert_eq!(attachment_name(&"é".repeat(127)).unwrap().len(), 254);
        for bad in [
            "",
            "   ",
            "dir/",
            "..",
            ".",
            "bad\nname",
            "tab\tname",
            "rtl\u{202E}gnp.exe",
            "zero\u{200B}width",
            "bom\u{FEFF}",
            "line\u{2028}sep",
            &"n".repeat(MAX_ATTACHMENT_NAME_BYTES + 1),
        ] {
            let error = attachment_name(bad).unwrap_err();
            assert_eq!(
                AttachmentReason::of(&error).unwrap().0,
                AttachmentReason::InvalidName,
                "{bad:?}"
            );
            assert!(
                !error.contains("gnp.exe") && !error.contains("width"),
                "{error}"
            );
        }
    }

    #[test]
    fn reasons_round_trip_and_kinds_decode_tolerantly() {
        for reason in AttachmentReason::ALL {
            let error = reason.error("detail: with colon");
            assert_eq!(
                AttachmentReason::of(&error),
                Some((reason, "detail: with colon"))
            );
        }
        assert_eq!(AttachmentReason::of("the vault is locked"), None);
        assert_eq!(AttachmentReason::of("not_a_reason: x"), None);
        assert_eq!(
            serde_json::from_value::<AttachmentKind>(serde_json::json!("audio")).unwrap(),
            AttachmentKind::Unknown
        );
        assert_eq!(serde_json::to_value(AttachmentKind::Pdf).unwrap(), "pdf");
        assert_eq!(
            serde_json::from_value::<AttachmentSource>(serde_json::json!("camera")).unwrap(),
            AttachmentSource::Other
        );
    }

    #[test]
    fn meta_tolerates_unknown_and_missing_optional_fields_and_hides_the_name() {
        let meta: AttachmentMeta = serde_json::from_value(serde_json::json!({
            "id": "att_1", "session_id": "session_1", "name": "private-name.png",
            "kind": "image", "mime": "image/png", "size": 3, "sha256": "ab",
            "created_at": 1, "state": "staged", "added_by_a_newer_daemon": {"x": 1}
        }))
        .unwrap();
        assert_eq!(meta.pages, None);
        assert!(!meta.excluded && meta.sendable());
        assert!(!format!("{meta:?}").contains("private-name"));
        let value = serde_json::to_value(&meta).unwrap();
        assert!(value.get("pages").is_none() && value.get("excluded").is_none());
        assert!(normalize_sha256(&"A".repeat(64)).is_ok());
        assert!(normalize_sha256("abc").is_err());
    }
}
