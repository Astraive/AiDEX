//! Dependency-free HTML handling: a forgiving tokenizer, character
//! references, and readable-text extraction.

mod entities;
mod extract;
pub(crate) mod tokenizer;

pub use entities::decode as decode_entities;
pub(crate) use extract::truncate_chars;
pub use extract::{Readable, extract, extract_until, plain_text};
