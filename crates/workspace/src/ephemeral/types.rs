//! Public ephemeral-workspace request and result types.

use aidex_core::{EntityId, WorkspacePath};
use serde::{Deserialize, Serialize};

use crate::{SkippedEntry, Workspace};

/// Bounds for copying and reviewing ephemeral workspaces.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EphemeralLimits {
    /// Maximum regular files copied from the source or examined in a review.
    pub max_files: u64,
    /// Maximum total bytes copied, or actually hashed per side in a review.
    /// The ephemeral tree and source conflict checks have separate budgets.
    pub max_bytes: u64,
    /// Files larger than this (either side) get no unified diff.
    pub max_diff_input_bytes: u64,
    /// Maximum unified-diff text per file; longer diffs are cut at a line boundary.
    pub max_file_diff_bytes: usize,
    /// Maximum unified-diff text per review across all files.
    pub max_total_diff_bytes: usize,
}

impl Default for EphemeralLimits {
    fn default() -> Self {
        Self {
            max_files: 50_000,
            max_bytes: 2 * 1024 * 1024 * 1024,
            max_diff_input_bytes: 1024 * 1024,
            max_file_diff_bytes: 64 * 1024,
            max_total_diff_bytes: 256 * 1024,
        }
    }
}

/// Result of creating an ephemeral workspace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EphemeralCreated {
    pub workspace: Workspace,
    /// Regular files copied (and recorded in the baseline).
    pub files: u64,
    /// Bytes copied.
    pub bytes: u64,
    /// Source entries that were not copied (links, unsupported names, special files).
    pub skipped: Vec<SkippedEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

impl ChangeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
        }
    }
}

/// Why a change cannot be applied without overwriting work in the source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictReason {
    /// The source file changed since the ephemeral copy was made.
    SourceChanged,
    /// A file appeared in the source where the ephemeral workspace adds one.
    SourceAdded,
    /// The source file was removed or replaced by a non-file since the copy.
    SourceRemoved,
    /// The source path is unsafe now (escapes through a link, or a link leaf).
    UnsafePath,
}

impl ConflictReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::SourceChanged => "source_changed",
            Self::SourceAdded => "source_added",
            Self::SourceRemoved => "source_removed",
            Self::UnsafePath => "unsafe_path",
        }
    }
}

/// One reviewable change between the baseline and the ephemeral tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileChange {
    pub path: WorkspacePath,
    pub kind: ChangeKind,
    /// Either side is not UTF-8 text; no unified diff is produced.
    pub binary: bool,
    /// Size of the new content (`None` for deletions).
    pub size: Option<u64>,
    /// SHA-256 of the new content (`None` for deletions).
    pub sha256: Option<String>,
    /// Unified diff from the current source file to the ephemeral file, bounded by
    /// [`EphemeralLimits`].
    pub unified_diff: Option<String>,
    /// The diff was cut short or omitted because of a limit.
    pub diff_truncated: bool,
    /// Set when applying would overwrite source work; such changes are never applied.
    pub conflict: Option<ConflictReason>,
}

/// The reviewable difference between an ephemeral workspace and its baseline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EphemeralDiff {
    pub workspace_id: EntityId,
    pub source_id: EntityId,
    /// Changes in path order.
    pub changes: Vec<FileChange>,
    /// Paths whose change conflicts with the source (also flagged per change).
    pub conflicts: Vec<WorkspacePath>,
    /// Ephemeral entries that can never be applied (links, special files,
    /// unsupported names, a directory replacing a file).
    pub unsupported: Vec<SkippedEntry>,
    /// SHA-256 over every change (kind, path, new hash, conflict). Pass it to apply to
    /// guarantee that exactly the reviewed state is applied.
    pub digest: String,
}

/// Which reviewed changes to apply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplySelection {
    /// Every non-conflicting change.
    All,
    /// Only these paths; each must be a current change.
    Paths(Vec<WorkspacePath>),
}

/// A selected change that could not be written.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyFailure {
    pub path: WorkspacePath,
    pub message: String,
}

/// Outcome of applying selected changes to the source workspace.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyReport {
    /// Changes written into the source (and folded into the baseline).
    pub applied: Vec<WorkspacePath>,
    /// Selected changes refused because the source (or the ephemeral file) changed.
    pub skipped_conflicts: Vec<WorkspacePath>,
    /// Selected changes that failed with an I/O error; the source file is unchanged.
    pub failed: Vec<ApplyFailure>,
}
