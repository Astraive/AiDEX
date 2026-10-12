//! Broker errors with stable service codes and machine-readable reasons.
//!
//! A refused or failed *tool call* is not an error: it is a
//! [`crate::BrokerOutcome::Denied`] or [`crate::BrokerOutcome::Failed`]. Errors are
//! for requests the broker cannot process at all (emergency stop, unknown tool or
//! workspace, approval state conflicts, storage or audit failures).

use std::{error::Error, fmt};

use aidex_core::{EntityId, ServiceErrorCode};

/// Why an approval could not be resolved, revoked, or continued. `as_str` is stable.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ApprovalRefusal {
    /// No approval with this id exists.
    NotFound,
    /// The approval is still waiting for a decision.
    Pending,
    /// The approval was denied.
    Denied,
    /// The approval expired before it was resolved.
    Expired,
    /// The approval (or the session grant it created) was revoked.
    Revoked,
    /// The approval was cancelled (session cancelled, emergency stop, daemon restart).
    Cancelled,
    /// A decision was already recorded; only pending approvals can be resolved.
    AlreadyResolved,
    /// The approved call was already executed (approvals are single-use).
    AlreadyConsumed,
    /// The approver chose a scope the request did not offer.
    ScopeNotOffered,
    /// Re-planning produced a different intent than the one approved.
    IntentChanged,
    /// The approved invocation is no longer held by this broker (daemon restart).
    InvocationLost,
}

impl ApprovalRefusal {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "approval_not_found",
            Self::Pending => "approval_pending",
            Self::Denied => "approval_denied",
            Self::Expired => "approval_expired",
            Self::Revoked => "approval_revoked",
            Self::Cancelled => "approval_cancelled",
            Self::AlreadyResolved => "already_resolved",
            Self::AlreadyConsumed => "already_consumed",
            Self::ScopeNotOffered => "scope_not_offered",
            Self::IntentChanged => "intent_changed",
            Self::InvocationLost => "invocation_lost",
        }
    }

    pub const fn code(self) -> ServiceErrorCode {
        match self {
            Self::NotFound | Self::InvocationLost => ServiceErrorCode::NotFound,
            Self::ScopeNotOffered => ServiceErrorCode::InvalidArgument,
            Self::Denied | Self::Expired | Self::Revoked | Self::Cancelled => {
                ServiceErrorCode::PermissionDenied
            }
            Self::Pending | Self::AlreadyResolved | Self::AlreadyConsumed | Self::IntentChanged => {
                ServiceErrorCode::Conflict
            }
        }
    }
}

impl fmt::Display for ApprovalRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Every failure the broker reports. Messages never contain secret values or tool
/// input; they name ids, tools, and stable reasons.
#[derive(Debug)]
pub enum BrokerError {
    /// An emergency stop is in effect: no new tool execution until
    /// `resume_operations`.
    Stopped { reason: String },
    /// No tool with this id is registered.
    UnknownTool(String),
    /// The workspace is unknown or unavailable.
    Workspace {
        code: ServiceErrorCode,
        reason: &'static str,
        message: String,
    },
    /// An approval transition or continuation was refused.
    Approval {
        approval_id: EntityId,
        refusal: ApprovalRefusal,
        message: String,
    },
    /// The principal may not perform this operation (for example a remote device
    /// changing policy).
    Unauthorized(String),
    /// The tool refused to plan the call (dry runs only; submissions report this as
    /// [`crate::BrokerOutcome::Failed`]).
    Tool {
        code: ServiceErrorCode,
        reason: &'static str,
        message: String,
    },
    /// The request itself is malformed.
    InvalidRequest(String),
    /// The approval database failed; nothing was changed by the failing statement.
    Store(String),
    /// Policy settings could not be read or written.
    Settings(String),
    /// An audit event could not be recorded. `committed` says whether the change it
    /// describes had already been applied (it is not undone).
    Audit { committed: bool, message: String },
    /// Any other internal failure (id generation, scratch directories).
    Internal(String),
}

impl BrokerError {
    pub fn code(&self) -> ServiceErrorCode {
        match self {
            Self::Stopped { .. } | Self::Unauthorized(_) => ServiceErrorCode::PermissionDenied,
            Self::UnknownTool(_) => ServiceErrorCode::NotFound,
            Self::Workspace { code, .. } | Self::Tool { code, .. } => *code,
            Self::Approval { refusal, .. } => refusal.code(),
            Self::InvalidRequest(_) => ServiceErrorCode::InvalidArgument,
            Self::Store(_) | Self::Settings(_) | Self::Audit { .. } | Self::Internal(_) => {
                ServiceErrorCode::Internal
            }
        }
    }

    /// Stable machine-readable reason (`data.reason` on the daemon API).
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Stopped { .. } => "emergency_stop",
            Self::UnknownTool(_) => "unknown_tool",
            Self::Workspace { reason, .. } | Self::Tool { reason, .. } => reason,
            Self::Approval { refusal, .. } => refusal.as_str(),
            Self::Unauthorized(_) => "unauthorized",
            Self::InvalidRequest(_) => "invalid_request",
            Self::Store(_) => "approval_store",
            Self::Settings(_) => "policy_settings",
            Self::Audit { .. } => "event_not_recorded",
            Self::Internal(_) => "internal",
        }
    }

    pub(crate) fn approval(
        approval_id: &EntityId,
        refusal: ApprovalRefusal,
        message: impl Into<String>,
    ) -> Self {
        Self::Approval {
            approval_id: approval_id.clone(),
            refusal,
            message: message.into(),
        }
    }

    pub(crate) fn internal(context: &str, error: impl fmt::Display) -> Self {
        Self::Internal(format!("{context}: {error}"))
    }
}

impl fmt::Display for BrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped { reason } => write!(
                f,
                "an emergency stop is in effect ({reason}); resume operations to run tools again"
            ),
            Self::UnknownTool(tool) => write!(f, "no tool `{tool}` is registered"),
            Self::Workspace { message, .. } => write!(f, "workspace: {message}"),
            Self::Tool { message, .. } => write!(f, "tool: {message}"),
            Self::Approval {
                approval_id,
                message,
                ..
            } => write!(f, "approval {approval_id}: {message}"),
            Self::Unauthorized(message) => write!(f, "not authorized: {message}"),
            Self::InvalidRequest(message) => write!(f, "invalid request: {message}"),
            Self::Store(message) => write!(f, "approval store: {message}"),
            Self::Settings(message) => write!(f, "policy settings: {message}"),
            Self::Audit { committed, message } => {
                if *committed {
                    write!(
                        f,
                        "the change was applied, but its audit event was not recorded: {message}"
                    )
                } else {
                    write!(
                        f,
                        "the audit event could not be recorded, so nothing ran: {message}"
                    )
                }
            }
            Self::Internal(message) => write!(f, "internal broker error: {message}"),
        }
    }
}

impl Error for BrokerError {}

impl From<rusqlite::Error> for BrokerError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error.to_string())
    }
}

impl From<aidex_workspace::WorkspaceError> for BrokerError {
    fn from(error: aidex_workspace::WorkspaceError) -> Self {
        Self::Workspace {
            code: error.code(),
            reason: error.reason(),
            message: error.to_string(),
        }
    }
}
