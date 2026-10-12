//! Scalars: plain, quoted, block (`|`, `>`), and inline lists.

use super::{FrontMatterError, FrontMatterValue, lines::Line};

/// A parsed scalar before it becomes a [`FrontMatterValue`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Scalar {
    Text(String),
    Bool(bool),
    /// Empty, `~`, or `null`.
    Null,
    /// A nested mapping or list the subset does not support.
    Unsupported,
}

impl Scalar {
    pub fn into_value(self) -> Option<FrontMatterValue> {
        match self {
            Self::Text(text) => Some(FrontMatterValue::Text(text)),
            Self::Bool(value) => Some(FrontMatterValue::Bool(value)),
            Self::Null | Self::Unsupported => None,
        }
    }
}

/// A plain scalar's meaning: booleans and nulls are recognised, the rest is text.
fn interpret_plain(text: &str) -> Scalar {
    if text.is_empty() || text == "~" || text.eq_ignore_ascii_case("null") {
        Scalar::Null
    } else if text.eq_ignore_ascii_case("true") {
        Scalar::Bool(true)
    } else if text.eq_ignore_ascii_case("false") {
        Scalar::Bool(false)
    } else {
        Scalar::Text(text.to_owned())
    }
}

/// Cut a trailing ` # comment` from a plain scalar.
fn strip_plain_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    (1..bytes.len())
        .find(|&at| bytes[at] == b'#' && matches!(bytes[at - 1], b' ' | b'\t'))
        .map_or(text, |at| &text[..at])
        .trim_end()
}

/// A plain scalar starting with `rest`; indented lines that follow continue it
/// (folded with spaces, blank lines kept as line breaks).
pub(super) fn plain_scalar(lines: &[Line<'_>], start: usize, rest: &str) -> (Scalar, usize) {
    let mut value = strip_plain_comment(rest).to_owned();
    let mut index = start;
    let mut pending_breaks = 0;
    let mut continued = false;
    while index < lines.len() {
        let line = &lines[index];
        if line.content.is_empty() {
            pending_breaks += 1;
            index += 1;
            continue;
        }
        if line.indent == 0 {
            break;
        }
        index += 1;
        if line.content.starts_with('#') {
            continue;
        }
        if pending_breaks > 0 {
            value.push_str(&"\n".repeat(pending_breaks));
        } else {
            value.push(' ');
        }
        value.push_str(strip_plain_comment(line.content));
        pending_breaks = 0;
        continued = true;
    }
    let scalar = if continued {
        Scalar::Text(value)
    } else {
        interpret_plain(&value)
    };
    (scalar, index)
}

/// Scan a quoted scalar at the start of `text`: the unescaped value and the byte
/// length consumed, or `None` when the closing quote is missing.
fn scan_quoted(text: &str) -> Result<Option<(String, usize)>, &'static str> {
    let Some(mut scan) = QuotedScan::new(text) else {
        return Ok(None);
    };
    Ok(scan.feed(text)?.map(|end| (scan.out, end)))
}

/// A resumable scan of a quoted scalar. A value spread over many lines is fed the
/// growing text after each appended line and continues where it stopped, so every
/// byte is decoded once (linear in the length, however many lines there are).
///
/// Stopping never leaves a character half-decoded: an escape cut off by the end of
/// the text is an error, and a single quote there closes the value, exactly as a
/// scan of the whole text from the start would decide.
struct QuotedScan {
    quote: char,
    out: String,
    /// Byte offset where the next [`QuotedScan::feed`] continues.
    pos: usize,
}

impl QuotedScan {
    /// A scan of the quoted value opened by the first character of `text`.
    fn new(text: &str) -> Option<Self> {
        let quote = text.chars().next()?;
        Some(Self {
            quote,
            out: String::new(),
            pos: quote.len_utf8(),
        })
    }

    /// Continue over `text` (what earlier calls saw, with more appended): the byte
    /// length up to and including the closing quote, or `None` when `text` ends first.
    fn feed(&mut self, text: &str) -> Result<Option<usize>, &'static str> {
        let base = self.pos;
        let quote = self.quote;
        let out = &mut self.out;
        let mut chars = text[base..].char_indices().map(|(at, ch)| (base + at, ch));
        while let Some((at, ch)) = chars.next() {
            if quote == '\'' {
                if ch == '\'' {
                    if text[at + 1..].starts_with('\'') {
                        chars.next();
                        out.push('\'');
                        continue;
                    }
                    return Ok(Some(at + 1));
                }
                out.push(ch);
                continue;
            }
            match ch {
                '"' => return Ok(Some(at + 1)),
                '\\' => decode_escape(&mut chars, out)?,
                _ => out.push(ch),
            }
        }
        self.pos = text.len();
        Ok(None)
    }
}

/// Decode the escape after a `\` in a double-quoted value into `out`.
fn decode_escape(
    chars: &mut impl Iterator<Item = (usize, char)>,
    out: &mut String,
) -> Result<(), &'static str> {
    let (_, escape) = chars.next().ok_or("unterminated escape sequence")?;
    let simple = match escape {
        'n' => Some('\n'),
        't' | '\t' => Some('\t'),
        'r' => Some('\r'),
        '0' => Some('\0'),
        'a' => Some('\u{7}'),
        'b' => Some('\u{8}'),
        'e' => Some('\u{1b}'),
        'f' => Some('\u{c}'),
        'v' => Some('\u{b}'),
        'N' => Some('\u{85}'),
        '_' => Some('\u{a0}'),
        'L' => Some('\u{2028}'),
        'P' => Some('\u{2029}'),
        ' ' | '"' | '\\' | '/' => Some(escape),
        _ => None,
    };
    if let Some(ch) = simple {
        out.push(ch);
        return Ok(());
    }
    let digits = match escape {
        'x' => 2,
        'u' => 4,
        'U' => 8,
        _ => return Err("invalid escape sequence in a double-quoted value"),
    };
    let mut code = 0_u32;
    for _ in 0..digits {
        let (_, digit) = chars.next().ok_or("unterminated escape sequence")?;
        code = code * 16 + digit.to_digit(16).ok_or("invalid hexadecimal escape")?;
    }
    out.push(char::from_u32(code).ok_or("escape is not a Unicode scalar value")?);
    Ok(())
}

/// A quoted scalar starting with `rest`; it may continue on indented lines,
/// which fold to spaces (blank lines become line breaks).
pub(super) fn parse_quoted(
    lines: &[Line<'_>],
    start: usize,
    key_line: usize,
    rest: &str,
) -> Result<(String, usize), FrontMatterError> {
    let mut text = rest.to_owned();
    let mut scan = QuotedScan::new(&text)
        .ok_or_else(|| FrontMatterError::at(key_line, "unterminated quoted value"))?;
    let mut index = start;
    loop {
        // Only the text appended since the last pass is scanned.
        if let Some(end) = scan
            .feed(&text)
            .map_err(|message| FrontMatterError::at(key_line, message))?
        {
            let after = text[end..].trim();
            if !after.is_empty() && !after.starts_with('#') {
                return Err(FrontMatterError::at(
                    key_line,
                    "unexpected text after a quoted value",
                ));
            }
            return Ok((scan.out, index));
        }
        let line = lines
            .get(index)
            .filter(|line| line.content.is_empty() || line.indent > 0)
            .ok_or_else(|| FrontMatterError::at(key_line, "unterminated quoted value"))?;
        index += 1;
        if line.content.is_empty() {
            text.push('\n');
        } else {
            if !text.ends_with('\n') {
                text.push(' ');
            }
            text.push_str(line.content);
        }
    }
}

#[derive(Clone, Copy)]
enum Chomp {
    Clip,
    Strip,
    Keep,
}

/// A `|` (literal) or `>` (folded) block scalar whose header is `header`.
pub(super) fn parse_block_scalar(
    lines: &[Line<'_>],
    start: usize,
    key_line: usize,
    header: &str,
) -> Result<(String, usize), FrontMatterError> {
    let folded = header.starts_with('>');
    let mut chomp = Chomp::Clip;
    let mut explicit_indent = None;
    let mut indicators = header[1..].chars().peekable();
    while let Some(&ch) = indicators.peek() {
        match ch {
            '+' => chomp = Chomp::Keep,
            '-' => chomp = Chomp::Strip,
            '1'..='9' => explicit_indent = ch.to_digit(10).map(|digit| digit as usize),
            _ => break,
        }
        indicators.next();
    }
    let trailing = indicators.collect::<String>();
    let trailing = trailing.trim();
    if !trailing.is_empty() && !trailing.starts_with('#') {
        return Err(FrontMatterError::at(
            key_line,
            "invalid block scalar header",
        ));
    }
    let mut index = start;
    while index < lines.len() && (lines[index].content.is_empty() || lines[index].indent > 0) {
        index += 1;
    }
    let block = &lines[start..index];
    let content_indent = explicit_indent.or_else(|| {
        block
            .iter()
            .find(|line| !line.content.is_empty())
            .map(|line| line.indent)
    });
    let mut texts: Vec<&str> = Vec::with_capacity(block.len());
    if let Some(indent) = content_indent {
        for line in block {
            if line.raw.trim().is_empty() {
                texts.push("");
            } else if line.indent < indent {
                return Err(FrontMatterError::at(
                    line.number,
                    "a block scalar line is less indented than its first line",
                ));
            } else {
                texts.push(&line.raw[indent..]);
            }
        }
    }
    let trailing_blank = texts
        .iter()
        .rev()
        .take_while(|line| line.is_empty())
        .count();
    let body = &texts[..texts.len() - trailing_blank];
    let text = if folded { fold(body) } else { body.join("\n") };
    let value = match chomp {
        Chomp::Strip => text,
        Chomp::Clip if text.is_empty() => text,
        Chomp::Clip => text + "\n",
        Chomp::Keep if text.is_empty() => "\n".repeat(trailing_blank),
        Chomp::Keep => text + &"\n".repeat(1 + trailing_blank),
    };
    Ok((value, index))
}

/// YAML folding: adjacent lines join with a space, blank lines become line
/// breaks, and more-indented lines keep their breaks.
fn fold(lines: &[&str]) -> String {
    #[derive(PartialEq)]
    enum Previous {
        Start,
        Normal,
        Blank,
        MoreIndented,
    }
    let mut out = String::new();
    let mut previous = Previous::Start;
    for line in lines {
        if line.is_empty() {
            out.push('\n');
            previous = Previous::Blank;
            continue;
        }
        let more = line.starts_with([' ', '\t']);
        match previous {
            Previous::Normal if !more => out.push(' '),
            Previous::Normal | Previous::MoreIndented => out.push('\n'),
            Previous::Blank if more => out.push('\n'),
            Previous::Start | Previous::Blank => {}
        }
        out.push_str(line);
        previous = if more {
            Previous::MoreIndented
        } else {
            Previous::Normal
        };
    }
    out
}

/// The value of one `- item` line.
pub(super) fn list_item(text: &str) -> Result<Scalar, &'static str> {
    match text.as_bytes().first() {
        None | Some(b'#') => Ok(Scalar::Null),
        Some(b'"' | b'\'') => {
            let (value, end) = scan_quoted(text)?.ok_or("unterminated quoted value")?;
            let after = text[end..].trim();
            if !after.is_empty() && !after.starts_with('#') {
                return Err("unexpected text after a quoted value");
            }
            Ok(Scalar::Text(value))
        }
        Some(b'[' | b'{') => Ok(Scalar::Unsupported),
        Some(_) if text == "-" || text.starts_with("- ") => Ok(Scalar::Unsupported),
        Some(_) => {
            let plain = strip_plain_comment(text);
            if plain.ends_with(':') || plain.contains(": ") {
                // `- key: value` is a list of mappings.
                return Ok(Scalar::Unsupported);
            }
            Ok(interpret_plain(plain))
        }
    }
}

/// An inline `[a, "b", c]` list on one line. `Ok(None)` when it nests lists or
/// mappings (unsupported, ignored).
pub(super) fn parse_flow_list(rest: &str) -> Result<Option<Vec<String>>, &'static str> {
    let inner = &rest[1..];
    let mut items = Vec::new();
    let mut at = 0;
    let bytes = inner.as_bytes();
    loop {
        while at < bytes.len() && matches!(bytes[at], b' ' | b'\t') {
            at += 1;
        }
        let Some(&byte) = bytes.get(at) else {
            return Err("inline lists must close on the same line");
        };
        match byte {
            b']' => {
                at += 1;
                break;
            }
            b'[' | b'{' => return Ok(None),
            b'"' | b'\'' => {
                let (value, len) = scan_quoted(&inner[at..])?.ok_or("unterminated quoted value")?;
                items.push(value);
                at += len;
            }
            _ => {
                let end = inner[at..]
                    .find([',', ']'])
                    .map_or(inner.len(), |end| at + end);
                match interpret_plain(inner[at..end].trim()) {
                    Scalar::Text(text) => items.push(text),
                    Scalar::Bool(value) => items.push(value.to_string()),
                    Scalar::Null | Scalar::Unsupported => {}
                }
                at = end;
            }
        }
        while at < bytes.len() && matches!(bytes[at], b' ' | b'\t') {
            at += 1;
        }
        match bytes.get(at) {
            Some(b',') => at += 1,
            Some(b']') => {
                at += 1;
                break;
            }
            Some(_) => return Err("expected `,` or `]` in an inline list"),
            None => return Err("inline lists must close on the same line"),
        }
    }
    let after = inner[at..].trim();
    if !after.is_empty() && !after.starts_with('#') {
        return Err("unexpected text after an inline list");
    }
    Ok(Some(items))
}
