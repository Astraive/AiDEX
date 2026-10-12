//! Artifact records: closed kinds, plan lifecycle, limits, and input validation.
use crate::StoreError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Largest artifact the store accepts (16 MiB).
pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
/// Artifacts up to this size (1 MiB) are sealed inline in the database; larger
/// ones go to encrypted, content-addressed blob files beside it.
pub const INLINE_ARTIFACT_BYTES: usize = 1024 * 1024;
/// Largest page [`SessionStore::read_artifact`](crate::SessionStore::read_artifact) returns (64 KiB).
pub const ARTIFACT_PAGE_BYTES: usize = 64 * 1024;
/// Largest page of metadata [`SessionStore::list_artifacts`](crate::SessionStore::list_artifacts) returns.
pub const ARTIFACT_LIST_LIMIT: usize = 100;
/// Longest artifact name in bytes.
pub const MAX_ARTIFACT_NAME_BYTES: usize = 256;
/// Longest language tag in bytes.
pub const MAX_ARTIFACT_LANGUAGE_BYTES: usize = 32;

/// What an artifact holds. The wire names are the closed set
/// `document | code | data | diagram | plan | web_page | other`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Document,
    Code,
    Data,
    Diagram,
    Plan,
    WebPage,
    Other,
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Code => "code",
            Self::Data => "data",
            Self::Diagram => "diagram",
            Self::Plan => "plan",
            Self::WebPage => "web_page",
            Self::Other => "other",
        }
    }

    /// A reasonable MIME type when a producer names none.
    pub fn default_mime(self) -> &'static str {
        match self {
            Self::Document | Self::Plan | Self::WebPage => "text/markdown",
            Self::Data => "application/json",
            Self::Code | Self::Diagram | Self::Other => "text/plain",
        }
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle of a `plan` artifact. A session has at most one `active` plan:
/// a new plan (or re-activating an old one) supersedes the previous active plan.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Active,
    Superseded,
    Completed,
}

/// How paged reads return an artifact's bytes: `utf8` when the whole content is
/// valid UTF-8 (pages then end on character boundaries), else `base64`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactEncoding {
    Utf8,
    Base64,
}

/// A new artifact. `mime` parameters (`; charset=...`) are dropped and the type
/// is lowercased; an empty `language` means none.
#[derive(Clone)]
pub struct NewArtifact {
    pub name: String,
    pub kind: ArtifactKind,
    pub mime: String,
    pub language: Option<String>,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for NewArtifact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewArtifact")
            .field("kind", &self.kind)
            .field("mime", &self.mime)
            .field("language", &self.language)
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Artifact metadata. It is sealed with the store's record key like the bytes;
/// only the artifact and session ids are plaintext SQL columns.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactMeta {
    pub id: String,
    pub session_id: String,
    pub run_id: Option<String>,
    pub name: String,
    pub kind: ArtifactKind,
    pub mime: String,
    pub language: Option<String>,
    /// Plaintext size in bytes.
    pub size: u64,
    /// SHA-256 of the plaintext bytes (lowercase hex).
    pub sha256: String,
    pub encoding: ArtifactEncoding,
    /// `Some` exactly for `plan` artifacts.
    pub plan_status: Option<PlanStatus>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// One page of a session's artifacts, newest first.
#[derive(Clone, Debug, Serialize)]
pub struct ArtifactPage {
    pub artifacts: Vec<ArtifactMeta>,
    pub total: usize,
    pub offset: usize,
    pub next_offset: Option<usize>,
}

/// One page of an artifact's plaintext bytes.
#[derive(Clone)]
pub struct ArtifactChunk {
    pub id: String,
    pub bytes: Vec<u8>,
    pub encoding: ArtifactEncoding,
    pub offset: u64,
    pub next_offset: Option<u64>,
    pub total_bytes: u64,
    pub sha256: String,
}

impl fmt::Debug for ArtifactChunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArtifactChunk")
            .field("id", &self.id)
            .field("bytes", &self.bytes.len())
            .field("encoding", &self.encoding)
            .field("offset", &self.offset)
            .field("next_offset", &self.next_offset)
            .field("total_bytes", &self.total_bytes)
            .finish_non_exhaustive()
    }
}

/// Validated, normalized descriptive fields of a new artifact.
pub(crate) struct Described {
    pub name: String,
    pub mime: String,
    pub language: Option<String>,
}

pub(crate) fn describe(new: &NewArtifact) -> Result<Described, StoreError> {
    check_size(new.bytes.len())?;
    let name = new.name.trim();
    if name.is_empty() || name.len() > MAX_ARTIFACT_NAME_BYTES || name.chars().any(char::is_control)
    {
        return Err(format!(
            "invalid artifact name: 1-{MAX_ARTIFACT_NAME_BYTES} bytes without control characters"
        ));
    }
    let mime = new
        .mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !valid_mime(&mime) {
        return Err("invalid artifact mime: expected type/subtype".into());
    }
    let language = match new.language.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(language) => {
            let language = language.to_ascii_lowercase();
            if language.len() > MAX_ARTIFACT_LANGUAGE_BYTES
                || !language
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+#._-".contains(&b))
            {
                return Err(format!(
                    "invalid artifact language: 1-{MAX_ARTIFACT_LANGUAGE_BYTES} of [a-z0-9+#._-]"
                ));
            }
            Some(language)
        }
    };
    Ok(Described {
        name: name.into(),
        mime,
        language,
    })
}

pub(crate) fn check_size(len: usize) -> Result<(), StoreError> {
    if len > MAX_ARTIFACT_BYTES {
        return Err(format!(
            "invalid artifact size: {len} bytes exceeds the {MAX_ARTIFACT_BYTES}-byte limit"
        ));
    }
    Ok(())
}

pub(crate) fn encoding_of(bytes: &[u8]) -> ArtifactEncoding {
    if std::str::from_utf8(bytes).is_ok() {
        ArtifactEncoding::Utf8
    } else {
        ArtifactEncoding::Base64
    }
}

fn valid_mime(mime: &str) -> bool {
    let token = |part: &str| {
        (1..=63).contains(&part.len())
            && part
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphanumeric())
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    mime.split_once('/')
        .is_some_and(|(kind, subtype)| token(kind) && token(subtype))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new(name: &str, mime: &str, language: Option<&str>) -> NewArtifact {
        NewArtifact {
            name: name.into(),
            kind: ArtifactKind::Code,
            mime: mime.into(),
            language: language.map(Into::into),
            bytes: b"fn main() {}".to_vec(),
        }
    }

    #[test]
    fn descriptive_fields_are_normalized_and_bounded() {
        let d = describe(&new(
            "  main.rs ",
            "Text/X-Rust; charset=utf-8",
            Some("Rust"),
        ))
        .unwrap();
        assert_eq!(
            (d.name.as_str(), d.mime.as_str(), d.language.as_deref()),
            ("main.rs", "text/x-rust", Some("rust"))
        );
        assert!(
            describe(&new("main.rs", "text/plain", Some(" ")))
                .unwrap()
                .language
                .is_none()
        );
        assert!(describe(&new("c++", "text/plain", Some("c++"))).is_ok());
        for (name, mime, language) in [
            ("", "text/plain", None),
            ("bad\nname", "text/plain", None),
            (&"n".repeat(MAX_ARTIFACT_NAME_BYTES + 1), "text/plain", None),
            ("ok", "text", None),
            ("ok", "text/", None),
            ("ok", "text/plain/extra", None),
            ("ok", "text/plain", Some("rust lang")),
            (
                "ok",
                "text/plain",
                Some(&"r".repeat(MAX_ARTIFACT_LANGUAGE_BYTES + 1)),
            ),
        ] {
            assert!(
                describe(&new(name, mime, language)).is_err(),
                "{name:?} {mime:?} {language:?}"
            );
        }
    }

    #[test]
    fn wire_names_and_debug_stay_closed_and_private() {
        assert_eq!(
            serde_json::to_value(ArtifactKind::WebPage).unwrap(),
            "web_page"
        );
        assert!(serde_json::from_value::<ArtifactKind>(serde_json::json!("patch")).is_err());
        assert_eq!(
            serde_json::to_value(PlanStatus::Superseded).unwrap(),
            "superseded"
        );
        let mut secret = new("notes", "text/plain", None);
        secret.bytes = b"private artifact body".to_vec();
        assert!(!format!("{secret:?}").contains("private artifact body"));
        assert_eq!(encoding_of(&[0xff, 0xfe]), ArtifactEncoding::Base64);
        assert_eq!(encoding_of("héllo".as_bytes()), ArtifactEncoding::Utf8);
    }
}
