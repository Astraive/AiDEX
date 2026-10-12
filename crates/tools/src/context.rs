//! The per-call execution context: identity, workspace root, events, limits.

use std::{fmt, sync::Arc, time::Duration};

pub use aidex_core::CancelToken;
use aidex_core::EntityId;
use aidex_events::{EventScope, EventSink};
use aidex_workspace::ConfinedRoot;

use crate::{ProcessLauncher, ToolError};

/// `Err(ToolError::Cancelled)` once the call's token (shared with the broker and
/// harness, `aidex_core::CancelToken`) is cancelled or its deadline passed.
pub(crate) fn check_cancel(cancel: &CancelToken) -> Result<(), ToolError> {
    if cancel.is_stopped() {
        Err(ToolError::Cancelled)
    } else {
        Ok(())
    }
}

/// Bounds applied to every tool call. Inputs may ask for less, never more.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolLimits {
    /// Largest `fs.read` content returned, in bytes.
    pub max_read_bytes: u64,
    /// Largest file `fs.write`/`fs.patch` will produce or replace, in bytes.
    pub max_write_bytes: u64,
    /// Most matches `fs.search`/`fs.glob` return.
    pub max_search_results: usize,
    /// Most bytes of textual output (process output, `git diff`) per stream.
    pub max_output_bytes: usize,
    /// Files larger than this are skipped by `fs.search`.
    pub max_search_file_bytes: u64,
    /// Most directory entries a walk (list, glob, search, recursive delete) visits.
    pub max_walk_entries: usize,
    /// Most bytes of unified diff text in one preview.
    pub max_diff_bytes: usize,
    /// Longest wall-clock time a launched process may run.
    pub process_timeout: Duration,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            max_read_bytes: 512 * 1024,
            max_write_bytes: 4 * 1024 * 1024,
            max_search_results: 200,
            max_output_bytes: 256 * 1024,
            max_search_file_bytes: 2 * 1024 * 1024,
            max_walk_entries: 20_000,
            max_diff_bytes: 128 * 1024,
            process_timeout: Duration::from_secs(120),
        }
    }
}

/// Everything a tool needs for one call. Built by the broker per call; the harness
/// never constructs one for direct execution (it holds a broker handle instead).
#[derive(Clone)]
pub struct ToolContext {
    pub session_id: EntityId,
    pub workspace_id: EntityId,
    pub call_id: EntityId,
    /// The confined workspace root every path is resolved against.
    pub root: Arc<ConfinedRoot>,
    /// Where the registry emits `tool.*`, `file.changed`, and `diff.created`.
    pub events: Arc<dyn EventSink>,
    pub cancel: CancelToken,
    pub limits: ToolLimits,
    /// Launches processes for git and shell tools; `None` makes them unavailable.
    pub processes: Option<Arc<dyn ProcessLauncher>>,
}

impl ToolContext {
    pub fn new(
        session_id: EntityId,
        workspace_id: EntityId,
        call_id: EntityId,
        root: Arc<ConfinedRoot>,
        events: Arc<dyn EventSink>,
    ) -> Self {
        Self {
            session_id,
            workspace_id,
            call_id,
            root,
            events,
            cancel: CancelToken::new(),
            limits: ToolLimits::default(),
            processes: None,
        }
    }

    pub fn with_limits(mut self, limits: ToolLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub fn with_launcher(mut self, launcher: Arc<dyn ProcessLauncher>) -> Self {
        self.processes = Some(launcher);
        self
    }

    /// The same context for another call id (the broker builds one per call).
    pub fn for_call(&self, call_id: EntityId) -> Self {
        Self {
            call_id,
            ..self.clone()
        }
    }

    /// The event scope of this call (workspace + session).
    pub fn scope(&self) -> EventScope {
        EventScope::session(self.workspace_id.clone(), self.session_id.clone())
    }

    /// The canonical workspace root as displayed (no Windows verbatim prefix).
    pub fn root_display(&self) -> String {
        self.root.display_root().display().to_string()
    }

    pub(crate) fn launcher(&self) -> Result<&Arc<dyn ProcessLauncher>, ToolError> {
        self.processes
            .as_ref()
            .ok_or(ToolError::LauncherUnavailable)
    }
}

impl fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolContext")
            .field("session_id", &self.session_id)
            .field("workspace_id", &self.workspace_id)
            .field("call_id", &self.call_id)
            .field("root", &self.root.display_root())
            .field("cancelled", &self.cancel.is_stopped())
            .field("limits", &self.limits)
            .field("launcher", &self.processes.is_some())
            .finish_non_exhaustive()
    }
}
