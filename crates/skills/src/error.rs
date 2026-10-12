//! Skill errors with stable service codes and `data.reason` strings.
//!
//! Messages never quote skill text or state-file values (either could hold a
//! pasted credential); they name the skill, the field, and the problem.

use std::{fmt, path::PathBuf};

use aidex_core::ServiceErrorCode;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SkillError {
    /// A skill id or name is malformed.
    InvalidId(String),
    /// No skill with this id is visible in the catalog.
    NotFound(String),
    /// A draft (or an enable request) was rejected; the message says why.
    Invalid(String),
    /// Draft text looks like it contains a credential; skills are stored in
    /// plain text on the drive, so it is refused rather than redacted.
    CredentialDetected,
    /// The skill cannot be used for invocation: unknown, disabled, or invalid.
    Unavailable {
        id: String,
        reason: String,
    },
    /// A project or repo skill was named without a workspace.
    WorkspaceRequired,
    /// The change conflicts with what is on disk.
    Conflict(String),
    /// `private_config/skills.toml` does not match its closed schema.
    State {
        path: PathBuf,
        message: String,
    },
    /// `private_config/skills.toml` was written by a different schema version.
    StateVersion {
        path: PathBuf,
        found: u32,
    },
    /// A managed path is a symbolic link, junction, or not the expected kind.
    UnsafePath(PathBuf),
    Io {
        path: PathBuf,
        message: String,
    },
    /// The change committed but its event could not be recorded.
    EventNotRecorded(String),
}

impl SkillError {
    pub const fn code(&self) -> ServiceErrorCode {
        match self {
            Self::InvalidId(_)
            | Self::Invalid(_)
            | Self::CredentialDetected
            | Self::WorkspaceRequired
            | Self::State { .. }
            | Self::UnsafePath(_) => ServiceErrorCode::InvalidArgument,
            Self::NotFound(_) => ServiceErrorCode::NotFound,
            Self::Unavailable { .. } => ServiceErrorCode::PermissionDenied,
            Self::Conflict(_) => ServiceErrorCode::Conflict,
            Self::StateVersion { .. } => ServiceErrorCode::IncompatibleVersion,
            Self::Io { .. } | Self::EventNotRecorded(_) => ServiceErrorCode::Internal,
        }
    }

    /// Stable machine-readable reason (RPC `data.reason`).
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::InvalidId(_) => "invalid_skill_id",
            Self::NotFound(_) => "skill_not_found",
            Self::Invalid(_) => "invalid_skill",
            Self::CredentialDetected => "credential_detected",
            Self::Unavailable { .. } => "skill_unavailable",
            Self::WorkspaceRequired => "workspace_required",
            Self::Conflict(_) => "skill_conflict",
            Self::State { .. } => "invalid_state_file",
            Self::StateVersion { .. } => "unsupported_state_version",
            Self::UnsafePath(_) => "unsafe_path",
            Self::Io { .. } => "io",
            Self::EventNotRecorded(_) => "event_not_recorded",
        }
    }

    pub(crate) fn io(path: impl Into<PathBuf>, error: &std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            message: error.to_string(),
        }
    }
}

impl fmt::Display for SkillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(message) | Self::Invalid(message) | Self::Conflict(message) => {
                f.write_str(message)
            }
            Self::NotFound(id) => write!(f, "no skill `{id}` was found"),
            Self::CredentialDetected => f.write_str(
                "the skill text looks like it contains a credential; store secrets in the vault instead",
            ),
            Self::Unavailable { id, reason } => write!(f, "skill `{id}` is unavailable: {reason}"),
            Self::WorkspaceRequired => {
                f.write_str("project and repo skills need a workspace_id")
            }
            Self::State { path, message } => write!(f, "{} is invalid: {message}", path.display()),
            Self::StateVersion { path, found } => write!(
                f,
                "{} has version {found}; this build supports version {}",
                path.display(),
                crate::STATE_VERSION
            ),
            Self::UnsafePath(path) => write!(
                f,
                "{} is a symbolic link, junction, or not the expected kind of entry",
                path.display()
            ),
            Self::Io { path, message } => write!(f, "{}: {message}", path.display()),
            Self::EventNotRecorded(message) => write!(
                f,
                "the change was applied, but its event could not be recorded: {message}"
            ),
        }
    }
}

impl std::error::Error for SkillError {}
