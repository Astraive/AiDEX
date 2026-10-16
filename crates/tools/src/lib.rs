//! Typed, two-phase AiDEX tools confined to a workspace.
//!
//! - [`Tool`]: `plan` (validate, resolve, compute [`PlanFacts`] and a reviewable
//!   [`DiffPreview`], no side effects) then `execute` (only with an
//!   [`ExecutionPermit`] bound to the plan's [`PlanFingerprint`]).
//! - [`ToolRegistry`]: descriptors with capability and risk metadata for policy,
//!   model-facing [`ToolSpec`]s, permit-checked single-use execution, per-path write
//!   locks, and `tool.*` / `file.changed` / `diff.created` events.
//! - [`ProcessLauncher`]: the only way tools run processes (git, shell); the broker
//!   implements it over `aidex-sandbox`. `DirectLauncher` (feature
//!   `direct-launcher`) is for tests and development only.
//!
//! Only the execution broker holds a registry; the harness holds a broker handle.

mod builtin;
mod context;
mod descriptor;
mod diff;
mod error;
mod fs;
mod git;
mod launcher;
mod locks;
mod output;
mod plan;
mod registry;
pub mod schema;
mod sensitive;
mod shell;
mod text;
mod tool;
mod walk;

pub use builtin::BUILTIN_TOOL_IDS;
pub use context::{CancelToken, ToolContext, ToolLimits};
pub use descriptor::{RiskProfile, ToolDescriptor, ToolId, ToolSpec};
pub use diff::{DiffKind, DiffPreview, FileDiff};
pub use error::{InputError, LaunchError, ToolError};
pub use fs::path_scope;
#[cfg(feature = "direct-launcher")]
pub use launcher::DirectLauncher;
pub use launcher::{LaunchOutcome, LaunchRequest, ProcessLauncher};
pub use output::{MAX_OUTPUT_SUMMARY_BYTES, PathChange, ToolOutput};
pub use plan::{
    EntryKind, ExecutionPermit, MAX_SUMMARY_BYTES, NetworkFacts, PathAccess, PermitIssuer,
    PlanFacts, PlanFingerprint, PlanParts, PlanPath, ToolPlan,
};
pub use registry::ToolRegistry;
pub use sensitive::looks_sensitive;
pub use tool::Tool;
