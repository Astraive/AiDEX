//! Redaction helpers: credential patterns, known secret values, and size bounds.

use std::fmt;

use zeroize::Zeroizing;

/// Shortest secret value scrubbed (shorter values would mangle ordinary text).
const MIN_SECRET_LEN: usize = 4;
/// Shortest word of a multi-word secret scrubbed on its own.
const MIN_PART_LEN: usize = 8;

/// Cut `text` to at most `max` bytes on a character boundary.
pub(crate) fn truncate_utf8(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// `text` with credential patterns redacted, cut to `max` bytes (marked `...`).
pub(crate) fn bounded_redacted(text: &str, max: usize) -> String {
    let redacted = aidex_events::redact_text(text);
    let (head, cut) = truncate_utf8(&redacted, max);
    if cut {
        format!("{head}...")
    } else {
        head.to_owned()
    }
}

/// Replaces the resolved secret values of one server with `[REDACTED]` in
/// everything that server sends back (protocol frames and stderr).
#[derive(Clone, Default)]
pub struct Scrubber {
    secrets: Vec<Zeroizing<String>>,
}

impl Scrubber {
    /// Scrub each value, and also each long word of a multi-word value (the
    /// credential in `Bearer <token>` or `Basic <encoded>` header values).
    /// Longer values are replaced first so a part never splits a whole value.
    pub fn new(secrets: Vec<Zeroizing<String>>) -> Self {
        let mut all: Vec<Zeroizing<String>> = Vec::new();
        for secret in secrets {
            for part in secret.split_whitespace() {
                if part.len() >= MIN_PART_LEN && part.len() < secret.len() {
                    all.push(Zeroizing::new(part.to_owned()));
                }
            }
            if secret.len() >= MIN_SECRET_LEN {
                all.push(secret);
            }
        }
        all.sort_by(|left, right| {
            right
                .len()
                .cmp(&left.len())
                .then_with(|| left.as_str().cmp(right.as_str()))
        });
        all.dedup_by(|left, right| left.as_str() == right.as_str());
        Self { secrets: all }
    }

    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// `text` without any known secret value.
    pub fn text(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), "[REDACTED]");
            }
        }
        out
    }

    /// `value` with every string (keys included) scrubbed.
    pub fn value(&self, value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value;
        if self.secrets.is_empty() {
            return value;
        }
        match value {
            Value::String(text) => Value::String(self.text(&text)),
            Value::Array(items) => Value::Array(items.into_iter().map(|v| self.value(v)).collect()),
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(key, v)| (self.text(&key), self.value(v)))
                    .collect(),
            ),
            other => other,
        }
    }
}

impl fmt::Debug for Scrubber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Scrubber({} secrets, values redacted)",
            self.secrets.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubber_removes_known_values_everywhere() {
        let scrubber = Scrubber::new(vec![Zeroizing::new("opaque-value-1234".into())]);
        let value = serde_json::json!({"a": ["x opaque-value-1234 y"], "opaque-value-1234": 1});
        let text = scrubber.value(value).to_string();
        assert!(!text.contains("opaque-value-1234"));
        assert!(text.contains("[REDACTED]"));
        assert!(!format!("{scrubber:?}").contains("opaque"));
        let header = Scrubber::new(vec![Zeroizing::new("Bearer token-value-5678".into())]);
        assert_eq!(header.text("got token-value-5678"), "got [REDACTED]");
        assert_eq!(header.text("Bearer token-value-5678"), "[REDACTED]");
        assert_eq!(header.text("Bearer alone"), "Bearer alone");
    }

    #[test]
    fn truncation_respects_character_boundaries() {
        assert_eq!(truncate_utf8("héllo", 2), ("h", true));
        assert_eq!(truncate_utf8("abc", 5), ("abc", false));
        assert_eq!(bounded_redacted("abcdef", 3), "abc...");
    }
}
