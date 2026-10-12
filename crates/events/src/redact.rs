//! Credential redaction and length bounds for free text.
//!
//! [`redact_text`] replaces recognisable credentials with [`REDACTED`]; it is applied
//! to every text field of an event before it is stored or delivered, and is meant to
//! be reused for logs. Detection is pattern based (no dependencies, linear time) and
//! deliberately errs toward redacting: a false positive hides a harmless value, a
//! false negative leaks a secret. It is a last line of defence; producers must still
//! keep secret values out of events.

use std::borrow::Cow;

/// Replacement for every detected credential.
pub const REDACTED: &str = "[REDACTED]";
/// Longest text field kept in an event, in bytes (after redaction).
pub const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Start of the marker appended to truncated text: `[TRUNCATED: N bytes omitted]`.
pub const TRUNCATION_MARKER: &str = "[TRUNCATED: ";

/// Room reserved for the truncation marker.
const MARKER_RESERVE: usize = 48;
/// Longest key name considered in `name=value` assignments.
const MAX_NAME: usize = 64;

/// Replace credentials in `text` with [`REDACTED`]; borrowed when nothing matched.
///
/// Recognised: OpenAI/Anthropic `sk-…` keys, Stripe `sk_live_…`-style keys, GitHub
/// `ghp_/gho_/ghu_/ghs_/ghr_` tokens and `github_pat_…`, GitLab `glpat-…`, AWS
/// `AKIA…`/`ASIA…` key ids, Slack `xox[abprs]-…`, Google `AIza…`, Hugging Face `hf_…`,
/// JWTs, `Bearer …` credentials, `Authorization: <scheme> …` headers, PEM private-key
/// blocks, passwords in URLs (`scheme://user:pass@host`), and values assigned to names
/// containing `password`/`passwd`/`pwd`/`passphrase` (any value of 3+ bytes) or
/// `key`/`token`/`secret`/`credential`/`auth` (long high-entropy values), in
/// `name=value`, `name: value`, JSON `"name": "value"`, and `--name value` forms.
pub fn redact_text(text: &str) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    let mut spans = Vec::new();
    prefixed_tokens(bytes, &mut spans);
    bearer_credentials(bytes, &mut spans);
    authorization_headers(bytes, &mut spans);
    assignments(bytes, &mut spans);
    flag_values(bytes, &mut spans);
    pem_private_keys(bytes, &mut spans);
    url_passwords(bytes, &mut spans);
    if spans.is_empty() {
        return Cow::Borrowed(text);
    }
    spans.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (start, end) in merged {
        // Spans start and end on ASCII bytes or the end of text, so these are char
        // boundaries; skip defensively rather than panic if that ever changes.
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }
        out.push_str(&text[copied..start]);
        out.push_str(REDACTED);
        copied = end;
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// Truncate `text` to at most `max` bytes (on a char boundary), appending an explicit
/// `[TRUNCATED: N bytes omitted]` marker; borrowed when it already fits.
pub fn truncate_text(text: &str, max: usize) -> Cow<'_, str> {
    if text.len() <= max {
        return Cow::Borrowed(text);
    }
    let mut cut = max.saturating_sub(MARKER_RESERVE);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    Cow::Owned(format!(
        "{}{TRUNCATION_MARKER}{} bytes omitted]",
        &text[..cut],
        text.len() - cut
    ))
}

/// Redact, then bound to [`MAX_TEXT_BYTES`]: what every event text field goes through.
pub fn sanitize_text(text: &str) -> Cow<'_, str> {
    match redact_text(text) {
        Cow::Borrowed(text) => truncate_text(text, MAX_TEXT_BYTES),
        Cow::Owned(redacted) => match truncate_text(&redacted, MAX_TEXT_BYTES) {
            Cow::Borrowed(_) => Cow::Owned(redacted),
            Cow::Owned(truncated) => Cow::Owned(truncated),
        },
    }
}

fn alnum(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

fn alnum_underscore(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn alnum_dash(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

fn token_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

fn upper_digit(byte: u8) -> bool {
    byte.is_ascii_uppercase() || byte.is_ascii_digit()
}

/// RFC 6750 `b64token` characters.
fn b64token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~+/=".contains(&byte)
}

fn run(bytes: &[u8], from: usize, class: impl Fn(u8) -> bool) -> usize {
    let mut end = from;
    while end < bytes.len() && class(bytes[end]) {
        end += 1;
    }
    end
}

/// Position `at` does not continue an alphanumeric word.
fn word_start(bytes: &[u8], at: usize) -> bool {
    at == 0 || !bytes[at - 1].is_ascii_alphanumeric()
}

fn starts_with_ignore_case(bytes: &[u8], at: usize, pattern: &[u8]) -> bool {
    bytes
        .get(at..at + pattern.len())
        .is_some_and(|window| window.eq_ignore_ascii_case(pattern))
}

fn find(bytes: &[u8], from: usize, pattern: &[u8]) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    bytes[from..]
        .windows(pattern.len())
        .position(|window| window == pattern)
        .map(|offset| from + offset)
}

struct Prefix {
    prefix: &'static [u8],
    class: fn(u8) -> bool,
    min: usize,
}

const PREFIXES: &[Prefix] = &[
    Prefix {
        prefix: b"sk-",
        class: token_char,
        min: 20,
    },
    Prefix {
        prefix: b"sk_live_",
        class: alnum,
        min: 16,
    },
    Prefix {
        prefix: b"sk_test_",
        class: alnum,
        min: 16,
    },
    Prefix {
        prefix: b"rk_live_",
        class: alnum,
        min: 16,
    },
    Prefix {
        prefix: b"rk_test_",
        class: alnum,
        min: 16,
    },
    Prefix {
        prefix: b"ghp_",
        class: alnum,
        min: 20,
    },
    Prefix {
        prefix: b"gho_",
        class: alnum,
        min: 20,
    },
    Prefix {
        prefix: b"ghu_",
        class: alnum,
        min: 20,
    },
    Prefix {
        prefix: b"ghs_",
        class: alnum,
        min: 20,
    },
    Prefix {
        prefix: b"ghr_",
        class: alnum,
        min: 20,
    },
    Prefix {
        prefix: b"github_pat_",
        class: alnum_underscore,
        min: 20,
    },
    Prefix {
        prefix: b"glpat-",
        class: token_char,
        min: 20,
    },
    Prefix {
        prefix: b"xoxa-",
        class: alnum_dash,
        min: 10,
    },
    Prefix {
        prefix: b"xoxb-",
        class: alnum_dash,
        min: 10,
    },
    Prefix {
        prefix: b"xoxp-",
        class: alnum_dash,
        min: 10,
    },
    Prefix {
        prefix: b"xoxr-",
        class: alnum_dash,
        min: 10,
    },
    Prefix {
        prefix: b"xoxs-",
        class: alnum_dash,
        min: 10,
    },
    Prefix {
        prefix: b"AKIA",
        class: upper_digit,
        min: 16,
    },
    Prefix {
        prefix: b"ASIA",
        class: upper_digit,
        min: 16,
    },
    Prefix {
        prefix: b"AIza",
        class: token_char,
        min: 30,
    },
    Prefix {
        prefix: b"hf_",
        class: alnum,
        min: 30,
    },
];

/// Provider tokens recognisable by their prefix, and JWTs.
fn prefixed_tokens(bytes: &[u8], spans: &mut Vec<(usize, usize)>) {
    let mut at = 0;
    while at < bytes.len() {
        if word_start(bytes, at)
            && let Some(end) = prefixed_token_at(bytes, at).or_else(|| jwt_at(bytes, at))
        {
            spans.push((at, end));
            at = end;
            continue;
        }
        at += 1;
    }
}

fn prefixed_token_at(bytes: &[u8], at: usize) -> Option<usize> {
    PREFIXES.iter().find_map(|rule| {
        if !bytes[at..].starts_with(rule.prefix) {
            return None;
        }
        let body = at + rule.prefix.len();
        let end = run(bytes, body, rule.class);
        (end - body >= rule.min).then_some(end)
    })
}

/// `eyJ<header>.<payload>.<signature>` (base64url segments).
fn jwt_at(bytes: &[u8], at: usize) -> Option<usize> {
    if !bytes[at..].starts_with(b"eyJ") {
        return None;
    }
    let header = run(bytes, at, token_char);
    if header - at < 10 || bytes.get(header) != Some(&b'.') {
        return None;
    }
    let payload = run(bytes, header + 1, token_char);
    if payload - header - 1 < 4 || bytes.get(payload) != Some(&b'.') {
        return None;
    }
    Some(run(bytes, payload + 1, token_char))
}

/// `Bearer <credential>`, any case.
fn bearer_credentials(bytes: &[u8], spans: &mut Vec<(usize, usize)>) {
    let mut at = 0;
    while at + 6 < bytes.len() {
        if word_start(bytes, at) && starts_with_ignore_case(bytes, at, b"bearer") {
            let gap = run(bytes, at + 6, |byte| byte == b' ' || byte == b'\t');
            if gap > at + 6 {
                let end = run(bytes, gap, b64token);
                if end - gap >= 8 {
                    spans.push((gap, end));
                    at = end;
                    continue;
                }
            }
        }
        at += 1;
    }
}

/// `Authorization: <scheme> <credential>` (Basic, Token, Digest, ...).
fn authorization_headers(bytes: &[u8], spans: &mut Vec<(usize, usize)>) {
    const NAME: &[u8] = b"authorization";
    let mut at = 0;
    while at + NAME.len() < bytes.len() {
        if !(word_start(bytes, at) && starts_with_ignore_case(bytes, at, NAME)) {
            at += 1;
            continue;
        }
        let mut cursor = at + NAME.len();
        cursor = run(bytes, cursor, |byte| {
            matches!(byte, b'"' | b'\'' | b' ' | b'\t')
        });
        if !matches!(bytes.get(cursor), Some(b':' | b'=')) {
            at += 1;
            continue;
        }
        cursor = run(bytes, cursor + 1, |byte| {
            matches!(byte, b'"' | b'\'' | b' ' | b'\t')
        });
        let scheme_end = run(bytes, cursor, |byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        });
        let gap = run(bytes, scheme_end, |byte| matches!(byte, b' ' | b'\t'));
        if scheme_end > cursor && gap > scheme_end {
            let end = run(bytes, gap, |byte| {
                !byte.is_ascii_whitespace() && !matches!(byte, b'"' | b'\'' | b',' | b';')
            });
            if end - gap >= 6 {
                spans.push((gap, end));
                at = end;
                continue;
            }
        }
        at += 1;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sensitivity {
    /// Any non-trivial value is a secret.
    Password,
    /// Long, high-entropy values are secrets.
    Credential,
}

fn classify(name: &[u8]) -> Option<Sensitivity> {
    let lower = name.to_ascii_lowercase();
    let contains = |needle: &[u8]| lower.windows(needle.len()).any(|window| window == needle);
    if [&b"passw"[..], b"pwd", b"passphrase"]
        .iter()
        .any(|needle| contains(needle))
    {
        Some(Sensitivity::Password)
    } else if [&b"secret"[..], b"token", b"key", b"credential", b"auth"]
        .iter()
        .any(|needle| contains(needle))
    {
        Some(Sensitivity::Credential)
    } else {
        None
    }
}

/// At least 16 bytes mixing two of lowercase, uppercase, and digits.
fn high_entropy(value: &[u8]) -> bool {
    if value.len() < 16 {
        return false;
    }
    let classes = [
        value.iter().any(u8::is_ascii_lowercase),
        value.iter().any(u8::is_ascii_uppercase),
        value.iter().any(u8::is_ascii_digit),
    ];
    classes.iter().filter(|present| **present).count() >= 2
}

fn should_redact(sensitivity: Sensitivity, value: &[u8]) -> bool {
    // Already redacted (keeps redaction idempotent).
    if value.starts_with(b"[REDACTED") {
        return false;
    }
    match sensitivity {
        Sensitivity::Password => value.len() >= 3,
        Sensitivity::Credential => high_entropy(value),
    }
}

fn name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

fn value_terminator(byte: u8) -> bool {
    byte.is_ascii_whitespace()
        || matches!(
            byte,
            b'"' | b'\'' | b',' | b';' | b'&' | b'}' | b']' | b')' | b'<' | b'>'
        )
}

/// The value after a separator: optional spaces, then a quoted string or a bare word.
fn value_span(bytes: &[u8], from: usize) -> (usize, usize) {
    let start = run(bytes, from, |byte| byte == b' ' || byte == b'\t');
    match bytes.get(start) {
        Some(&quote @ (b'"' | b'\'')) => {
            let begin = start + 1;
            let end = run(bytes, begin, |byte| byte != quote && byte != b'\n');
            (begin, end)
        }
        _ => (start, run(bytes, start, |byte| !value_terminator(byte))),
    }
}

/// `name=value`, `name: value`, and `"name": "value"` where the name is sensitive.
fn assignments(bytes: &[u8], spans: &mut Vec<(usize, usize)>) {
    for separator in 0..bytes.len() {
        let byte = bytes[separator];
        if byte != b'=' && byte != b':' {
            continue;
        }
        // Skip `::` paths, `==`/`=>` operators, and `scheme://` URLs.
        let next = bytes.get(separator + 1).copied();
        let previous = separator.checked_sub(1).map(|index| bytes[index]);
        if matches!(next, Some(b':' | b'=' | b'>' | b'/'))
            || matches!(previous, Some(b':' | b'=' | b'!' | b'<' | b'>'))
        {
            continue;
        }
        let mut name_end = separator;
        if matches!(previous, Some(b'"' | b'\'')) {
            name_end -= 1;
        }
        let mut name_start = name_end;
        while name_start > 0 && name_end - name_start < MAX_NAME && name_char(bytes[name_start - 1])
        {
            name_start -= 1;
        }
        // `scheme://user:password@` userinfo is handled by `url_passwords`.
        if name_start == name_end || bytes[..name_start].ends_with(b"://") {
            continue;
        }
        let Some(sensitivity) = classify(&bytes[name_start..name_end]) else {
            continue;
        };
        let (start, end) = value_span(bytes, separator + 1);
        if should_redact(sensitivity, &bytes[start..end]) {
            spans.push((start, end));
        }
    }
}

/// `--password value` / `-p`-less long flags followed by a space-separated value.
fn flag_values(bytes: &[u8], spans: &mut Vec<(usize, usize)>) {
    let mut at = 0;
    while at + 2 < bytes.len() {
        let boundary = at == 0 || bytes[at - 1].is_ascii_whitespace();
        if !(boundary && bytes[at] == b'-' && bytes[at + 1] == b'-') {
            at += 1;
            continue;
        }
        let name_end = run(bytes, at + 2, |byte| {
            byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
        });
        let sensitivity = classify(&bytes[at + 2..name_end]);
        if let (Some(sensitivity), Some(b' ')) = (sensitivity, bytes.get(name_end)) {
            let start = name_end + 1;
            let end = run(bytes, start, |byte| !byte.is_ascii_whitespace());
            if bytes.get(start) != Some(&b'-') && should_redact(sensitivity, &bytes[start..end]) {
                spans.push((start, end));
            }
        }
        at = name_end.max(at + 1);
    }
}

/// `-----BEGIN … PRIVATE KEY-----` through the matching END line (or end of text).
fn pem_private_keys(bytes: &[u8], spans: &mut Vec<(usize, usize)>) {
    const BEGIN: &[u8] = b"-----BEGIN ";
    let mut from = 0;
    while let Some(start) = find(bytes, from, BEGIN) {
        let label_start = start + BEGIN.len();
        let label_end = run(bytes, label_start, |byte| {
            byte.is_ascii_uppercase() || byte == b' ' || byte.is_ascii_digit()
        });
        let label = &bytes[label_start..label_end];
        let is_private = label.windows(11).any(|window| window == b"PRIVATE KEY");
        if !is_private || !bytes[label_end..].starts_with(b"-----") {
            from = label_start;
            continue;
        }
        let end = find(bytes, label_end + 5, b"-----END ")
            .map(|end_line| {
                let label_end = run(bytes, end_line + 9, |byte| {
                    byte.is_ascii_uppercase() || byte == b' ' || byte.is_ascii_digit()
                });
                if bytes[label_end..].starts_with(b"-----") {
                    label_end + 5
                } else {
                    label_end
                }
            })
            .unwrap_or(bytes.len());
        spans.push((start, end));
        from = end;
    }
}

/// `scheme://user:password@host` → the password.
fn url_passwords(bytes: &[u8], spans: &mut Vec<(usize, usize)>) {
    let mut from = 0;
    while let Some(scheme) = find(bytes, from, b"://") {
        let authority = scheme + 3;
        let end = run(bytes, authority, |byte| {
            !byte.is_ascii_whitespace() && !matches!(byte, b'/' | b'?' | b'#' | b'"' | b'\'')
        });
        if let Some(at) = bytes[authority..end].iter().rposition(|byte| *byte == b'@') {
            let userinfo = &bytes[authority..authority + at];
            if let Some(colon) = userinfo.iter().position(|byte| *byte == b':') {
                let start = authority + colon + 1;
                let stop = authority + at;
                if stop > start {
                    spans.push((start, stop));
                }
            }
        }
        from = end.max(authority);
    }
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
