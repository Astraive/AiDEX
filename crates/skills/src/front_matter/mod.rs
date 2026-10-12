//! A small, dependency-free parser for the YAML front matter of `SKILL.md`.
//!
//! It accepts the subset skills use in practice: top-level `key: value` pairs
//! with plain, single-quoted, or double-quoted scalars (multi-line plain and
//! quoted scalars fold to one line), booleans, `|`/`>` block scalars with
//! chomping indicators, inline `[a, "b"]` lists, and `- item` block lists.
//! Nested mappings, flow mappings, and lists of mappings are skipped (their key
//! is ignored); comments and unknown keys are ignored. Errors name a line and a
//! problem but never quote the value, which could hold a pasted credential.

mod lines;
mod scalar;
#[cfg(test)]
mod tests;

use std::{fmt, ops::Range};

use serde::{Deserialize, Serialize};

use lines::{Line, split_lines};
use scalar::{Scalar, parse_block_scalar, parse_flow_list, parse_quoted, plain_scalar};

use crate::manifest::MAX_FRONT_MATTER_BYTES;

/// Most keys one front matter block may hold.
const MAX_KEYS: usize = 256;
/// Most items one list may hold.
const MAX_LIST_ITEMS: usize = 256;

/// A front matter value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FrontMatterValue {
    Bool(bool),
    Text(String),
    List(Vec<String>),
}

impl FrontMatterValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
}

/// Parsed front matter: supported top-level keys in file order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrontMatter {
    entries: Vec<(String, FrontMatterValue)>,
}

/// Why front matter could not be parsed. `line` is 1-based within the file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontMatterError {
    pub line: Option<usize>,
    pub message: String,
}

impl FrontMatterError {
    pub(crate) fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    fn general(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for FrontMatterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "front matter line {line}: {}", self.message),
            None => write!(f, "front matter: {}", self.message),
        }
    }
}

impl std::error::Error for FrontMatterError {}

/// A `SKILL.md` split into its front matter and Markdown body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillDocument {
    /// `None` when the file does not start with a `---` line.
    pub front_matter: Option<FrontMatter>,
    /// The instructions after the front matter, with `\n` line endings and
    /// surrounding blank lines removed.
    pub body: String,
}

impl SkillDocument {
    /// Split `text` at its `---` delimiters and parse the front matter. A UTF-8
    /// byte-order mark and CRLF line endings are accepted.
    pub fn parse(text: &str) -> Result<Self, FrontMatterError> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let Some((front, body)) = split(text)? else {
            return Ok(Self {
                front_matter: None,
                body: normalize_body(text),
            });
        };
        if front.len() > MAX_FRONT_MATTER_BYTES {
            return Err(FrontMatterError::general(format!(
                "front matter is larger than {} KiB",
                MAX_FRONT_MATTER_BYTES / 1024
            )));
        }
        Ok(Self {
            // The front matter starts on the file's second line.
            front_matter: Some(FrontMatter::parse_at(front, 2)?),
            body: normalize_body(body),
        })
    }
}

/// `(front matter, body)`, or `None` when the text does not open with `---`.
fn split(text: &str) -> Result<Option<(&str, &str)>, FrontMatterError> {
    let (first, mut offset) = match text.find('\n') {
        Some(end) => (&text[..end], end + 1),
        None => (text, text.len()),
    };
    if first.trim_end() != "---" {
        return Ok(None);
    }
    let start = offset;
    while offset < text.len() {
        let end = text[offset..]
            .find('\n')
            .map_or(text.len(), |at| offset + at);
        let line = text[offset..end].trim_end();
        if line == "---" || line == "..." {
            let body = text.get(end + 1..).unwrap_or("");
            return Ok(Some((&text[start..offset], body)));
        }
        offset = end + 1;
    }
    Err(FrontMatterError::at(
        1,
        "front matter opened here is not closed with a `---` line",
    ))
}

fn normalize_body(body: &str) -> String {
    let body = body.replace("\r\n", "\n");
    body.trim_start_matches(['\n', '\r']).trim_end().to_owned()
}

impl FrontMatter {
    /// Parse front matter text (without its `---` delimiters).
    pub fn parse(text: &str) -> Result<Self, FrontMatterError> {
        Self::parse_at(text, 1)
    }

    fn parse_at(text: &str, first_line: usize) -> Result<Self, FrontMatterError> {
        let lines = split_lines(text, first_line);
        let entries = parse_keys(&lines)?
            .into_iter()
            .filter_map(|key| Some((key.key.to_owned(), key.value?)))
            .collect();
        Ok(Self { entries })
    }

    pub fn get(&self, key: &str) -> Option<&FrontMatterValue> {
        self.entries
            .iter()
            .find(|(candidate, _)| candidate == key)
            .map(|(_, value)| value)
    }

    /// The first of `keys` that is present (for `user-invocable`/`user_invocable`).
    pub fn get_any(&self, keys: &[&str]) -> Option<(&str, &FrontMatterValue)> {
        keys.iter().find_map(|key| {
            self.entries
                .iter()
                .find(|(candidate, _)| candidate == key)
                .map(|(key, value)| (key.as_str(), value))
        })
    }

    pub fn entries(&self) -> &[(String, FrontMatterValue)] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One top-level key: its value (`None` for null or unsupported values) and the
/// lines it spans, from the key's line up to the next unread line.
struct KeySpan<'a> {
    key: &'a str,
    value: Option<FrontMatterValue>,
    lines: Range<usize>,
}

/// Every top-level key of `lines`, in file order.
fn parse_keys<'a>(lines: &[Line<'a>]) -> Result<Vec<KeySpan<'a>>, FrontMatterError> {
    let mut keys: Vec<KeySpan<'a>> = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        if line.is_ignorable() {
            index += 1;
            continue;
        }
        if line.indent > 0 {
            return Err(FrontMatterError::at(line.number, "unexpected indentation"));
        }
        if line.is_list_item() {
            return Err(FrontMatterError::at(
                line.number,
                "a list item must follow a key",
            ));
        }
        let (key, rest) = split_key(line)?;
        if keys.iter().any(|existing| existing.key == key) {
            return Err(FrontMatterError::at(
                line.number,
                format!("duplicate key `{key}`"),
            ));
        }
        if keys.len() >= MAX_KEYS {
            return Err(FrontMatterError::at(line.number, "too many keys"));
        }
        let (value, next) = parse_value(lines, index + 1, line.number, rest)?;
        keys.push(KeySpan {
            key,
            value,
            lines: index..next,
        });
        index = next;
    }
    Ok(keys)
}

/// The front matter of a `SKILL.md` as written, without the top-level keys in
/// `drop` (each with its value's lines): every other line (other keys, values this
/// parser skips such as nested mappings, comments) is kept verbatim, each ending in
/// `\n`. `None` when the text has no front matter.
pub(crate) fn front_matter_without(
    text: &str,
    drop: &[&str],
) -> Result<Option<String>, FrontMatterError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some((front, _)) = split(text)? else {
        return Ok(None);
    };
    let lines = split_lines(front, 2);
    let dropped: Vec<Range<usize>> = parse_keys(&lines)?
        .into_iter()
        .filter(|key| drop.contains(&key.key))
        .map(|key| key.lines)
        .collect();
    // `front` ends with the line break before the closing `---`: the last split
    // "line" is that break's empty remainder, not a line of its own.
    let count = if front.is_empty() || front.ends_with('\n') {
        lines.len().saturating_sub(1)
    } else {
        lines.len()
    };
    let mut kept = String::new();
    for (index, line) in lines.iter().enumerate().take(count) {
        if !dropped.iter().any(|span| span.contains(&index)) {
            kept.push_str(line.raw);
            kept.push('\n');
        }
    }
    Ok(Some(kept))
}

/// `key: rest` with `key` in `[A-Za-z0-9_.-]+`.
fn split_key<'a>(line: &Line<'a>) -> Result<(&'a str, &'a str), FrontMatterError> {
    let content = line.content;
    let bytes = content.as_bytes();
    let colon = bytes
        .iter()
        .enumerate()
        .position(|(at, byte)| {
            *byte == b':'
                && bytes
                    .get(at + 1)
                    .is_none_or(|next| *next == b' ' || *next == b'\t')
        })
        .ok_or_else(|| FrontMatterError::at(line.number, "expected `key: value`"))?;
    let key = content[..colon].trim_end();
    let valid = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !valid {
        return Err(FrontMatterError::at(
            line.number,
            "keys are letters, digits, `-`, `_`, or `.` followed by `: `",
        ));
    }
    Ok((key, content[colon + 1..].trim_start()))
}

/// Parse the value that starts with `rest` on the key's line; returns the value
/// (`None` for null or unsupported values) and the index of the next unread line.
fn parse_value(
    lines: &[Line<'_>],
    start: usize,
    key_line: usize,
    rest: &str,
) -> Result<(Option<FrontMatterValue>, usize), FrontMatterError> {
    match rest.as_bytes().first() {
        None | Some(b'#') => nested_value(lines, start),
        Some(b'|' | b'>') => {
            let (text, next) = parse_block_scalar(lines, start, key_line, rest)?;
            Ok((Some(FrontMatterValue::Text(text)), next))
        }
        Some(b'[') => {
            let items = parse_flow_list(rest).map_err(|m| FrontMatterError::at(key_line, m))?;
            let next = skip_indented(lines, start);
            Ok((items.map(|items| list(items, key_line)).transpose()?, next))
        }
        // Flow mappings are not supported: skip the value and its continuation.
        Some(b'{') => Ok((None, skip_indented(lines, start))),
        Some(b'"' | b'\'') => {
            let (text, next) = parse_quoted(lines, start, key_line, rest)?;
            Ok((Some(FrontMatterValue::Text(text)), next))
        }
        Some(_) => {
            let (scalar, next) = plain_scalar(lines, start, rest);
            Ok((scalar.into_value(), next))
        }
    }
}

/// A value on the following lines: a block list, or (unsupported) a nested mapping.
fn nested_value(
    lines: &[Line<'_>],
    start: usize,
) -> Result<(Option<FrontMatterValue>, usize), FrontMatterError> {
    let mut index = start;
    let mut children: Vec<&Line<'_>> = Vec::new();
    while index < lines.len() {
        let line = &lines[index];
        if line.content.is_empty() || (line.indent > 0 && line.content.starts_with('#')) {
            index += 1;
            continue;
        }
        if line.indent == 0 && !line.is_list_item() {
            break;
        }
        children.push(line);
        index += 1;
    }
    let Some(first) = children.first() else {
        return Ok((None, index));
    };
    let is_list = children
        .iter()
        .all(|line| line.is_list_item() && line.indent == first.indent);
    if !is_list {
        // A nested mapping or a list with nested content: ignored.
        return Ok((None, index));
    }
    let mut items = Vec::with_capacity(children.len());
    for line in &children {
        let item = line.content[1..].trim_start();
        match scalar::list_item(item).map_err(|m| FrontMatterError::at(line.number, m))? {
            Scalar::Text(text) => items.push(text),
            Scalar::Bool(value) => items.push(value.to_string()),
            Scalar::Null => {}
            // A list of mappings or nested lists: ignored.
            Scalar::Unsupported => return Ok((None, index)),
        }
    }
    Ok((Some(list(items, first.number)?), index))
}

fn list(items: Vec<String>, line: usize) -> Result<FrontMatterValue, FrontMatterError> {
    if items.len() > MAX_LIST_ITEMS {
        return Err(FrontMatterError::at(line, "too many list items"));
    }
    Ok(FrontMatterValue::List(items))
}

/// Skip continuation lines (indented or blank) of a value that is ignored.
fn skip_indented(lines: &[Line<'_>], start: usize) -> usize {
    let mut index = start;
    while index < lines.len() && (lines[index].content.is_empty() || lines[index].indent > 0) {
        index += 1;
    }
    index
}
