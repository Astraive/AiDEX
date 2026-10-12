//! Sealed content shared by artifacts and attachments: the segment crypto, the
//! blob directories, and paged plaintext reads.
pub(crate) mod blobs;
pub(crate) mod crypto;

pub(crate) use blobs::{BlobDir, BlobWriter};
pub(crate) use crypto::{
    Domain, OVERHEAD, SEGMENT_BYTES, cipher, sealed_len, segment_count, segment_span,
};

use crate::StoreError;
use chacha20poly1305::XChaCha20Poly1305;
use zeroize::Zeroizing;

/// Where a record's sealed content lives.
pub(crate) enum Stored<'a> {
    /// The whole sealed content from the row.
    Inline(&'a [u8]),
    /// A blob file in `dir`.
    Blob { dir: &'a BlobDir, address: &'a str },
}

/// Identity of sealed content: the record id and plaintext SHA-256 its segment
/// AAD binds, and its plaintext size.
pub(crate) struct Content<'a> {
    pub domain: Domain,
    pub id: &'a str,
    pub sha256: &'a str,
    pub size: u64,
}

impl Content<'_> {
    /// Sealed segment `index`.
    pub(crate) fn sealed_segment(
        &self,
        stored: &Stored<'_>,
        index: u64,
    ) -> Result<Vec<u8>, StoreError> {
        let (start, len) = segment_span(index, self.size);
        self.sealed_range(stored, start, len)
    }

    fn sealed_range(
        &self,
        stored: &Stored<'_>,
        start: u64,
        len: u64,
    ) -> Result<Vec<u8>, StoreError> {
        match stored {
            Stored::Blob { dir, address } => dir.read(address, sealed_len(self.size), start, len),
            Stored::Inline(content) => {
                if content.len() as u64 != sealed_len(self.size) {
                    return Err(self.domain.truncated());
                }
                Ok(content[start as usize..(start + len) as usize].to_vec())
            }
        }
    }

    /// Decrypt plaintext bytes `[start, end)`, touching only the segments they span.
    pub(crate) fn window(
        &self,
        cipher: &XChaCha20Poly1305,
        stored: &Stored<'_>,
        start: u64,
        end: u64,
    ) -> Result<Zeroizing<Vec<u8>>, StoreError> {
        let end = end.min(self.size);
        if start >= end {
            return Ok(Zeroizing::new(vec![]));
        }
        let (first, last) = (start / SEGMENT_BYTES, (end - 1) / SEGMENT_BYTES);
        let (from, _) = segment_span(first, self.size);
        let (last_start, last_len) = segment_span(last, self.size);
        let sealed = self.sealed_range(stored, from, last_start + last_len - from)?;
        let mut plain = Zeroizing::new(Vec::with_capacity(sealed.len()));
        for index in first..=last {
            let (at, segment_len) = segment_span(index, self.size);
            let at = (at - from) as usize;
            plain.extend_from_slice(&self.domain.open_segment(
                cipher,
                self.id,
                self.sha256,
                self.size,
                index,
                &sealed[at..at + segment_len as usize],
            )?);
        }
        let skip = (start - first * SEGMENT_BYTES) as usize;
        Ok(Zeroizing::new(
            plain[skip..skip + (end - start) as usize].to_vec(),
        ))
    }
}

/// A page of plaintext and the offset of the next one.
pub(crate) type Page = (Vec<u8>, Option<u64>);

/// Why a page cannot start at the requested offset.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PageError {
    /// `offset` is past the end of the content.
    Beyond,
    /// `offset` is inside a UTF-8 character.
    InsideCharacter,
}

/// One page of plaintext bytes from `offset`: at most `limit` bytes, and for
/// `utf8` content a page that starts and ends on character boundaries and
/// always makes progress unless `offset` is the end. `fetch(start, end)`
/// decrypts the plaintext range.
pub(crate) fn page(
    size: u64,
    offset: u64,
    limit: u64,
    utf8: bool,
    fetch: impl FnOnce(u64, u64) -> Result<Zeroizing<Vec<u8>>, StoreError>,
) -> Result<Result<Page, PageError>, StoreError> {
    if offset > size {
        return Ok(Err(PageError::Beyond));
    }
    let wanted = offset.saturating_add(limit).min(size);
    // A UTF-8 character is at most four bytes: read past the page end to find it.
    let fetched = if utf8 { (wanted + 4).min(size) } else { wanted };
    let window = fetch(offset, fetched)?;
    let continuation = |at: u64| at < fetched && window[(at - offset) as usize] & 0xC0 == 0x80;
    let mut end = wanted;
    if utf8 {
        if continuation(offset) {
            return Ok(Err(PageError::InsideCharacter));
        }
        while end > offset && continuation(end) {
            end -= 1;
        }
        if end == offset && offset < size {
            end += 1;
            while continuation(end) {
                end += 1;
            }
        }
    }
    Ok(Ok((
        window[..(end - offset) as usize].to_vec(),
        (end < size).then_some(end),
    )))
}
