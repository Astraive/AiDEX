//! `PluginError`: every refusal and failure, with a stable reason and a service code.
//!
//! Messages name plugin-relative paths, manifest fields, and limits; they never quote
//! manifest values or file contents.

use std::{fmt, io, path::Path};

use aidex_core::ServiceErrorCode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PluginError {
    /// A request parameter is invalid (name, hash, or source path).
    InvalidRequest(String),
    /// No plugin with that name is installed.
    NotFound(String),
    /// A plugin with that name is already installed (pass `replace`).
    Exists(String),
    /// The manifest is missing or violates the manifest schema.
    InvalidManifest(String),
    /// The plugin tree holds something that is never installed: a link, a special
    /// file, an unportable or traversing name, or an invalid `.mcp.json`.
    UnsafeContent {
        path: String,
        reason: String,
    },
    /// The plugin ships an executable that no `.mcp.json` server declares.
    Executable {
        path: String,
    },
    /// A size, file-count, or archive limit was exceeded.
    TooLarge(String),
    /// The tree hash to trust is not the plugin's current tree hash.
    HashMismatch {
        name: String,
    },
    /// Enabling needs the user to trust the plugin's exact tree hash first.
    NotTrusted {
        name: String,
    },
    /// The installed files are missing, unreadable, or changed since they were
    /// trusted; trust was revoked and the plugin disabled.
    Integrity {
        name: String,
        reason: String,
    },
    /// `private_config/plugins.toml` is invalid or has an unsupported version.
    Config(String),
    Io(String),
    /// The change committed, but its `extension.*` event could not be recorded.
    EventNotRecorded(String),
}

impl PluginError {
    pub const fn code(&self) -> ServiceErrorCode {
        match self {
            Self::InvalidRequest(_)
            | Self::InvalidManifest(_)
            | Self::UnsafeContent { .. }
            | Self::Executable { .. }
            | Self::TooLarge(_)
            | Self::Config(_) => ServiceErrorCode::InvalidArgument,
            Self::NotFound(_) => ServiceErrorCode::NotFound,
            Self::Exists(_) | Self::HashMismatch { .. } | Self::Integrity { .. } => {
                ServiceErrorCode::Conflict
            }
            Self::NotTrusted { .. } => ServiceErrorCode::PermissionDenied,
            Self::Io(_) | Self::EventNotRecorded(_) => ServiceErrorCode::Internal,
        }
    }

    /// Stable machine-readable reason (`data.reason` on the wire).
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::NotFound(_) => "plugin_not_found",
            Self::Exists(_) => "plugin_exists",
            Self::InvalidManifest(_) => "invalid_manifest",
            Self::UnsafeContent { .. } => "unsafe_content",
            Self::Executable { .. } => "executable_refused",
            Self::TooLarge(_) => "too_large",
            Self::HashMismatch { .. } => "hash_mismatch",
            Self::NotTrusted { .. } => "not_trusted",
            Self::Integrity { .. } => "integrity_failed",
            Self::Config(_) => "invalid_config",
            Self::Io(_) => "io",
            Self::EventNotRecorded(_) => "event_not_recorded",
        }
    }

    pub(crate) fn io(what: &str, path: &Path, error: &io::Error) -> Self {
        Self::Io(format!("cannot {what} `{}`: {error}", path.display()))
    }

    pub(crate) fn unsafe_content(path: &str, reason: impl Into<String>) -> Self {
        Self::UnsafeContent {
            path: path.to_owned(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) | Self::Config(message) | Self::Io(message) => {
                f.write_str(message)
            }
            Self::NotFound(name) => write!(f, "plugin `{name}` is not installed"),
            Self::Exists(name) => write!(
                f,
                "plugin `{name}` is already installed; uninstall it or install with replace"
            ),
            Self::InvalidManifest(message) => write!(f, "invalid plugin manifest: {message}"),
            Self::UnsafeContent { path, reason } => {
                write!(f, "refused plugin content `{}`: {reason}", shown_path(path))
            }
            Self::Executable { path } => write!(
                f,
                "refused plugin content `{}`: plugins may not ship executables unless `.mcp.json` declares them as a server command",
                shown_path(path)
            ),
            Self::TooLarge(message) => write!(f, "plugin is too large: {message}"),
            Self::HashMismatch { name } => write!(
                f,
                "plugin `{name}` changed since it was reviewed; review it again before trusting it"
            ),
            Self::NotTrusted { name } => write!(
                f,
                "plugin `{name}` is not trusted; review it and trust its exact tree hash first"
            ),
            Self::Integrity { name, reason } => write!(
                f,
                "plugin `{name}` failed its integrity check ({reason}); it was disabled and its trust revoked"
            ),
            Self::EventNotRecorded(message) => write!(
                f,
                "the change was applied, but its event could not be recorded: {message}"
            ),
        }
    }
}

impl std::error::Error for PluginError {}

/// A refused path comes from the refused plugin and may hold any character: control
/// characters (terminal escapes, carriage returns, line breaks) are shown escaped, so
/// the message cannot drive a terminal or fake lines wherever it is printed.
fn shown_path(path: &str) -> String {
    let mut shown = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_control() {
            shown.extend(c.escape_debug());
        } else {
            shown.push(c);
        }
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_paths_cannot_drive_the_terminal() {
        let hostile = "a\u{1b}]52;c;eA==\u{7}\r\nfake\tb";
        for error in [
            PluginError::unsafe_content(hostile, "character '\\u{1b}' is not allowed"),
            PluginError::Executable {
                path: hostile.into(),
            },
        ] {
            let message = error.to_string();
            assert!(!message.chars().any(char::is_control), "{message:?}");
            assert!(
                message.contains(r"`a\u{1b}]52;c;eA==\u{7}\r\nfake\tb`"),
                "{message}"
            );
        }
        assert_eq!(
            PluginError::unsafe_content("bin/tool.exe", "r").to_string(),
            "refused plugin content `bin/tool.exe`: r"
        );
    }
}
