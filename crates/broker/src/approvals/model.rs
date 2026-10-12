//! Approval requests, their human-readable cards, and their lifecycle states.

use std::{fmt, str::FromStr};

use aidex_core::{EntityId, Timestamp};
use aidex_events::{RiskLevel, redact_text};
use aidex_policy::{Action, ApprovalScope, SandboxLevel};
use aidex_tools::{DiffKind, DiffPreview};
use serde::{Deserialize, Serialize};

use crate::{BrokerError, Origin, Principal};

/// Most bytes of unified diff text kept per file on a card.
const MAX_CARD_DIFF_BYTES: usize = 32 * 1024;

/// What the approver decided.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalResolution {
    /// Run this exact intent once.
    ApproveOnce,
    /// Run it, and allow identical intents (same tool, action, and targets) for the
    /// rest of the session until the grant expires or is revoked.
    ApproveSession,
    Deny,
}

impl ApprovalResolution {
    pub fn scope(self) -> Option<ApprovalScope> {
        match self {
            Self::ApproveOnce => Some(ApprovalScope::Once),
            Self::ApproveSession => Some(ApprovalScope::Session),
            Self::Deny => None,
        }
    }
}

impl FromStr for ApprovalResolution {
    type Err = BrokerError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "approve_once" => Ok(Self::ApproveOnce),
            "approve_session" => Ok(Self::ApproveSession),
            "deny" => Ok(Self::Deny),
            other => Err(BrokerError::InvalidRequest(format!(
                "unknown decision `{other}` (expected approve_once, approve_session, or deny)"
            ))),
        }
    }
}

/// Lifecycle status, without details. Stable wire names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Approved,
    Denied,
    Expired,
    Revoked,
    Cancelled,
}

impl ApprovalStatus {
    pub const ALL: [Self; 6] = [
        Self::Pending,
        Self::Approved,
        Self::Denied,
        Self::Expired,
        Self::Revoked,
        Self::Cancelled,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Approved => "approved",
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for ApprovalStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ApprovalStatus {
    type Err = BrokerError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == text)
            .ok_or_else(|| BrokerError::InvalidRequest(format!("unknown approval state `{text}`")))
    }
}

/// Where an approval is in its lifecycle. Transitions out of `pending` (and from
/// `approved` to `revoked`) are atomic compare-and-set updates, so concurrent
/// decisions have exactly one winner.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalState {
    Pending,
    Approved {
        scope: ApprovalScope,
        by: Principal,
        at: Timestamp,
    },
    Denied {
        by: Principal,
        at: Timestamp,
    },
    Expired {
        at: Timestamp,
    },
    Revoked {
        by: Principal,
        at: Timestamp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Cancelled {
        at: Timestamp,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl ApprovalState {
    pub fn status(&self) -> ApprovalStatus {
        match self {
            Self::Pending => ApprovalStatus::Pending,
            Self::Approved { .. } => ApprovalStatus::Approved,
            Self::Denied { .. } => ApprovalStatus::Denied,
            Self::Expired { .. } => ApprovalStatus::Expired,
            Self::Revoked { .. } => ApprovalStatus::Revoked,
            Self::Cancelled { .. } => ApprovalStatus::Cancelled,
        }
    }
}

/// Who asked: the invocation origin plus the agent it ran under.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requester {
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<EntityId>,
}

impl fmt::Display for Requester {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.origin, &self.agent_id) {
            (Origin::Agent, Some(agent)) => write!(f, "agent {agent}"),
            (origin, Some(agent)) => write!(f, "{origin} (agent {agent})"),
            (origin, None) => write!(f, "{origin}"),
        }
    }
}

/// A secret the action will use (name and consumer only; never the value).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretNotice {
    pub secret_name: String,
    pub consumer: String,
}

/// One file of a reviewable diff on a card.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardDiffFile {
    pub path: String,
    /// `created`, `modified`, or `deleted`.
    pub kind: String,
    pub additions: u64,
    pub deletions: u64,
    /// Redacted, bounded unified diff text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified: Option<String>,
    pub binary: bool,
    pub truncated: bool,
}

/// The diff a write will apply, as shown for review.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardDiff {
    pub files: Vec<CardDiffFile>,
    pub files_omitted: u64,
    pub additions: u64,
    pub deletions: u64,
}

impl CardDiff {
    /// Copy a tool preview; diff text is redacted (it may contain credentials being
    /// written) and bounded per file.
    pub fn from_preview(preview: &DiffPreview) -> Self {
        let files = preview
            .files
            .iter()
            .map(|file| {
                let (unified, cut) = match &file.unified {
                    Some(text) => {
                        let redacted = redact_text(text);
                        let (bounded, cut) = bound(&redacted, MAX_CARD_DIFF_BYTES);
                        (Some(bounded), cut)
                    }
                    None => (None, false),
                };
                CardDiffFile {
                    path: file.path.as_str().to_owned(),
                    kind: match file.kind {
                        DiffKind::Created => "created",
                        DiffKind::Modified => "modified",
                        DiffKind::Deleted => "deleted",
                    }
                    .to_owned(),
                    additions: file.additions,
                    deletions: file.deletions,
                    unified,
                    binary: file.binary,
                    truncated: file.truncated || cut,
                }
            })
            .collect();
        Self {
            files,
            files_omitted: preview.files_omitted,
            additions: preview.additions(),
            deletions: preview.deletions(),
        }
    }
}

/// Cut `text` to at most `max` bytes on a line (or char) boundary.
pub(crate) fn bound(text: &str, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text.to_owned(), false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rfind('\n').map_or(end, |newline| newline + 1);
    (text[..cut].to_owned(), true)
}

/// The human-readable approval card (spec §14) shown by the CLI, desktop, and mobile.
/// It never contains secret values; command lines and diff text are redacted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalCard {
    pub approval_id: EntityId,
    pub requester: Requester,
    pub tool_id: String,
    pub action: Action,
    /// What was asked, e.g. `git push origin main`.
    pub summary: String,
    /// What it will do, e.g. "Uploads local commits on branch main to origin
    /// (github.com)".
    pub effect: String,
    pub workspace_id: EntityId,
    pub workspace_name: String,
    pub session_id: EntityId,
    pub risk: RiskLevel,
    /// What the approver may choose (`once`, and `session` when policy allows).
    pub scope_options: Vec<ApprovalScope>,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    /// The policy rule that asked for approval, and why.
    pub rule: String,
    pub reason: String,
    /// Minimum isolation the action runs at once approved.
    pub sandbox: SandboxLevel,
    /// Files, hosts, and remotes the action touches.
    pub targets: Vec<String>,
    /// The redacted command line for process-running tools.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretNotice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_preview: Option<CardDiff>,
    /// What "approve for session" would allow, when offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_grant: Option<String>,
    /// Extra cautions (for example a remote URL that embeds credentials).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// A session grant created by `approve_session`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantInfo {
    pub expires_at: Timestamp,
    /// Approved, not revoked, and not expired at the time of reading.
    pub active: bool,
}

/// An approval as stored: its card, state, and binding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRecord {
    pub approval_id: EntityId,
    pub session_id: EntityId,
    pub workspace_id: EntityId,
    pub call_id: EntityId,
    pub tool_id: String,
    pub state: ApprovalState,
    pub card: ApprovalCard,
    /// SHA-256 binding over the plan (minus its random ids), the evaluated intents'
    /// fingerprints, and the origin. Continuation re-plans and must reproduce it.
    pub intent_fingerprint: String,
    /// When the approved call was executed (approvals are single-use).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<GrantInfo>,
}

impl ApprovalRecord {
    pub fn status(&self) -> ApprovalStatus {
        self.state.status()
    }
}

/// Which approvals to list.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApprovalFilter {
    pub session_id: Option<EntityId>,
    pub workspace_id: Option<EntityId>,
    pub status: Option<ApprovalStatus>,
    /// At most this many, newest first (default and maximum 500).
    pub limit: Option<usize>,
}

impl ApprovalFilter {
    pub fn pending() -> Self {
        Self {
            status: Some(ApprovalStatus::Pending),
            ..Self::default()
        }
    }

    pub fn for_session(session_id: EntityId) -> Self {
        Self {
            session_id: Some(session_id),
            ..Self::default()
        }
    }
}
