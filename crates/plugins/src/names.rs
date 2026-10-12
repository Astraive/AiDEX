//! Plugin names, plugin-relative paths, and tree hashes.
//!
//! Relative paths use `/` and must extract to the same names on NTFS, exFAT, FAT32,
//! ext4, and APFS: no traversal, no absolute or drive-qualified paths, no characters
//! Windows forbids, no device names, and no trailing dots or spaces (which Windows
//! silently strips, so two archive names could land on one file).

use std::path::{Path, PathBuf};

/// Longest plugin name.
pub const MAX_NAME_LEN: usize = 64;
/// Longest plugin-relative path, in bytes.
pub const MAX_PATH_LEN: usize = 200;
/// Deepest plugin-relative path, in segments.
pub const MAX_DEPTH: usize = 16;
const MAX_SEGMENT_LEN: usize = 128;
const RESERVED_DEVICE_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A plugin name: `[a-z0-9][a-z0-9-]{0,63}` (also its directory under `plugins/`).
pub fn validate_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME_LEN
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "plugin name {} must be 1-{MAX_NAME_LEN} characters of a-z, 0-9, and `-`, starting with a letter or digit",
            quote(name)
        ))
    }
}

/// A SHA-256 tree hash: 64 lowercase hex digits.
pub fn is_tree_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Validate a `/`-separated path relative to a plugin root.
pub(crate) fn validate_relative(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("the path is empty".into());
    }
    if path.len() > MAX_PATH_LEN {
        return Err(format!("the path is longer than {MAX_PATH_LEN} bytes"));
    }
    let segments: Vec<&str> = path.split('/').collect();
    if segments.len() > MAX_DEPTH {
        return Err(format!("the path is nested deeper than {MAX_DEPTH} levels"));
    }
    segments.into_iter().try_for_each(validate_segment)
}

/// Validate one file or directory name.
pub(crate) fn validate_segment(segment: &str) -> Result<(), String> {
    if segment.is_empty() {
        return Err("the path has an empty segment (absolute path or `//`)".into());
    }
    if segment == "." || segment == ".." {
        return Err("relative traversal segments are not allowed".into());
    }
    if segment.len() > MAX_SEGMENT_LEN {
        return Err(format!("a name is longer than {MAX_SEGMENT_LEN} bytes"));
    }
    if let Some(bad) = segment
        .chars()
        .find(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*'))
    {
        return Err(format!("character {bad:?} is not allowed in file names"));
    }
    if segment.starts_with(' ') || segment.ends_with(' ') || segment.ends_with('.') {
        return Err("names may not start or end with a space or end with a dot".into());
    }
    let stem = segment.split('.').next().unwrap_or(segment).trim_end();
    if RESERVED_DEVICE_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
    {
        return Err("the name is a reserved Windows device name".into());
    }
    Ok(())
}

/// Case-folded form used to detect names that collide on case-insensitive filesystems.
pub(crate) fn fold(path: &str) -> String {
    path.to_lowercase()
}

/// `base` joined with a validated `/`-separated relative path.
pub(crate) fn join(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(base.to_path_buf(), |path, segment| path.join(segment))
}

/// Untrusted text for an error message: escaped and bounded.
pub(crate) fn quote(text: &str) -> String {
    let mut shown: String = text.chars().take(80).collect();
    if shown.len() < text.len() {
        shown.push('…');
    }
    format!("{shown:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_lowercase_slugs() {
        for good in ["a", "code-review", "0day", &"a".repeat(64)] {
            validate_name(good).unwrap();
        }
        for bad in [
            "",
            "-a",
            "A",
            "a_b",
            "a.b",
            "a b",
            "a/b",
            "..",
            &"a".repeat(65),
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn relative_paths_refuse_traversal_and_unportable_names() {
        for good in [
            "skills/review/SKILL.md",
            ".claude-plugin/plugin.json",
            ".mcp.json",
            "docs/notes (draft).md",
            "docs/caf\u{e9}.md",
        ] {
            validate_relative(good).unwrap();
        }
        for bad in [
            "",
            "/etc/passwd",
            "a//b",
            "a/../b",
            "../b",
            "./a",
            "a\\b",
            "C:/x",
            "a/b.",
            "a/b ",
            "a/NUL.txt",
            "a/con",
            "a/b\u{7}",
            "a/b*c",
        ] {
            assert!(validate_relative(bad).is_err(), "{bad:?}");
        }
        assert!(validate_relative(&["a"; 17].join("/")).is_err());
        assert!(validate_relative(&"a".repeat(201)).is_err());
    }

    #[test]
    fn tree_hashes_are_lowercase_hex() {
        assert!(is_tree_hash(&"0a".repeat(32)));
        assert!(!is_tree_hash(&"0A".repeat(32)));
        assert!(!is_tree_hash("abc"));
    }
}
