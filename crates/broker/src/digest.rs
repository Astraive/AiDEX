//! Domain-separated SHA-256 digests over canonical JSON (sorted keys, no
//! whitespace), independent of `serde_json`'s map ordering features.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Lowercase hex SHA-256 of `domain`, a NUL separator, and `value` as canonical JSON.
pub(crate) fn digest(domain: &str, value: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    canonical(value, &mut hasher);
    hex(&hasher.finalize())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    text
}

fn canonical(value: &Value, hasher: &mut Sha256) {
    match value {
        Value::Null => hasher.update(b"null"),
        Value::Bool(true) => hasher.update(b"true"),
        Value::Bool(false) => hasher.update(b"false"),
        Value::Number(number) => hasher.update(number.to_string().as_bytes()),
        Value::String(text) => string(text, hasher),
        Value::Array(items) => {
            hasher.update(b"[");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    hasher.update(b",");
                }
                canonical(item, hasher);
            }
            hasher.update(b"]");
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            hasher.update(b"{");
            for (index, key) in keys.into_iter().enumerate() {
                if index > 0 {
                    hasher.update(b",");
                }
                string(key, hasher);
                hasher.update(b":");
                canonical(&map[key], hasher);
            }
            hasher.update(b"}");
        }
    }
}

fn string(text: &str, hasher: &mut Sha256) {
    // serde_json escapes deterministically; serializing a &str cannot fail.
    let encoded = serde_json::to_string(text).unwrap_or_default();
    hasher.update(encoded.as_bytes());
}

/// Standard base64 with padding (RFC 4648), for HTTP Basic credentials.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for (position, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if position <= chunk.len() {
                text.push(char::from(ALPHABET[((triple >> shift) & 0x3f) as usize]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digests_ignore_key_order_but_not_values() {
        let a = json!({"b": 1, "a": [1, {"y": true, "x": null}]});
        let b = json!({"a": [1, {"x": null, "y": true}], "b": 1});
        assert_eq!(digest("d", &a), digest("d", &b));
        assert_ne!(digest("d", &a), digest("e", &a));
        assert_ne!(digest("d", &a), digest("d", &json!({"b": 2})));
    }

    #[test]
    fn base64_matches_rfc4648_vectors() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64(input.as_bytes()), expected);
        }
    }
}
