//! Sealing of record metadata and of content bytes in independently
//! authenticated 64 KiB segments, so a paged read decrypts at most two segments.
//!
//! Each sealed unit is `nonce (24) || XChaCha20-Poly1305 ciphertext+tag`. A
//! [`Domain`] separates the AAD of artifacts and attachments. Segment AAD binds
//! the record id, the plaintext SHA-256 recorded in the sealed metadata, the
//! segment index, and the segment count: segments cannot be moved, dropped,
//! reordered, or mixed across records, revisions, or domains.
use crate::StoreError;
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use zeroize::Zeroizing;

/// Plaintext bytes per sealed segment.
pub(crate) const SEGMENT_BYTES: u64 = 64 * 1024;
const NONCE_BYTES: usize = 24;
/// Nonce plus Poly1305 tag added to each sealed unit.
pub(crate) const OVERHEAD: u64 = 40;

/// What a sealed unit belongs to. The label is part of every AAD, so a unit of
/// one domain never opens as the other.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Domain {
    Artifact,
    Attachment,
}

impl Domain {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Artifact => "artifact",
            Self::Attachment => "attachment",
        }
    }

    /// `aidex-native-v1:<label>:<id>:<session>`.
    fn record_aad(self, id: &str, session_id: &str) -> String {
        format!("aidex-native-v1:{}:{id}:{session_id}", self.label())
    }

    /// `aidex-native-v1:<label>-content:<id>:<sha256>:<index>:<count>`.
    fn segment_aad(self, id: &str, sha256: &str, index: u64, count: u64) -> String {
        format!(
            "aidex-native-v1:{}-content:{id}:{sha256}:{index}:{count}",
            self.label()
        )
    }

    fn seal(
        self,
        cipher: &XChaCha20Poly1305,
        aad: &str,
        plain: &[u8],
    ) -> Result<Vec<u8>, StoreError> {
        let mut nonce = [0u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
        let encrypted = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plain,
                    aad: aad.as_bytes(),
                },
            )
            .map_err(|_| format!("{} encryption failed", self.label()))?;
        let mut out = Vec::with_capacity(NONCE_BYTES + encrypted.len());
        out.extend_from_slice(&nonce);
        out.extend(encrypted);
        Ok(out)
    }

    fn open(
        self,
        cipher: &XChaCha20Poly1305,
        aad: &str,
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        if sealed.len() < OVERHEAD as usize {
            return Err(self.truncated());
        }
        cipher
            .decrypt(
                XNonce::from_slice(&sealed[..NONCE_BYTES]),
                Payload {
                    msg: &sealed[NONCE_BYTES..],
                    aad: aad.as_bytes(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| format!("{} authentication failed", self.label()))
    }

    /// The storage-damage error for a short or missing sealed unit.
    pub(crate) fn truncated(self) -> StoreError {
        format!("truncated encrypted {}", self.label())
    }

    /// Seal a record's metadata.
    pub(crate) fn seal_record(
        self,
        cipher: &XChaCha20Poly1305,
        id: &str,
        session_id: &str,
        plain: &[u8],
    ) -> Result<Vec<u8>, StoreError> {
        self.seal(cipher, &self.record_aad(id, session_id), plain)
    }

    /// Open a record's metadata sealed for `id` in `session_id`.
    pub(crate) fn open_record(
        self,
        cipher: &XChaCha20Poly1305,
        id: &str,
        session_id: &str,
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        self.open(cipher, &self.record_aad(id, session_id), sealed)
    }

    /// Seal plaintext segment `index` of content of `size` bytes.
    pub(crate) fn seal_segment(
        self,
        cipher: &XChaCha20Poly1305,
        id: &str,
        sha256: &str,
        size: u64,
        index: u64,
        plain: &[u8],
    ) -> Result<Vec<u8>, StoreError> {
        let (_, len) = segment_span(index, size);
        if plain.len() as u64 + OVERHEAD != len {
            return Err(format!("{} segment has the wrong length", self.label()));
        }
        self.seal(
            cipher,
            &self.segment_aad(id, sha256, index, segment_count(size)),
            plain,
        )
    }

    /// Seal `bytes` as consecutive segments.
    pub(crate) fn seal_content(
        self,
        cipher: &XChaCha20Poly1305,
        id: &str,
        sha256: &str,
        bytes: &[u8],
    ) -> Result<Vec<u8>, StoreError> {
        let size = bytes.len() as u64;
        let mut out = Vec::with_capacity(sealed_len(size) as usize);
        for (index, segment) in bytes.chunks(SEGMENT_BYTES as usize).enumerate() {
            out.extend(self.seal_segment(cipher, id, sha256, size, index as u64, segment)?);
        }
        Ok(out)
    }

    /// Open sealed segment `index` of content of `size` plaintext bytes.
    pub(crate) fn open_segment(
        self,
        cipher: &XChaCha20Poly1305,
        id: &str,
        sha256: &str,
        size: u64,
        index: u64,
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        let (_, len) = segment_span(index, size);
        if sealed.len() as u64 != len {
            return Err(self.truncated());
        }
        self.open(
            cipher,
            &self.segment_aad(id, sha256, index, segment_count(size)),
            sealed,
        )
    }
}

pub(crate) fn cipher(key: &[u8]) -> Result<XChaCha20Poly1305, StoreError> {
    XChaCha20Poly1305::new_from_slice(key).map_err(|_| "invalid record key".into())
}

pub(crate) fn segment_count(size: u64) -> u64 {
    size.div_ceil(SEGMENT_BYTES)
}

/// Total sealed length of `size` plaintext bytes.
pub(crate) fn sealed_len(size: u64) -> u64 {
    size + segment_count(size) * OVERHEAD
}

/// Byte range `(start, len)` of sealed segment `index` within the sealed content.
pub(crate) fn segment_span(index: u64, size: u64) -> (u64, u64) {
    let plain = size
        .saturating_sub(index * SEGMENT_BYTES)
        .min(SEGMENT_BYTES);
    (index * (SEGMENT_BYTES + OVERHEAD), plain + OVERHEAD)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_round_trip_and_refuse_reordering() {
        let cipher = cipher(&[7u8; 32]).unwrap();
        let bytes: Vec<u8> = (0..SEGMENT_BYTES * 2 + 5).map(|n| n as u8).collect();
        let size = bytes.len() as u64;
        let artifact = Domain::Artifact;
        let sealed = artifact
            .seal_content(&cipher, "art_a", "sha", &bytes)
            .unwrap();
        assert_eq!(sealed.len() as u64, sealed_len(size));
        assert_eq!(segment_count(size), 3);
        let mut plain = vec![];
        for index in 0..3 {
            let (start, len) = segment_span(index, size);
            let segment = &sealed[start as usize..(start + len) as usize];
            plain.extend_from_slice(
                &artifact
                    .open_segment(&cipher, "art_a", "sha", size, index, segment)
                    .unwrap(),
            );
        }
        assert_eq!(plain, bytes);
        let (start, len) = segment_span(0, size);
        let first = &sealed[start as usize..(start + len) as usize];
        assert!(
            artifact
                .open_segment(&cipher, "art_a", "sha", size, 1, first)
                .is_err()
        );
        assert!(
            artifact
                .open_segment(&cipher, "art_b", "sha", size, 0, first)
                .is_err()
        );
        assert!(
            artifact
                .open_segment(&cipher, "art_a", "other", size, 0, first)
                .is_err()
        );
        assert!(
            Domain::Attachment
                .open_segment(&cipher, "art_a", "sha", size, 0, first)
                .is_err(),
            "domains are separated"
        );
        assert_eq!(sealed_len(0), 0);
    }

    #[test]
    fn aad_formats_are_stable() {
        assert_eq!(
            Domain::Artifact.record_aad("art_1", "session_1"),
            "aidex-native-v1:artifact:art_1:session_1"
        );
        assert_eq!(
            Domain::Artifact.segment_aad("art_1", "abc", 2, 3),
            "aidex-native-v1:artifact-content:art_1:abc:2:3"
        );
        assert_eq!(
            Domain::Attachment.record_aad("att_1", "session_1"),
            "aidex-native-v1:attachment:att_1:session_1"
        );
        assert_eq!(
            Domain::Attachment.segment_aad("att_1", "abc", 0, 1),
            "aidex-native-v1:attachment-content:att_1:abc:0:1"
        );
        let cipher = cipher(&[9u8; 32]).unwrap();
        let sealed = Domain::Attachment
            .seal_record(&cipher, "att_1", "session_1", b"meta")
            .unwrap();
        assert!(
            Domain::Artifact
                .open_record(&cipher, "att_1", "session_1", &sealed)
                .is_err()
        );
        assert_eq!(
            &*Domain::Attachment
                .open_record(&cipher, "att_1", "session_1", &sealed)
                .unwrap(),
            b"meta"
        );
    }
}
