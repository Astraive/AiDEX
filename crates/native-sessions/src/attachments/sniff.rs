//! Attachment kinds decided from the bytes, never from the client's MIME type.
//!
//! - `image`: PNG, JPEG, GIF, or WebP by magic bytes, with width and height read
//!   from the headers (an image whose header cannot be read is refused).
//! - `pdf`: `%PDF-`. Encrypted PDFs are refused. Pages are the page objects
//!   found in the file and inside Flate-compressed object streams, or the
//!   largest `/Count` of a page tree node, whichever is larger (at least 1).
//! - `text`: valid UTF-8 without NUL bytes; private key material is refused.
use super::model::{
    AttachmentKind, AttachmentReason, MAX_IMAGE_BYTES, MAX_IMAGE_DIMENSION, MAX_PDF_BYTES,
    MAX_PDF_PAGES, MAX_TEXT_ATTACHMENT_BYTES,
};
use crate::StoreError;
use std::{collections::HashSet, io::Read};

/// What the bytes are.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Sniffed {
    pub kind: AttachmentKind,
    pub mime: String,
    pub pages: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub animated: bool,
}

/// Decide the kind of `bytes` and check its per-kind limits. `declared_mime`
/// only picks the MIME type of text.
pub fn sniff_attachment(bytes: &[u8], declared_mime: Option<&str>) -> Result<Sniffed, StoreError> {
    if bytes.is_empty() {
        return Err(AttachmentReason::TooLarge.error("the attachment is empty"));
    }
    if let Some(format) = ImageFormat::of(bytes) {
        return image(bytes, format);
    }
    if bytes.starts_with(b"%PDF-") {
        return pdf(bytes);
    }
    text(bytes, declared_mime)
}

fn unsupported(detail: &str) -> StoreError {
    AttachmentReason::UnsupportedType.error(detail)
}

#[derive(Clone, Copy)]
enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
}

impl ImageFormat {
    fn of(b: &[u8]) -> Option<Self> {
        if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
            Some(Self::Png)
        } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Self::Jpeg)
        } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
            Some(Self::Gif)
        } else if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }

    fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

fn image(b: &[u8], format: ImageFormat) -> Result<Sniffed, StoreError> {
    if b.len() > MAX_IMAGE_BYTES {
        return Err(AttachmentReason::TooLarge
            .error(format!("images are limited to {MAX_IMAGE_BYTES} bytes")));
    }
    let (width, height, animated) = match format {
        ImageFormat::Png => png(b),
        ImageFormat::Jpeg => jpeg(b),
        ImageFormat::Gif => gif(b),
        ImageFormat::Webp => webp(b),
    }
    .filter(|(w, h, _)| *w > 0 && *h > 0)
    .ok_or_else(|| unsupported("the image could not be read"))?;
    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        return Err(AttachmentReason::TooLarge.error(format!(
            "images are limited to {MAX_IMAGE_DIMENSION} pixels per side"
        )));
    }
    Ok(Sniffed {
        kind: AttachmentKind::Image,
        mime: format.mime().into(),
        pages: None,
        width: Some(width),
        height: Some(height),
        animated,
    })
}

fn be16(b: &[u8]) -> u32 {
    u32::from(u16::from_be_bytes([b[0], b[1]]))
}
fn le16(b: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes([b[0], b[1]]))
}
fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}
fn le24(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], 0])
}

/// IHDR is the first chunk; an `acTL` chunk before the first `IDAT` marks APNG.
fn png(b: &[u8]) -> Option<(u32, u32, bool)> {
    if b.get(12..16)? != b"IHDR" {
        return None;
    }
    let (width, height) = (be32(b.get(16..20)?), be32(b.get(20..24)?));
    let mut at = 8usize;
    let mut animated = false;
    while let Some(header) = b.get(at..at + 8) {
        let kind = &header[4..8];
        if kind == b"acTL" {
            animated = true;
            break;
        }
        if kind == b"IDAT" || kind == b"IEND" {
            break;
        }
        at = at
            .checked_add(12)?
            .checked_add(be32(&header[..4]) as usize)?;
    }
    Some((width, height, animated))
}

/// Walk the marker segments to the first frame header (SOFn).
fn jpeg(b: &[u8]) -> Option<(u32, u32, bool)> {
    let mut at = 2usize;
    loop {
        if *b.get(at)? != 0xFF {
            return None;
        }
        while *b.get(at)? == 0xFF {
            at += 1;
        }
        let marker = b[at];
        at += 1;
        match marker {
            0x01 | 0xD0..=0xD8 => continue,
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = be16(b.get(at..at + 2)?) as usize;
        if len < 2 {
            return None;
        }
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let frame = b.get(at + 2..at + 7)?;
            return Some((be16(&frame[3..5]), be16(&frame[1..3]), false));
        }
        at += len;
    }
}

/// Logical screen size; more than one image descriptor marks an animation.
fn gif(b: &[u8]) -> Option<(u32, u32, bool)> {
    let screen = b.get(6..13)?;
    let (width, height) = (le16(&screen[..2]), le16(&screen[2..4]));
    let table = |flags: u8| {
        if flags & 0x80 != 0 {
            3usize << ((flags & 7) + 1)
        } else {
            0
        }
    };
    let mut at = 13 + table(screen[4]);
    let mut frames = 0;
    loop {
        match b.get(at) {
            Some(0x21) => at = skip_sub_blocks(b, at + 2)?,
            Some(0x2C) => {
                frames += 1;
                if frames > 1 {
                    break;
                }
                let descriptor = b.get(at + 1..at + 10)?;
                at += 10 + table(descriptor[8]) + 1;
                at = skip_sub_blocks(b, at)?;
            }
            _ => break,
        }
    }
    (frames > 0).then_some((width, height, frames > 1))
}

fn skip_sub_blocks(b: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let len = *b.get(at)? as usize;
        at += 1;
        if len == 0 {
            return Some(at);
        }
        at += len;
    }
}

/// `VP8 ` (lossy), `VP8L` (lossless), or `VP8X` (extended, with the animation flag).
fn webp(b: &[u8]) -> Option<(u32, u32, bool)> {
    match b.get(12..16)? {
        b"VP8 " => {
            let frame = b.get(20..30)?;
            if frame[3..6] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((
                le16(&frame[6..8]) & 0x3FFF,
                le16(&frame[8..10]) & 0x3FFF,
                false,
            ))
        }
        b"VP8L" => {
            let header = b.get(20..25)?;
            if header[0] != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1, false))
        }
        b"VP8X" => {
            let header = b.get(20..30)?;
            Some((
                le24(&header[4..7]) + 1,
                le24(&header[7..10]) + 1,
                header[0] & 0x02 != 0,
            ))
        }
        _ => None,
    }
}

fn pdf(b: &[u8]) -> Result<Sniffed, StoreError> {
    if b.len() > MAX_PDF_BYTES {
        return Err(
            AttachmentReason::TooLarge.error(format!("PDFs are limited to {MAX_PDF_BYTES} bytes"))
        );
    }
    if pdf_encrypted(b) {
        return Err(unsupported("encrypted PDFs cannot be attached"));
    }
    let pages = pdf_pages(b)?;
    if pages > MAX_PDF_PAGES {
        return Err(AttachmentReason::TooManyPages
            .error(format!("PDFs are limited to {MAX_PDF_PAGES} pages")));
    }
    Ok(Sniffed {
        kind: AttachmentKind::Pdf,
        mime: "application/pdf".into(),
        pages: Some(pages),
        width: None,
        height: None,
        animated: false,
    })
}

fn is_ws(b: u8) -> bool {
    matches!(b, 0 | b'\t' | b'\n' | 0x0C | b'\r' | b' ')
}

fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

fn is_regular(b: u8) -> bool {
    !is_ws(b) && !is_delimiter(b)
}

/// `b[at..]` starts with `word` and `word` ends there (end of input or a
/// non-regular byte follows).
fn token(b: &[u8], at: usize, word: &[u8]) -> bool {
    b[at..].starts_with(word) && b.get(at + word.len()).is_none_or(|&c| !is_regular(c))
}

fn skip_ws(b: &[u8], mut at: usize) -> usize {
    while b.get(at).is_some_and(|&c| is_ws(c)) {
        at += 1;
    }
    at
}

/// Unsigned integer at `at`: `(value, end)`.
fn integer(b: &[u8], at: usize) -> Option<(u64, usize)> {
    let end = at
        + b[at.min(b.len())..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count();
    if end == at || end - at > 18 {
        return None;
    }
    let value = std::str::from_utf8(&b[at..end]).ok()?.parse().ok()?;
    Some((value, end))
}

/// `<int> <int> R` at `at`.
fn reference(b: &[u8], at: usize) -> bool {
    let Some((_, end)) = integer(b, at) else {
        return false;
    };
    let next = skip_ws(b, end);
    if next == end {
        return false;
    }
    let Some((_, end)) = integer(b, next) else {
        return false;
    };
    let next = skip_ws(b, end);
    next > end && token(b, next, b"R")
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// An `/Encrypt` entry (an indirect reference or a dictionary), as written in
/// the trailer or a cross-reference stream dictionary of an encrypted PDF.
fn pdf_encrypted(b: &[u8]) -> bool {
    let mut from = 0;
    while let Some(at) = find(b, b"/Encrypt", from) {
        from = at + 8;
        if !token(b, at, b"/Encrypt") {
            continue;
        }
        let value = skip_ws(b, at + 8);
        if b[value..].starts_with(b"<<") || reference(b, value) {
            return true;
        }
    }
    false
}

/// Total plaintext a PDF's object streams may inflate to.
const MAX_INFLATED_BYTES: u64 = 64 * 1024 * 1024;

/// Most objects one object stream may hold (`/N`), which bounds its offset
/// table (16 bytes an entry); real streams hold far fewer.
const MAX_STREAM_OBJECTS: u64 = 1_000_000;

#[derive(Default)]
struct PageCount {
    /// Object numbers holding a `/Type /Page` dictionary.
    pages: HashSet<u64>,
    /// Page dictionaries outside any numbered object.
    loose: u64,
    /// Largest `/Count` of a `/Type /Pages` node.
    tree: u64,
    inflated: u64,
}

/// Facts about one object (or the bytes before the first one).
#[derive(Default)]
struct Object {
    number: Option<u64>,
    page: bool,
    pages_node: bool,
    count: Option<u64>,
    object_stream: bool,
}

impl PageCount {
    fn close(&mut self, object: &Object) {
        if object.page {
            match object.number {
                Some(number) => {
                    self.pages.insert(number);
                }
                None => self.loose += 1,
            }
        }
        if object.pages_node
            && let Some(count) = object.count
        {
            self.tree = self.tree.max(count);
        }
    }

    fn total(&self) -> u32 {
        let found = self.pages.len() as u64 + self.loose;
        u32::try_from(found.max(self.tree).max(1)).unwrap_or(u32::MAX)
    }
}

fn pdf_pages(b: &[u8]) -> Result<u32, StoreError> {
    let mut count = PageCount::default();
    scan(b, None, &mut count, true)?;
    Ok(count.total())
}

/// The object number of an `obj` keyword at `at` (`<num> <gen> obj`).
fn object_header(b: &[u8], at: usize) -> Option<u64> {
    if !token(b, at, b"obj") {
        return None;
    }
    let digits_back = |end: usize| {
        let start = end
            - b[..end]
                .iter()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .count();
        (start < end).then_some(start)
    };
    let ws_back = |end: usize| end - b[..end].iter().rev().take_while(|&&c| is_ws(c)).count();
    let gen_end = ws_back(at);
    if gen_end == at {
        return None;
    }
    let gen_start = digits_back(gen_end)?;
    let number_end = ws_back(gen_start);
    if number_end == gen_start {
        return None;
    }
    let number_start = digits_back(number_end)?;
    if number_start > 0 && is_regular(b[number_start - 1]) {
        return None;
    }
    integer(b, number_start).map(|(n, _)| n)
}

/// Scan objects for page dictionaries. In the file itself (`top`), `obj`
/// headers start objects and object streams are inflated; inside an object
/// stream the caller passes one object per call. Every byte is looked at a
/// bounded number of times, so crafted input cannot make it superlinear.
fn scan(b: &[u8], number: Option<u64>, count: &mut PageCount, top: bool) -> Result<(), StoreError> {
    let mut object = Object {
        number,
        ..Object::default()
    };
    // A stream's dictionary starts after the `obj` header or after the
    // previous stream, so no byte is searched for `/Length` twice.
    let mut dictionary_start = 0usize;
    let mut at = 0usize;
    while at < b.len() {
        match b[at] {
            b'%' => {
                while at < b.len() && b[at] != b'\n' && b[at] != b'\r' {
                    at += 1;
                }
                continue;
            }
            b'o' if top => {
                if let Some(number) = object_header(b, at) {
                    count.close(&object);
                    object = Object {
                        number: Some(number),
                        ..Object::default()
                    };
                    at += 3;
                    dictionary_start = at;
                    continue;
                }
            }
            b's' if top
                && (at == 0 || !is_regular(b[at - 1]))
                && b[at..].starts_with(b"stream") =>
            {
                let data = match b.get(at + 6) {
                    Some(b'\r') if b.get(at + 7) == Some(&b'\n') => at + 8,
                    Some(b'\r' | b'\n') => at + 7,
                    _ => {
                        at += 1;
                        continue;
                    }
                };
                let dictionary = &b[dictionary_start..at];
                let end = stream_end(b, dictionary, data);
                // An object has one stream; a stray second one is not
                // read as an object stream with the first one's dictionary.
                if std::mem::take(&mut object.object_stream) {
                    object_stream(dictionary, &b[data..end], count)?;
                }
                at = end;
                dictionary_start = end;
                continue;
            }
            b'/' => {
                if token(b, at, b"/Type") {
                    let value = skip_ws(b, at + 5);
                    if token(b, value, b"/Page") {
                        object.page = true;
                    } else if token(b, value, b"/Pages") {
                        object.pages_node = true;
                    } else if token(b, value, b"/ObjStm") {
                        object.object_stream = true;
                    }
                    at = value.max(at + 1);
                    continue;
                }
                if token(b, at, b"/Count") {
                    let value = skip_ws(b, at + 6);
                    if let Some((n, _)) = integer(b, value) {
                        object.count = Some(object.count.unwrap_or(0).max(n));
                    }
                }
            }
            _ => {}
        }
        at += 1;
    }
    count.close(&object);
    Ok(())
}

/// Most whitespace between a stream's data and `endstream` that the `/Length`
/// check skips. Writers put an end-of-line marker there, sometimes a few
/// spaces; the bound keeps the check constant-time per stream, since crafted
/// `/Length` values can all point into one long whitespace run.
const MAX_ENDSTREAM_GAP: usize = 64;

/// End of stream data starting at `data`: a direct `/Length` when it lands on
/// `endstream` (past at most [`MAX_ENDSTREAM_GAP`] whitespace bytes), else the
/// next `endstream` keyword, else the end of the file.
fn stream_end(b: &[u8], dictionary: &[u8], data: usize) -> usize {
    if let Some(length) = direct_integer(dictionary, b"/Length")
        && let Some(end) = data.checked_add(usize::try_from(length).unwrap_or(usize::MAX))
        && end <= b.len()
        && let gap = b[end..]
            .iter()
            .take(MAX_ENDSTREAM_GAP)
            .take_while(|&&c| is_ws(c))
            .count()
        && b[end + gap..].starts_with(b"endstream")
    {
        return end;
    }
    find(b, b"endstream", data).unwrap_or(b.len())
}

/// A direct (not indirect) integer value of `key` in a dictionary.
fn direct_integer(dictionary: &[u8], key: &[u8]) -> Option<u64> {
    let mut from = 0;
    while let Some(at) = find(dictionary, key, from) {
        from = at + key.len();
        if !token(dictionary, at, key) {
            continue;
        }
        let value = skip_ws(dictionary, at + key.len());
        if reference(dictionary, value) {
            return None;
        }
        return integer(dictionary, value).map(|(n, _)| n);
    }
    None
}

/// The filter of a stream: `None` (no filter), `Some(true)` for Flate alone,
/// `Some(false)` for anything else.
fn flate_filter(dictionary: &[u8]) -> Option<bool> {
    let at = find(dictionary, b"/Filter", 0)?;
    let mut value = skip_ws(dictionary, at + 7);
    if dictionary.get(value) == Some(&b'[') {
        value = skip_ws(dictionary, value + 1);
        let flate = token(dictionary, value, b"/FlateDecode");
        let after = skip_ws(dictionary, value + 12);
        return Some(flate && dictionary.get(after) == Some(&b']'));
    }
    Some(token(dictionary, value, b"/FlateDecode"))
}

/// Inflate an object stream and scan each object it holds.
fn object_stream(dictionary: &[u8], data: &[u8], count: &mut PageCount) -> Result<(), StoreError> {
    let unreadable = || unsupported("the PDF could not be read");
    let inflated;
    let content: &[u8] = match flate_filter(dictionary) {
        None => data,
        Some(false) => return Ok(()),
        Some(true) => {
            let budget = MAX_INFLATED_BYTES.saturating_sub(count.inflated);
            // Inflated document text is plaintext: zero it when dropped.
            let mut out = zeroize::Zeroizing::new(Vec::new());
            flate2::read::ZlibDecoder::new(data)
                .take(budget + 1)
                .read_to_end(&mut out)
                .map_err(|_| unreadable())?;
            if out.len() as u64 > budget {
                return Err(unreadable());
            }
            count.inflated += out.len() as u64;
            inflated = out;
            &inflated
        }
    };
    let objects = direct_integer(dictionary, b"/N")
        .filter(|&n| n <= MAX_STREAM_OBJECTS)
        .ok_or_else(unreadable)?;
    let first = direct_integer(dictionary, b"/First")
        .and_then(|f| usize::try_from(f).ok())
        .filter(|&f| f <= content.len())
        .ok_or_else(unreadable)?;
    let mut table = Vec::new();
    let mut at = 0usize;
    for _ in 0..objects.min(content.len() as u64) {
        at = skip_ws(content, at);
        let (number, end) = integer(content, at).ok_or_else(unreadable)?;
        at = skip_ws(content, end);
        let (offset, end) = integer(content, at).ok_or_else(unreadable)?;
        at = end;
        let start = usize::try_from(offset)
            .ok()
            .and_then(|o| first.checked_add(o))
            .filter(|&s| s <= content.len())
            .ok_or_else(unreadable)?;
        table.push((number, start));
    }
    // Offsets increase in a valid stream; in order, the objects' spans
    // partition the content, so no byte is scanned twice.
    table.sort_by_key(|&(_, start)| start);
    for (index, &(number, start)) in table.iter().enumerate() {
        let end = table
            .get(index + 1)
            .map_or(content.len(), |&(_, next)| next);
        scan(&content[start..end], Some(number), count, false)?;
    }
    Ok(())
}

fn text(b: &[u8], declared_mime: Option<&str>) -> Result<Sniffed, StoreError> {
    let Ok(text) = std::str::from_utf8(b) else {
        return Err(unsupported(
            "attach PNG, JPEG, GIF or WebP images, PDFs, or UTF-8 text files",
        ));
    };
    if b.contains(&0) {
        return Err(unsupported(
            "attach PNG, JPEG, GIF or WebP images, PDFs, or UTF-8 text files",
        ));
    }
    if b.len() > MAX_TEXT_ATTACHMENT_BYTES {
        return Err(AttachmentReason::TooLarge.error(format!(
            "text attachments are limited to {MAX_TEXT_ATTACHMENT_BYTES} bytes"
        )));
    }
    if contains_private_key(text) {
        return Err(AttachmentReason::SensitiveContent
            .error("the file contains private key material and cannot be attached"));
    }
    Ok(Sniffed {
        kind: AttachmentKind::Text,
        mime: text_mime(declared_mime),
        pages: None,
        width: None,
        height: None,
        animated: false,
    })
}

/// The client's MIME type when it names text, else `text/plain`.
fn text_mime(declared: Option<&str>) -> String {
    const TEXT_LIKE: [&str; 16] = [
        "application/json",
        "application/xml",
        "application/x-yaml",
        "application/yaml",
        "application/toml",
        "application/javascript",
        "application/x-javascript",
        "application/typescript",
        "application/x-typescript",
        "application/x-sh",
        "application/x-shellscript",
        "application/sql",
        "application/graphql",
        "application/x-python",
        "application/x-httpd-php",
        "application/ld+json",
    ];
    let mime = declared
        .and_then(|m| m.split(';').next())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let token = |part: &str| {
        (1..=63).contains(&part.len())
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b))
    };
    let valid = mime
        .split_once('/')
        .is_some_and(|(kind, subtype)| token(kind) && token(subtype));
    let text_like = mime.starts_with("text/")
        || TEXT_LIKE.contains(&mime.as_str())
        || (mime.starts_with("application/")
            && (mime.ends_with("+json") || mime.ends_with("+xml")));
    if valid && text_like {
        mime
    } else {
        "text/plain".into()
    }
}

/// Whether `text` contains a `-----BEGIN … PRIVATE KEY-----` (or PGP private
/// key) block header (the same rule as the context index's key detector).
fn contains_private_key(text: &str) -> bool {
    const BEGIN: &[u8] = b"-----BEGIN ";
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(start) = find(bytes, BEGIN, from) {
        let label_start = start + BEGIN.len();
        let window = &bytes[label_start..(label_start + 64).min(bytes.len())];
        let label = find(window, b"-----", 0).map_or(window, |stop| &window[..stop]);
        if find(label, b"PRIVATE KEY", 0).is_some() {
            return true;
        }
        from = label_start;
    }
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;

    pub(crate) fn png_bytes(width: u32, height: u32) -> Vec<u8> {
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

    fn reason(error: &str) -> AttachmentReason {
        AttachmentReason::of(error).unwrap().0
    }

    #[test]
    fn images_are_sniffed_with_dimensions() {
        let png = sniff_attachment(&png_bytes(640, 480), Some("text/plain")).unwrap();
        assert_eq!(
            (
                png.kind,
                png.mime.as_str(),
                png.width,
                png.height,
                png.animated
            ),
            (
                AttachmentKind::Image,
                "image/png",
                Some(640),
                Some(480),
                false
            )
        );
        let mut apng = png_bytes(10, 10);
        apng.splice(
            33..33,
            [0, 0, 0, 8].into_iter().chain(*b"acTL").chain([0; 12]),
        );
        assert!(sniff_attachment(&apng, None).unwrap().animated);

        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0];
        jpeg.extend([0xFF, 0xFF, 0xC2, 0, 11, 8, 0x01, 0xE0, 0x02, 0x80, 3, 0, 0]);
        let sniffed = sniff_attachment(&jpeg, None).unwrap();
        assert_eq!(
            (sniffed.mime.as_str(), sniffed.width, sniffed.height),
            ("image/jpeg", Some(640), Some(480))
        );
        assert_eq!(
            reason(&sniff_attachment(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2], None).unwrap_err()),
            AttachmentReason::UnsupportedType
        );

        let mut gif = b"GIF89a".to_vec();
        gif.extend([0x20, 0x03, 0x58, 0x02, 0x80, 0, 0]);
        gif.extend([0; 6]);
        let frame = [0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 2, 1, 0, 0];
        gif.extend(frame);
        gif.extend([0x21, 0xF9, 4, 0, 0, 0, 0, 0]);
        let single = [gif.clone(), vec![0x3B]].concat();
        let sniffed = sniff_attachment(&single, None).unwrap();
        assert_eq!(
            (sniffed.width, sniffed.height, sniffed.animated),
            (Some(800), Some(600), false)
        );
        gif.extend(frame);
        gif.push(0x3B);
        assert!(sniff_attachment(&gif, None).unwrap().animated);

        let mut lossy = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0".to_vec();
        lossy.extend([0, 0, 0, 0x9D, 0x01, 0x2A, 0x80, 0x02, 0xE0, 0x01]);
        assert_eq!(sniff_attachment(&lossy, None).unwrap().width, Some(640));
        let mut lossless = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0".to_vec();
        let bits: u32 = 639 | (479 << 14);
        lossless.push(0x2F);
        lossless.extend(bits.to_le_bytes());
        assert_eq!(sniff_attachment(&lossless, None).unwrap().height, Some(480));
        let mut extended = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0".to_vec();
        extended.extend([0x02, 0, 0, 0, 0x7F, 0x02, 0, 0xDF, 0x01, 0]);
        let sniffed = sniff_attachment(&extended, None).unwrap();
        assert_eq!(
            (sniffed.width, sniffed.height, sniffed.animated),
            (Some(640), Some(480), true)
        );
    }

    #[test]
    fn images_over_the_limits_or_unreadable_are_refused() {
        assert_eq!(
            reason(&sniff_attachment(&png_bytes(8001, 10), None).unwrap_err()),
            AttachmentReason::TooLarge
        );
        let mut big = png_bytes(100, 100);
        big.resize(MAX_IMAGE_BYTES + 1, 0);
        assert_eq!(
            reason(&sniff_attachment(&big, None).unwrap_err()),
            AttachmentReason::TooLarge
        );
        assert_eq!(
            reason(&sniff_attachment(&png_bytes(0, 10), None).unwrap_err()),
            AttachmentReason::UnsupportedType
        );
        let truncated = &png_bytes(10, 10)[..20];
        assert_eq!(
            reason(&sniff_attachment(truncated, None).unwrap_err()),
            AttachmentReason::UnsupportedType
        );
        for other in [
            &b"BM\x36\0\0\0\0\0"[..],
            b"\0\0\0\x18ftypheic\0\0\0\0",
            &[0xFF, 0xFE, 0, 0x41],
        ] {
            assert_eq!(
                reason(&sniff_attachment(other, None).unwrap_err()),
                AttachmentReason::UnsupportedType
            );
        }
    }

    pub(crate) fn pdf_with_pages(pages: usize) -> Vec<u8> {
        let mut b =
            b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
                .to_vec();
        let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", i + 3)).collect();
        b.extend(
            format!(
                "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {pages} >>\nendobj\n",
                kids.join(" ")
            )
            .bytes(),
        );
        for i in 0..pages {
            b.extend(
                format!(
                    "{} 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>\nendobj\n",
                    i + 3
                )
                .bytes(),
            );
        }
        b.extend(b"trailer\n<< /Root 1 0 R >>\n%%EOF\n");
        b
    }

    fn deflate(bytes: &[u8]) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    /// A PDF 1.5 file whose page dictionaries all sit in a compressed object stream.
    pub(crate) fn pdf_with_object_stream(pages: usize, count: usize) -> Vec<u8> {
        let mut objects = String::new();
        let mut table = String::new();
        for i in 0..pages {
            table.push_str(&format!("{} {} ", i + 3, objects.len()));
            objects.push_str("<</Type /Page /Parent 2 0 R>>\n");
        }
        let content = format!("{table}\n{objects}");
        let first = table.len() + 1;
        object_stream_pdf(pages, first, content.as_bytes(), count, true)
    }

    /// A PDF 1.5 file with one object stream holding `content` (`objects`
    /// objects from `first`), Flate-compressed when `flate`, and a page tree
    /// whose `/Count` is `count`.
    fn object_stream_pdf(
        objects: usize,
        first: usize,
        content: &[u8],
        count: usize,
        flate: bool,
    ) -> Vec<u8> {
        let (data, filter) = if flate {
            (deflate(content), " /Filter /FlateDecode")
        } else {
            (content.to_vec(), "")
        };
        let mut b = b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec();
        b.extend(format!("2 0 obj\n<< /Type /Pages /Count {count} >>\nendobj\n").bytes());
        b.extend(
            format!(
                "9 0 obj\n<< /Type /ObjStm /N {objects} /First {first}{filter} /Length {} >>\nstream\r\n",
                data.len()
            )
            .bytes(),
        );
        b.extend(&data);
        b.extend(b"\r\nendstream\nendobj\n%%EOF\n");
        b
    }

    /// Sniff crafted input that once took minutes to hours (dictionaries,
    /// object-stream spans and whitespace after `/Length` were rescanned); the
    /// bound is generous so debug builds do not flake.
    fn sniff_in_linear_time(bytes: &[u8]) -> Result<Sniffed, StoreError> {
        let started = std::time::Instant::now();
        let sniffed = sniff_attachment(bytes, None);
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "sniffing took {elapsed:?}"
        );
        sniffed
    }

    /// Object-stream content (`/First 0`) whose offset table alternates
    /// between 0 and the end of the content: `(objects, content)`.
    fn alternating_offsets(bytes: usize) -> (usize, Vec<u8>) {
        let entry = |offset: usize| format!("1 {offset:010} ");
        let pairs = bytes / (2 * entry(0).len());
        let len = pairs * 2 * entry(0).len();
        let mut content = String::with_capacity(len);
        for _ in 0..pairs {
            content.push_str(&entry(0));
            content.push_str(&entry(len));
        }
        (2 * pairs, content.into_bytes())
    }

    #[test]
    fn pdf_page_counting_is_linear_on_crafted_input() {
        // One object followed by many streams without a /Length.
        let mut streams = b"%PDF-1.4\n1 0 obj\n<< >>\n".to_vec();
        while streams.len() < 2 * 1024 * 1024 {
            streams.extend(b"stream\nendstream\n");
        }
        assert_eq!(sniff_in_linear_time(&streams).unwrap().pages, Some(1));

        // Streams whose /Length all land in one long whitespace run that
        // does not end in `endstream`.
        let mut gap = b"%PDF-1.4\n1 0 obj\n<< >>\n".to_vec();
        let block = |length: usize| format!("/Length {length:08}\nstream\nendstream\n");
        let blocks = 20_000;
        let run = gap.len() + blocks * block(0).len();
        for _ in 0..blocks {
            let data = gap.len() + block(0).len() - "endstream\n".len();
            gap.extend(block(run - data).bytes());
        }
        assert_eq!(gap.len(), run);
        gap.resize(run + 2 * 1024 * 1024, b' ');
        gap.extend(b"X\n");
        assert_eq!(sniff_in_linear_time(&gap).unwrap().pages, Some(1));

        // Object streams whose offsets alternate, raw and compressed.
        let (objects, content) = alternating_offsets(2 * 1024 * 1024);
        let raw = object_stream_pdf(objects, 0, &content, 1, false);
        assert_eq!(sniff_in_linear_time(&raw).unwrap().pages, Some(1));
        let (objects, content) = alternating_offsets(4 * 1024 * 1024);
        let compressed = object_stream_pdf(objects, 0, &content, 1, true);
        assert!(compressed.len() < 64 * 1024, "a small file");
        assert_eq!(sniff_in_linear_time(&compressed).unwrap().pages, Some(1));
    }

    #[test]
    fn stream_length_is_used_when_whitespace_precedes_endstream() {
        // The second object's data holds the word `endstream`; only the
        // /Length finds the third object, past a padded end of line.
        let pages = [
            "<</Type /Page>>\n",
            "<</T (endstream) /Type /Page>>\n",
            "<</Type /Page>>\n",
        ];
        let second = pages[0].len();
        let table = format!("3 0 4 {second} 5 {} ", second + pages[1].len());
        let content = format!("{table}{}", pages.concat());
        let mut b = object_stream_pdf(3, table.len(), content.as_bytes(), 1, false);
        let gap = find(&b, b"\r\nendstream\nendobj", 0).unwrap() + 2;
        b.splice(gap..gap, [b' '; 40]);
        assert_eq!(sniff_attachment(&b, None).unwrap().pages, Some(3));
    }

    #[test]
    fn object_stream_offsets_out_of_order_still_count_each_object() {
        let page = "<</Type /Page /Parent 2 0 R>>\n";
        let table = format!("4 {} 3 0\n", page.len());
        let content = format!("{table}{page}{page}");
        let b = object_stream_pdf(2, table.len(), content.as_bytes(), 1, true);
        assert_eq!(sniff_attachment(&b, None).unwrap().pages, Some(2));
    }

    #[test]
    fn object_streams_with_too_many_objects_are_refused() {
        // A table of empty objects, all at /First (the end of the content).
        let objects = MAX_STREAM_OBJECTS as usize + 1;
        let content = b"1 0 ".repeat(objects);
        let b = object_stream_pdf(objects, content.len(), &content, 1, true);
        assert_eq!(
            reason(&sniff_in_linear_time(&b).unwrap_err()),
            AttachmentReason::UnsupportedType
        );
    }

    #[test]
    fn pdf_pages_are_counted_in_the_file_and_in_object_streams() {
        let sniffed = sniff_attachment(&pdf_with_pages(3), Some("image/png")).unwrap();
        assert_eq!(
            (sniffed.kind, sniffed.mime.as_str(), sniffed.pages),
            (AttachmentKind::Pdf, "application/pdf", Some(3))
        );
        assert_eq!(
            sniff_attachment(&pdf_with_object_stream(7, 1), None)
                .unwrap()
                .pages,
            Some(7)
        );
        assert_eq!(
            sniff_attachment(&pdf_with_object_stream(2, 9), None)
                .unwrap()
                .pages,
            Some(9),
            "the page tree count is used when it is larger"
        );
        assert_eq!(
            reason(&sniff_attachment(&pdf_with_object_stream(101, 1), None).unwrap_err()),
            AttachmentReason::TooManyPages
        );
        assert_eq!(
            reason(
                &sniff_attachment(&pdf_with_pages(MAX_PDF_PAGES as usize + 1), None).unwrap_err()
            ),
            AttachmentReason::TooManyPages
        );
        // An incremental update rewrites page 3: still three pages.
        let mut updated = pdf_with_pages(3);
        updated.extend(b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Rotate 90 >>\nendobj\ntrailer\n<< /Root 1 0 R /Prev 9 >>\n");
        assert_eq!(sniff_attachment(&updated, None).unwrap().pages, Some(3));
        assert_eq!(
            sniff_attachment(b"%PDF-1.7\n%%EOF", None).unwrap().pages,
            Some(1)
        );
    }

    #[test]
    fn encrypted_or_corrupt_pdfs_are_refused() {
        let mut encrypted = pdf_with_pages(1);
        encrypted.extend(b"trailer\n<< /Root 1 0 R /Encrypt 5 0 R /ID [<00><00>] >>\n");
        assert_eq!(
            reason(&sniff_attachment(&encrypted, None).unwrap_err()),
            AttachmentReason::UnsupportedType
        );
        let mut inline = pdf_with_pages(1);
        inline.extend(b"trailer<</Encrypt<</Filter/Standard>>>>");
        assert!(sniff_attachment(&inline, None).is_err());
        let mut mention = pdf_with_pages(1);
        mention.extend(b"4 0 obj\n<< /EncryptMetadata false >>\nendobj\n(/Encrypt) Tj\n");
        assert!(
            sniff_attachment(&mention, None).is_ok(),
            "only a real /Encrypt entry counts"
        );

        let mut corrupt = pdf_with_object_stream(2, 1);
        let at = find(&corrupt, b"stream\r\n", 0).unwrap() + 8;
        corrupt[at..at + 4].copy_from_slice(b"junk");
        assert_eq!(
            reason(&sniff_attachment(&corrupt, None).unwrap_err()),
            AttachmentReason::UnsupportedType
        );
    }

    #[test]
    fn text_is_utf8_without_nul_or_keys() {
        let sniffed = sniff_attachment(
            "fn main() {}\n".as_bytes(),
            Some("text/x-rust; charset=utf-8"),
        )
        .unwrap();
        assert_eq!(
            (sniffed.kind, sniffed.mime.as_str()),
            (AttachmentKind::Text, "text/x-rust")
        );
        assert_eq!(
            sniff_attachment(b"{}", Some("application/json"))
                .unwrap()
                .mime,
            "application/json"
        );
        assert_eq!(
            sniff_attachment(b"{}", Some("application/vnd.api+json"))
                .unwrap()
                .mime,
            "application/vnd.api+json"
        );
        assert_eq!(
            sniff_attachment(b"<svg/>", Some("image/svg+xml"))
                .unwrap()
                .mime,
            "text/plain"
        );
        assert_eq!(
            sniff_attachment(b"x", Some("application/octet-stream"))
                .unwrap()
                .mime,
            "text/plain"
        );
        assert_eq!(sniff_attachment(b"x", None).unwrap().mime, "text/plain");
        assert_eq!(
            reason(&sniff_attachment(b"a\0b", None).unwrap_err()),
            AttachmentReason::UnsupportedType
        );
        assert_eq!(
            reason(&sniff_attachment(&[], None).unwrap_err()),
            AttachmentReason::TooLarge
        );
        let key =
            "notes\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\n";
        assert_eq!(
            reason(&sniff_attachment(key.as_bytes(), None).unwrap_err()),
            AttachmentReason::SensitiveContent
        );
        assert!(sniff_attachment(b"-----BEGIN CERTIFICATE-----\nAAAA\n", None).is_ok());
        let big = vec![b'a'; MAX_TEXT_ATTACHMENT_BYTES + 1];
        assert_eq!(
            reason(&sniff_attachment(&big, None).unwrap_err()),
            AttachmentReason::TooLarge
        );
    }
}
