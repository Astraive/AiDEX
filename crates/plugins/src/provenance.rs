//! `private_config/plugins.toml`: where each installed plugin came from, its tree
//! hash, the exact hash the user trusted, and whether it is enabled.
//!
//! Closed schema, version 1 (unknown keys are refused):
//!
//! ```toml
//! version = 1
//!
//! [[plugins]]
//! name = "code-review"
//! version = "1.2.0"
//! tree_sha256 = "<64 hex>"            # baseline: install, or the last trusted tree
//! installed_at = "2026-10-11T08:00:00.000Z"
//! source = "D:\\Downloads\\code-review.zip"
//! trusted_sha256 = "<64 hex>"         # optional; equals tree_sha256 when trusted
//! enabled = false
//! ```

use std::{collections::HashSet, io::ErrorKind, path::Path};

use aidex_core::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
    PluginError,
    fsutil::{read_bounded, write_atomic},
    limits::MAX_INSTALLED_PLUGINS,
    names::{is_tree_hash, validate_name},
};

/// File name inside `DrivePaths::config()`.
pub const PROVENANCE_FILE: &str = "plugins.toml";
/// The schema version this build reads and writes.
pub const PROVENANCE_VERSION: u32 = 1;
const MAX_PROVENANCE_BYTES: u64 = 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 4096;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Provenance {
    pub version: u32,
    #[serde(default)]
    pub plugins: Vec<PluginRecord>,
}

impl Default for Provenance {
    fn default() -> Self {
        Self {
            version: PROVENANCE_VERSION,
            plugins: Vec::new(),
        }
    }
}

/// One installed plugin's provenance and trust state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginRecord {
    pub name: String,
    pub version: String,
    /// The tree hash at install (or when a changed tree was last trusted).
    pub tree_sha256: String,
    pub installed_at: Timestamp,
    /// The folder or archive it was installed from.
    pub source: String,
    /// The exact tree hash the user trusted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_sha256: Option<String>,
    pub enabled: bool,
}

impl Provenance {
    pub(crate) fn position(&self, name: &str) -> Option<usize> {
        self.plugins.iter().position(|record| record.name == name)
    }

    fn validate(&self) -> Result<(), String> {
        if self.plugins.len() > MAX_INSTALLED_PLUGINS {
            return Err(format!(
                "more than {MAX_INSTALLED_PLUGINS} plugins are recorded"
            ));
        }
        let mut names = HashSet::new();
        for (index, record) in self.plugins.iter().enumerate() {
            let entry = index + 1;
            validate_name(&record.name).map_err(|_| format!("plugin {entry}: invalid `name`"))?;
            if !names.insert(record.name.as_str()) {
                return Err(format!(
                    "plugin {entry}: `{}` is recorded twice",
                    record.name
                ));
            }
            if record.version.is_empty()
                || record.version.len() > 64
                || record.version.chars().any(char::is_control)
            {
                return Err(format!("plugin {entry}: invalid `version`"));
            }
            if !is_tree_hash(&record.tree_sha256) {
                return Err(format!("plugin {entry}: invalid `tree_sha256`"));
            }
            if record
                .trusted_sha256
                .as_deref()
                .is_some_and(|hash| !is_tree_hash(hash))
            {
                return Err(format!("plugin {entry}: invalid `trusted_sha256`"));
            }
            if record.source.len() > MAX_SOURCE_BYTES || record.source.chars().any(char::is_control)
            {
                return Err(format!("plugin {entry}: invalid `source`"));
            }
        }
        Ok(())
    }
}

/// Read the provenance file (empty when it does not exist).
pub(crate) fn load(path: &Path) -> Result<Provenance, PluginError> {
    let invalid = |message: String| {
        PluginError::Config(format!("`{}` is invalid: {message}", path.display()))
    };
    let bytes = match read_bounded(path, MAX_PROVENANCE_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(Provenance::default()),
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::InvalidInput | ErrorKind::FileTooLarge
            ) =>
        {
            return Err(invalid(error.to_string()));
        }
        Err(error) => return Err(PluginError::io("read", path, &error)),
    };
    let text = String::from_utf8(bytes).map_err(|_| invalid("not UTF-8".into()))?;
    let provenance: Provenance =
        toml::from_str(&text).map_err(|error| invalid(describe_toml_error(&error, &text)))?;
    if provenance.version != PROVENANCE_VERSION {
        return Err(PluginError::Config(format!(
            "`{}` has version {}; this build supports version {PROVENANCE_VERSION}",
            path.display(),
            provenance.version
        )));
    }
    provenance.validate().map_err(invalid)?;
    Ok(provenance)
}

/// Write the provenance file atomically.
pub(crate) fn save(path: &Path, provenance: &Provenance) -> Result<(), PluginError> {
    provenance
        .validate()
        .map_err(|message| PluginError::Config(format!("refusing to write: {message}")))?;
    let text = toml::to_string(provenance)
        .map_err(|error| PluginError::Config(format!("cannot encode plugin records: {error}")))?;
    write_atomic(path, text.as_bytes())
}

/// `toml`'s `Display` quotes the offending line; report the position and the message
/// with quoted literals removed instead.
fn describe_toml_error(error: &toml::de::Error, text: &str) -> String {
    let message = redact_quoted(error.message());
    match error.span() {
        Some(span) => {
            let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
            format!("line {line}: {message}")
        }
        None => message,
    }
}

fn redact_quoted(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut quoted: Option<char> = None;
    for c in message.chars() {
        match quoted {
            Some(open) if c == open => {
                out.push('…');
                out.push(c);
                quoted = None;
            }
            Some(_) => {}
            None if c == '"' || c == '\'' => {
                out.push(c);
                quoted = Some(c);
            }
            None => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(name: &str) -> PluginRecord {
        PluginRecord {
            name: name.into(),
            version: "1.0.0".into(),
            tree_sha256: "a".repeat(64),
            installed_at: Timestamp::new("2026-10-11T08:00:00.000Z").unwrap(),
            source: "C:\\plugins\\x.zip".into(),
            trusted_sha256: None,
            enabled: false,
        }
    }

    #[test]
    fn records_round_trip_and_missing_files_are_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROVENANCE_FILE);
        assert_eq!(load(&path).unwrap(), Provenance::default());
        let mut trusted = record("two");
        trusted.trusted_sha256 = Some("a".repeat(64));
        trusted.enabled = true;
        let provenance = Provenance {
            version: PROVENANCE_VERSION,
            plugins: vec![record("one"), trusted],
        };
        save(&path, &provenance).unwrap();
        assert_eq!(load(&path).unwrap(), provenance);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_files_are_refused_without_quoting_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PROVENANCE_FILE);
        for (text, expect) in [
            ("version = 2\n", "version 2"),
            ("version = 1\nsecret = \"sk-pasted-value\"\n", "line 2"),
            (
                "version = 1\n[[plugins]]\nname = \"Bad\"\nversion = \"1\"\ntree_sha256 = \"x\"\ninstalled_at = \"2026-10-11T08:00:00Z\"\nsource = \"s\"\nenabled = false\n",
                "invalid `name`",
            ),
        ] {
            std::fs::write(&path, text).unwrap();
            let error = load(&path).unwrap_err();
            assert_eq!(error.reason(), "invalid_config");
            let message = error.to_string();
            assert!(message.contains(expect), "{message}");
            assert!(!message.contains("sk-pasted-value"), "{message}");
        }
        let duplicate = Provenance {
            version: PROVENANCE_VERSION,
            plugins: vec![record("one"), record("one")],
        };
        assert!(save(&path, &duplicate).is_err());
    }
}
