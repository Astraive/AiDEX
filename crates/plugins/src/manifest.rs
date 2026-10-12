//! Plugin manifests: `aidex-plugin.json` or the Claude Code `.claude-plugin/plugin.json`.
//!
//! ```json
//! { "name": "code-review", "version": "1.2.0", "description": "...",
//!   "author": {"name": "...", "url": "https://..."}, "homepage": "https://...",
//!   "license": "MIT",
//!   "permissions": {"network": ["api.github.com"], "secrets": ["github-token"], "shell": false} }
//! ```
//!
//! `name`, `version`, and `description` are required. Other keys (Claude Code's
//! `commands`, `agents`, `hooks`, `mcpServers`, `keywords`, ...) are not supported:
//! they are reported as ignored so the review shows them, and nothing they name is
//! ever run. When both manifest files exist, `aidex-plugin.json` is used.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    PluginError,
    fsutil::read_bounded,
    limits::MAX_MANIFEST_BYTES,
    names::{join, validate_name},
};

/// AiDEX's own manifest file, at the plugin root.
pub const AIDEX_MANIFEST: &str = "aidex-plugin.json";
/// The Claude Code plugin manifest file.
pub const CLAUDE_MANIFEST: &str = ".claude-plugin/plugin.json";

const MAX_DESCRIPTION_BYTES: usize = 1024;
const MAX_SHORT_TEXT_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 64;
const MAX_URL_BYTES: usize = 2048;
const MAX_PERMISSION_ENTRIES: usize = 64;
const MAX_IGNORED_FIELDS: usize = 32;
const KNOWN_FIELDS: [&str; 7] = [
    "name",
    "version",
    "description",
    "author",
    "homepage",
    "license",
    "permissions",
];

/// Which manifest file a plugin uses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ManifestFile {
    #[serde(rename = "aidex-plugin.json")]
    Aidex,
    #[serde(rename = ".claude-plugin/plugin.json")]
    ClaudeCode,
}

impl ManifestFile {
    /// The manifest path relative to the plugin root.
    pub const fn path(self) -> &'static str {
        match self {
            Self::Aidex => AIDEX_MANIFEST,
            Self::ClaudeCode => CLAUDE_MANIFEST,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<PluginAuthor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default)]
    pub permissions: PluginPermissions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginAuthor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// What the plugin declares it needs. Declarations are shown for review; MCP servers
/// still pass AiDEX policy for every network request, secret, and process.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginPermissions {
    /// Domains (`example.com` or `*.example.com`).
    #[serde(default)]
    pub network: Vec<String>,
    /// Vault secret names.
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub shell: bool,
}

/// A parsed manifest with the file it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedManifest {
    pub manifest: PluginManifest,
    pub file: ManifestFile,
    /// Unsupported manifest keys (sorted), e.g. `hooks` or `permissions.files`.
    pub ignored_fields: Vec<String>,
}

/// The manifest file present under `root` (`aidex-plugin.json` preferred).
pub(crate) fn find(root: &Path) -> Option<ManifestFile> {
    [ManifestFile::Aidex, ManifestFile::ClaudeCode]
        .into_iter()
        .find(|file| {
            std::fs::symlink_metadata(join(root, file.path()))
                .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        })
}

/// Read and validate the manifest of the plugin at `root`.
pub(crate) fn load(root: &Path) -> Result<LoadedManifest, PluginError> {
    let file = find(root).ok_or_else(|| {
        PluginError::InvalidManifest(format!(
            "no `{AIDEX_MANIFEST}` or `{CLAUDE_MANIFEST}` at the plugin root"
        ))
    })?;
    let path = join(root, file.path());
    let bytes = read_bounded(&path, MAX_MANIFEST_BYTES)
        .map_err(|error| PluginError::InvalidManifest(format!("`{}`: {error}", file.path())))?
        .ok_or_else(|| PluginError::InvalidManifest(format!("`{}` is missing", file.path())))?;
    let (manifest, ignored_fields) =
        parse_manifest(&bytes).map_err(|error| prefix(error, file.path()))?;
    Ok(LoadedManifest {
        manifest,
        file,
        ignored_fields,
    })
}

fn prefix(error: PluginError, file: &str) -> PluginError {
    match error {
        PluginError::InvalidManifest(message) => {
            PluginError::InvalidManifest(format!("`{file}`: {message}"))
        }
        other => other,
    }
}

/// Parse and validate manifest JSON; returns the manifest and its ignored keys.
pub fn parse_manifest(bytes: &[u8]) -> Result<(PluginManifest, Vec<String>), PluginError> {
    let invalid = |message: String| PluginError::InvalidManifest(message);
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid(format!(
            "larger than {} KiB",
            MAX_MANIFEST_BYTES / 1024
        )));
    }
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes).map_err(|error| {
        // Position only: serde_json's message can quote input.
        invalid(format!(
            "not valid JSON (line {}, column {})",
            error.line(),
            error.column()
        ))
    })?;
    let Value::Object(map) = value else {
        return Err(invalid("must be a JSON object".into()));
    };
    let name = required_text(&map, "name", MAX_SHORT_TEXT_BYTES, false)?;
    validate_name(&name).map_err(|_| {
        invalid(
            "`name` must be 1-64 characters of a-z, 0-9, and `-`, starting with a letter or digit"
                .into(),
        )
    })?;
    let version = required_text(&map, "version", MAX_VERSION_BYTES, false)?;
    validate_version(&version)?;
    let description = required_text(&map, "description", MAX_DESCRIPTION_BYTES, true)?;
    let author = author(map.get("author"))?;
    let homepage = optional_text(&map, "homepage", MAX_URL_BYTES, false)?;
    if let Some(url) = &homepage {
        validate_url("homepage", url)?;
    }
    let license = optional_text(&map, "license", MAX_SHORT_TEXT_BYTES, false)?;
    let mut ignored: Vec<String> = map
        .keys()
        .filter(|key| !KNOWN_FIELDS.contains(&key.as_str()))
        .filter_map(|key| displayable_key(key))
        .collect();
    let permissions = permissions(map.get("permissions"), &mut ignored)?;
    ignored.sort();
    ignored.dedup();
    ignored.truncate(MAX_IGNORED_FIELDS);
    Ok((
        PluginManifest {
            name,
            version,
            description,
            author,
            homepage,
            license,
            permissions,
        },
        ignored,
    ))
}

fn required_text(
    map: &Map<String, Value>,
    key: &str,
    max: usize,
    multiline: bool,
) -> Result<String, PluginError> {
    optional_text(map, key, max, multiline)?
        .ok_or_else(|| PluginError::InvalidManifest(format!("`{key}` is required")))
}

/// A trimmed, non-empty string field; `null` and absence are `None`.
fn optional_text(
    map: &Map<String, Value>,
    key: &str,
    max: usize,
    multiline: bool,
) -> Result<Option<String>, PluginError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => text(key, value, max, multiline).map(Some),
    }
}

fn text(key: &str, value: &Value, max: usize, multiline: bool) -> Result<String, PluginError> {
    let invalid = |message: &str| PluginError::InvalidManifest(format!("`{key}` {message}"));
    let Value::String(text) = value else {
        return Err(invalid("must be a string"));
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(invalid("must not be empty"));
    }
    if text.len() > max {
        return Err(invalid(&format!("must be at most {max} bytes")));
    }
    if text
        .chars()
        .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\t')))
    {
        return Err(invalid("must not contain control characters"));
    }
    Ok(text.to_owned())
}

/// `[0-9A-Za-z.+-]`, starting with a digit, at most 64 bytes (SemVer fits).
fn validate_version(version: &str) -> Result<(), PluginError> {
    let valid = version.starts_with(|c: char| c.is_ascii_digit())
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'));
    if valid {
        Ok(())
    } else {
        Err(PluginError::InvalidManifest(
            "`version` must start with a digit and use only 0-9, A-Z, a-z, `.`, `+`, and `-`"
                .into(),
        ))
    }
}

/// An `http(s)` URL without whitespace or user information.
fn validate_url(key: &str, url: &str) -> Result<(), PluginError> {
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"));
    let valid = rest.is_some_and(|rest| {
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !authority.is_empty() && !authority.contains('@')
    }) && !url.chars().any(char::is_whitespace);
    if valid {
        Ok(())
    } else {
        Err(PluginError::InvalidManifest(format!(
            "`{key}` must be an http(s) URL without spaces or user information"
        )))
    }
}

/// `{"name", "url"?}` (other keys such as `email` are not kept), or a plain name.
fn author(value: Option<&Value>) -> Result<Option<PluginAuthor>, PluginError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(name @ Value::String(_)) => Ok(Some(PluginAuthor {
            name: text("author", name, MAX_SHORT_TEXT_BYTES, false)?,
            url: None,
        })),
        Some(Value::Object(author)) => {
            let name = author
                .get("name")
                .ok_or_else(|| PluginError::InvalidManifest("`author.name` is required".into()))?;
            let name = text("author.name", name, MAX_SHORT_TEXT_BYTES, false)?;
            let url = match author.get("url") {
                None | Some(Value::Null) => None,
                Some(url) => {
                    let url = text("author.url", url, MAX_URL_BYTES, false)?;
                    validate_url("author.url", &url)?;
                    Some(url)
                }
            };
            Ok(Some(PluginAuthor { name, url }))
        }
        Some(_) => Err(PluginError::InvalidManifest(
            "`author` must be an object or a string".into(),
        )),
    }
}

fn permissions(
    value: Option<&Value>,
    ignored: &mut Vec<String>,
) -> Result<PluginPermissions, PluginError> {
    let map = match value {
        None | Some(Value::Null) => return Ok(PluginPermissions::default()),
        Some(Value::Object(map)) => map,
        Some(_) => {
            return Err(PluginError::InvalidManifest(
                "`permissions` must be an object".into(),
            ));
        }
    };
    ignored.extend(
        map.keys()
            .filter(|key| !matches!(key.as_str(), "network" | "secrets" | "shell"))
            .filter_map(|key| displayable_key(key))
            .map(|key| format!("permissions.{key}")),
    );
    let network = list(map.get("network"), "permissions.network", |domain| {
        let domain = domain.to_ascii_lowercase();
        valid_domain(&domain).then_some(domain)
    })?;
    let secrets = list(map.get("secrets"), "permissions.secrets", |name| {
        valid_secret_name(name).then(|| name.to_owned())
    })?;
    let shell = match map.get("shell") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(shell)) => *shell,
        Some(_) => {
            return Err(PluginError::InvalidManifest(
                "`permissions.shell` must be true or false".into(),
            ));
        }
    };
    Ok(PluginPermissions {
        network,
        secrets,
        shell,
    })
}

/// A list of strings, each normalized by `accept` (`None` refuses it); duplicates dropped.
fn list(
    value: Option<&Value>,
    key: &str,
    accept: impl Fn(&str) -> Option<String>,
) -> Result<Vec<String>, PluginError> {
    let invalid = |message: String| PluginError::InvalidManifest(format!("`{key}` {message}"));
    let items = match value {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(invalid("must be a list of strings".into())),
    };
    if items.len() > MAX_PERMISSION_ENTRIES {
        return Err(invalid(format!(
            "may list at most {MAX_PERMISSION_ENTRIES} entries"
        )));
    }
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let accepted = item
            .as_str()
            .and_then(|item| accept(item.trim()))
            .ok_or_else(|| invalid(format!("entry {} is not valid", index + 1)))?;
        if !out.contains(&accepted) {
            out.push(accepted);
        }
    }
    Ok(out)
}

/// `host` or `*.host`: lowercase DNS labels of `a-z`, `0-9`, and inner `-`.
fn valid_domain(domain: &str) -> bool {
    let host = domain.strip_prefix("*.").unwrap_or(domain);
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// A vault secret name: 1-128 of `A-Z a-z 0-9 . _ -`, starting with a letter or digit.
fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A manifest key safe to echo (`[A-Za-z0-9_.$-]`, at most 64 bytes).
fn displayable_key(key: &str) -> Option<String> {
    (!key.is_empty()
        && key.len() <= 64
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '$')))
    .then(|| key.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<(PluginManifest, Vec<String>), PluginError> {
        parse_manifest(value.to_string().as_bytes())
    }

    #[test]
    fn full_manifests_parse_and_unknown_keys_are_reported() {
        let (manifest, ignored) = parse(json!({
            "name": "code-review",
            "version": "1.2.0-rc.1",
            "description": "Reviews code.\nTwo lines.",
            "author": {"name": "Ada", "email": "ada@example.com", "url": "https://example.com"},
            "homepage": "https://example.com/plugin",
            "license": "MIT",
            "permissions": {"network": ["API.GitHub.com", "*.example.com", "api.github.com"], "secrets": ["github-token"], "shell": true, "files": "write"},
            "hooks": {"PreToolUse": []},
            "commands": ["./x"],
            "key with spaces": 1
        }))
        .unwrap();
        assert_eq!(manifest.name, "code-review");
        assert_eq!(
            manifest.author,
            Some(PluginAuthor {
                name: "Ada".into(),
                url: Some("https://example.com".into())
            })
        );
        assert_eq!(
            manifest.permissions,
            PluginPermissions {
                network: vec!["api.github.com".into(), "*.example.com".into()],
                secrets: vec!["github-token".into()],
                shell: true,
            }
        );
        assert_eq!(ignored, ["commands", "hooks", "permissions.files"]);

        let (minimal, ignored) =
            parse(json!({"name": "x", "version": "1", "description": "d", "author": "Bob"}))
                .unwrap();
        assert_eq!(minimal.author.unwrap().name, "Bob");
        assert_eq!(minimal.permissions, PluginPermissions::default());
        assert!(ignored.is_empty());
    }

    #[test]
    fn invalid_manifests_are_refused_without_quoting_values() {
        let base = json!({"name": "ok", "version": "1.0.0", "description": "d"});
        let with = |key: &str, value: Value| {
            let mut manifest = base.clone();
            manifest[key] = value;
            manifest
        };
        let mut missing = base.clone();
        missing.as_object_mut().unwrap().remove("version");
        for bad in [
            json!([]),
            missing,
            with("name", json!("Bad_Name")),
            with("name", json!("../escape")),
            with("version", json!("v1")),
            with("version", json!("1.0 beta")),
            with("description", json!("")),
            with("description", json!("x".repeat(1025))),
            with("description", json!("bell\u{7}")),
            with("homepage", json!("javascript:alert(1)")),
            with("homepage", json!("https://user:sk-secret@example.com")),
            with("author", json!(3)),
            with("author", json!({"url": "https://example.com"})),
            with("permissions", json!({"network": ["exa mple.com"]})),
            with("permissions", json!({"network": "example.com"})),
            with("permissions", json!({"secrets": ["../vault"]})),
            with("permissions", json!({"shell": "yes"})),
            with("permissions", json!({"network": vec!["a.com"; 65]})),
        ] {
            let error = parse(bad.clone()).unwrap_err();
            assert_eq!(error.reason(), "invalid_manifest", "{bad}");
            assert!(!error.to_string().contains("sk-secret"), "{error}");
        }
        let error = parse_manifest(b"{\"name\": \"sk-secret").unwrap_err();
        assert!(error.to_string().contains("line 1"), "{error}");
        assert!(!error.to_string().contains("sk-secret"), "{error}");
        assert!(parse_manifest(&vec![b' '; 64 * 1024 + 1]).is_err());
    }
}
