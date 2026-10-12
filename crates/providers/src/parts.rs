//! Image and PDF parts carried by user messages for one request.
//!
//! Parts are request-only: [`crate::ChatMessage::parts`] is `#[serde(skip)]`,
//! so persisted transcripts, sealed records, and older daemons never see
//! them. The harness resolves attachment bytes for each request; the
//! protocol mappers base64-encode them into the vendor's content blocks.

use std::{fmt, sync::Arc};

use base64::Engine as _;

/// Most image and PDF parts on one message.
pub const MAX_PARTS_PER_MESSAGE: usize = 20;

/// Image MIME types a part may carry. The bytes must match the type.
pub const IMAGE_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// The MIME type of every PDF part.
pub const PDF_MIME: &str = "application/pdf";

/// Token estimate for an image of unknown size, and the cap for any image.
pub const MAX_IMAGE_TOKENS: u32 = 4_800;

/// Token estimate per PDF page (extracted text plus a page image).
pub const PDF_PAGE_TOKENS: u32 = 3_000;

/// Longest PDF display name accepted on a part, in bytes.
const MAX_PART_NAME_BYTES: usize = 255;

/// An image or PDF the model reads with a user message.
///
/// `Debug` reports only the type and size, never the bytes or the name.
#[derive(Clone, Eq, PartialEq)]
pub enum ContentPart {
    /// `mime` is one of [`IMAGE_MIMES`] and matches the bytes.
    Image { mime: String, data: Arc<[u8]> },
    /// A PDF (`%PDF-`). `name` is display-only (1-255 bytes, no control
    /// characters); vendors that need a document name get a sanitized copy.
    Pdf {
        name: String,
        data: Arc<[u8]>,
        pages: u32,
    },
}

impl fmt::Debug for ContentPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Image { mime, data } => f
                .debug_struct("Image")
                .field("mime", mime)
                .field("bytes", &data.len())
                .finish(),
            Self::Pdf { data, pages, .. } => f
                .debug_struct("Pdf")
                .field("bytes", &data.len())
                .field("pages", pages)
                .finish_non_exhaustive(),
        }
    }
}

impl ContentPart {
    pub fn image(mime: impl Into<String>, data: impl Into<Arc<[u8]>>) -> Self {
        Self::Image {
            mime: mime.into(),
            data: data.into(),
        }
    }

    pub fn pdf(name: impl Into<String>, data: impl Into<Arc<[u8]>>, pages: u32) -> Self {
        Self::Pdf {
            name: name.into(),
            data: data.into(),
            pages,
        }
    }

    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }

    pub fn is_pdf(&self) -> bool {
        matches!(self, Self::Pdf { .. })
    }

    /// The raw bytes.
    pub fn data(&self) -> &[u8] {
        match self {
            Self::Image { data, .. } | Self::Pdf { data, .. } => data,
        }
    }

    /// The image MIME type, or `application/pdf`.
    pub fn mime(&self) -> &str {
        match self {
            Self::Image { mime, .. } => mime,
            Self::Pdf { .. } => PDF_MIME,
        }
    }

    /// Size of the base64 encoding that reaches the vendor (vendor request
    /// budgets are measured on it).
    pub fn encoded_len(&self) -> usize {
        base64_len(self.data().len())
    }

    /// `(width, height)` read from the image header; `None` for PDFs and
    /// headers that cannot be read.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match self {
            Self::Image { data, .. } => image_dimensions(data),
            Self::Pdf { .. } => None,
        }
    }

    /// Prompt tokens this part is expected to cost: `ceil(w/28) * ceil(h/28)`
    /// for an image (at most [`MAX_IMAGE_TOKENS`], which is also used when
    /// the size is unknown) and [`PDF_PAGE_TOKENS`] per PDF page. These are
    /// deliberate over-estimates for budgeting, never base64 length.
    pub fn estimated_tokens(&self) -> u32 {
        self.estimated_tokens_with(PDF_PAGE_TOKENS)
    }

    /// [`Self::estimated_tokens`] with `pdf_page_tokens` per PDF page, the
    /// family's [`crate::PartLimits::pdf_page_tokens`].
    pub fn estimated_tokens_with(&self, pdf_page_tokens: u32) -> u32 {
        match self {
            Self::Image { data, .. } => {
                image_dimensions(data).map_or(MAX_IMAGE_TOKENS, |(w, h)| {
                    let tiles = u64::from(w.div_ceil(28)) * u64::from(h.div_ceil(28));
                    u32::try_from(tiles.clamp(1, u64::from(MAX_IMAGE_TOKENS)))
                        .unwrap_or(MAX_IMAGE_TOKENS)
                })
            }
            Self::Pdf { pages, .. } => (*pages).max(1).saturating_mul(pdf_page_tokens),
        }
    }

    /// The standard base64 encoding of the bytes.
    pub(crate) fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.data())
    }

    /// `data:<mime>;base64,<bytes>`.
    pub(crate) fn data_url(&self) -> String {
        let mime = self.mime();
        let mut url = String::with_capacity(13 + mime.len() + self.encoded_len());
        url.push_str("data:");
        url.push_str(mime);
        url.push_str(";base64,");
        base64::engine::general_purpose::STANDARD.encode_string(self.data(), &mut url);
        url
    }

    /// Structural checks; the error describes the rule, never the content.
    pub(crate) fn check(&self) -> Result<(), &'static str> {
        match self {
            Self::Image { mime, data } => {
                if !IMAGE_MIMES.contains(&mime.as_str()) {
                    return Err("image parts must be PNG, JPEG, GIF, or WebP");
                }
                if image_mime(data) != Some(mime.as_str()) {
                    return Err("an image part's bytes do not match its MIME type");
                }
            }
            Self::Pdf { name, data, pages } => {
                if !data.starts_with(b"%PDF-") {
                    return Err("PDF parts must contain a PDF document");
                }
                if *pages == 0 {
                    return Err("PDF parts need a positive page count");
                }
                if name.trim().is_empty()
                    || name.len() > MAX_PART_NAME_BYTES
                    || name.chars().any(char::is_control)
                {
                    return Err("PDF part names must be 1-255 bytes without control characters");
                }
            }
        }
        Ok(())
    }
}

/// Length of the padded standard base64 encoding of `len` bytes.
pub(crate) fn base64_len(len: usize) -> usize {
    len.div_ceil(3).saturating_mul(4)
}

/// The image MIME type the bytes' signature identifies (PNG, JPEG, GIF, or
/// WebP); `None` for anything else.
pub fn image_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn be16(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 2)?;
    Some(u32::from(u16::from_be_bytes([bytes[0], bytes[1]])))
}

fn le16(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 2)?;
    Some(u32::from(u16::from_le_bytes([bytes[0], bytes[1]])))
}

fn le24(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 3)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]))
}

/// `(width, height)` from a PNG (IHDR), GIF (logical screen), WebP
/// (VP8/VP8L/VP8X), or JPEG (SOFn) header; `None` when it cannot be read.
pub fn image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let dims = match image_mime(data)? {
        "image/png" => {
            if data.get(12..16)? != b"IHDR" {
                return None;
            }
            let word = |at: usize| -> Option<u32> {
                let bytes = data.get(at..at + 4)?;
                Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            };
            (word(16)?, word(20)?)
        }
        "image/gif" => (le16(data, 6)?, le16(data, 8)?),
        "image/webp" => match data.get(12..16)? {
            b"VP8 " => (le16(data, 26)? & 0x3FFF, le16(data, 28)? & 0x3FFF),
            b"VP8L" => {
                if *data.get(20)? != 0x2F {
                    return None;
                }
                let bytes = data.get(21..25)?;
                let bits = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
                ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
            }
            b"VP8X" => (le24(data, 24)? + 1, le24(data, 27)? + 1),
            _ => return None,
        },
        _ => jpeg_dimensions(data)?,
    };
    (dims.0 > 0 && dims.1 > 0).then_some(dims)
}

/// Walk JPEG marker segments to the first start-of-frame marker.
fn jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut at = 2;
    loop {
        if *data.get(at)? != 0xFF {
            return None;
        }
        while *data.get(at)? == 0xFF {
            at += 1;
        }
        let marker = *data.get(at)?;
        match marker {
            // Markers without a length: TEM, RST0-7, SOI.
            0x01 | 0xD0..=0xD8 => at += 1,
            // End of image or start of scan before any frame header.
            0xD9 | 0xDA => return None,
            0xC0..=0xCF if !matches!(marker, 0xC4 | 0xC8 | 0xCC) => {
                return Some((be16(data, at + 6)?, be16(data, at + 4)?));
            }
            _ => {
                let length = usize::try_from(be16(data, at + 1)?).ok()?;
                if length < 2 {
                    return None;
                }
                at = at.checked_add(1 + length)?;
            }
        }
    }
}

/// The number of frames in a GIF, or `None` when its block structure
/// cannot be read.
fn gif_frames(data: &[u8]) -> Option<usize> {
    if image_mime(data) != Some("image/gif") {
        return None;
    }
    let table = |flags: u8| 3usize << ((flags & 0x07) + 1);
    let skip_sub_blocks = |mut at: usize| -> Option<usize> {
        loop {
            let size = usize::from(*data.get(at)?);
            at += 1;
            if size == 0 {
                return Some(at);
            }
            at = at.checked_add(size)?;
        }
    };
    let flags = *data.get(10)?;
    let mut at = 13;
    if flags & 0x80 != 0 {
        at += table(flags);
    }
    let mut frames = 0;
    loop {
        match *data.get(at)? {
            0x2C => {
                frames += 1;
                if frames > 1 {
                    return Some(frames);
                }
                let flags = *data.get(at + 9)?;
                at += 10;
                if flags & 0x80 != 0 {
                    at += table(flags);
                }
                // LZW minimum code size, then the image data sub-blocks.
                at = skip_sub_blocks(at + 1)?;
            }
            0x21 => at = skip_sub_blocks(at + 2)?,
            0x3B => return Some(frames),
            _ => return None,
        }
    }
}

/// Whether a GIF has more than one frame. A GIF whose structure cannot be
/// read counts as animated, so callers err on the side of a placeholder.
pub fn is_animated_gif(data: &[u8]) -> bool {
    gif_frames(data).is_none_or(|frames| frames > 1)
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Minimal valid headers for each image format and a tiny PDF.

    /// A 1x1 PNG.
    pub fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        bytes
    }

    /// A JPEG with an APP0 segment before its SOF0 header.
    pub fn jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        bytes.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[0x01, 0x01, 0x11, 0x00, 0xFF, 0xD9]);
        bytes
    }

    /// A GIF with `frames` 1x1 frames and a global color table.
    pub fn gif(frames: usize) -> Vec<u8> {
        let mut bytes = b"GIF89a".to_vec();
        bytes.extend_from_slice(&[2, 0, 3, 0, 0x80, 0, 0]);
        bytes.extend_from_slice(&[0; 6]);
        for _ in 0..frames {
            bytes.extend_from_slice(&[0x21, 0xF9, 4, 0, 0, 0, 0, 0]);
            bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
            bytes.extend_from_slice(&[2, 2, 0x4C, 0x01, 0]);
        }
        bytes.push(0x3B);
        bytes
    }

    /// A lossless WebP header of `width` x `height`.
    pub fn webp(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(b"WEBPVP8L");
        bytes.extend_from_slice(&[5, 0, 0, 0, 0x2F]);
        let bits = (width - 1) | ((height - 1) << 14);
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes
    }

    pub fn pdf() -> Vec<u8> {
        b"%PDF-1.7\n1 0 obj << /Type /Page >> endobj\n%%EOF\n".to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::{fixtures::*, *};

    #[test]
    fn signatures_and_headers_give_types_and_dimensions() {
        assert_eq!(image_mime(&png(1, 1)), Some("image/png"));
        assert_eq!(image_dimensions(&png(640, 480)), Some((640, 480)));
        assert_eq!(image_dimensions(&jpeg(1024, 768)), Some((1024, 768)));
        assert_eq!(image_dimensions(&gif(1)), Some((2, 3)));
        assert_eq!(image_dimensions(&webp(300, 200)), Some((300, 200)));
        let mut vp8x = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
        vp8x.extend_from_slice(&[0x3F, 0x1F, 0x00, 0xCF, 0x07, 0x00]);
        assert_eq!(image_dimensions(&vp8x), Some((8000, 2000)));
        let mut vp8 = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0".to_vec();
        vp8.extend_from_slice(&[0, 0, 0, 0x9D, 0x01, 0x2A, 0x80, 0x02, 0xE0, 0x01]);
        assert_eq!(image_dimensions(&vp8), Some((640, 480)));
        for bad in [
            b"BM\0\0".to_vec(),
            b"<svg/>".to_vec(),
            vec![0xFF, 0xD8, 0xFF, 0xDA],
            png(0, 5),
            png(5, 5)[..20].to_vec(),
            vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x01],
        ] {
            assert_eq!(image_dimensions(&bad), None);
        }
        assert_eq!(image_mime(&pdf()), None);
    }

    #[test]
    fn animated_gifs_are_told_apart_from_still_ones() {
        assert!(!is_animated_gif(&gif(1)));
        assert!(is_animated_gif(&gif(2)));
        let truncated = gif(1);
        assert!(is_animated_gif(&truncated[..truncated.len() - 4]));
        assert!(is_animated_gif(&png(1, 1)));
    }

    #[test]
    fn token_estimates_use_dimensions_and_pages_not_bytes() {
        let small = ContentPart::image("image/png", png(512, 512));
        assert_eq!(small.estimated_tokens(), 19 * 19);
        let large = ContentPart::image("image/png", png(2000, 1500));
        assert_eq!(large.estimated_tokens(), 72 * 54);
        let huge = ContentPart::image("image/png", png(8000, 8000));
        assert_eq!(huge.estimated_tokens(), MAX_IMAGE_TOKENS);
        let unknown = ContentPart::image("image/jpeg", vec![0xFF, 0xD8, 0xFF, 0xDA]);
        assert_eq!(unknown.estimated_tokens(), MAX_IMAGE_TOKENS);
        assert_eq!(
            ContentPart::pdf("a.pdf", pdf(), 4).estimated_tokens(),
            4 * PDF_PAGE_TOKENS
        );
        // A family's own page cost; images do not depend on it.
        assert_eq!(
            ContentPart::pdf("a.pdf", pdf(), 4).estimated_tokens_with(560),
            4 * 560
        );
        assert_eq!(small.estimated_tokens_with(560), 19 * 19);
    }

    #[test]
    fn parts_check_types_and_debug_hides_content() {
        assert!(ContentPart::image("image/png", png(1, 1)).check().is_ok());
        assert!(ContentPart::image("image/jpeg", png(1, 1)).check().is_err());
        assert!(
            ContentPart::image("image/svg+xml", b"<svg/>".to_vec())
                .check()
                .is_err()
        );
        assert!(
            ContentPart::image("image/bmp", b"BM".to_vec())
                .check()
                .is_err()
        );
        assert!(ContentPart::pdf("a.pdf", pdf(), 1).check().is_ok());
        assert!(ContentPart::pdf("a.pdf", pdf(), 0).check().is_err());
        assert!(
            ContentPart::pdf("a.pdf", b"PK\x03\x04".to_vec(), 1)
                .check()
                .is_err()
        );
        assert!(ContentPart::pdf(" ", pdf(), 1).check().is_err());
        assert!(ContentPart::pdf("a\nb.pdf", pdf(), 1).check().is_err());
        assert!(ContentPart::pdf("x".repeat(256), pdf(), 1).check().is_err());
        let part = ContentPart::pdf("secret-plan.pdf", pdf(), 1);
        let debug = format!("{part:?}");
        assert!(
            !debug.contains("secret") && !debug.contains("PDF-1.7"),
            "{debug}"
        );
        assert_eq!(part.encoded_len(), part.base64().len());
        assert!(
            part.data_url()
                .starts_with("data:application/pdf;base64,JVBERi0")
        );
    }
}
