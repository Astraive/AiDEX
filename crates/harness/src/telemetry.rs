//! Public tool events carry metadata only. Tool arguments, results, failure
//! details, and command lines stay in encrypted run history; the registry's
//! own `tool.*`, `file.changed`, `diff.created`, and the launcher's
//! `process.*` events reach the embedder's sink through [`ToolEventSink`].
use aidex_events::{Cursor, EventError, EventKind, EventScope, EventSink};
use std::sync::Arc;

/// Replaces the free text of tool and process events with fixed summaries
/// before passing them on. `process.output` is live-only and was redacted
/// line by line by the launcher; paths and counts pass unchanged.
pub(crate) struct ToolEventSink(Arc<dyn EventSink>);

impl ToolEventSink {
    pub(crate) fn new(events: Arc<dyn EventSink>) -> Self {
        Self(events)
    }
}

impl EventSink for ToolEventSink {
    fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
        let event = match event {
            EventKind::ToolRequested {
                call_id,
                tool_id,
                risk,
                ..
            } => EventKind::ToolRequested {
                call_id,
                tool_id,
                risk,
                summary: "native tool arguments stored in encrypted history".into(),
            },
            EventKind::ToolCompleted {
                call_id,
                duration_ms,
                ..
            } => EventKind::ToolCompleted {
                call_id,
                duration_ms,
                summary: "native tool completed; output in encrypted history".into(),
            },
            EventKind::ToolFailed { call_id, code, .. } => EventKind::ToolFailed {
                call_id,
                code,
                message: "native tool failed; details in encrypted history".into(),
            },
            EventKind::ProcessStarted {
                process_id,
                sandbox_level,
                ..
            } => EventKind::ProcessStarted {
                process_id,
                sandbox_level,
                command_summary: "native tool process".into(),
            },
            event => event,
        };
        self.0.emit(scope, event)
    }
}
