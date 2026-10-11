//! Public metadata events cannot carry private tool arguments or results.
use aidex_events::{Cursor, EventError, EventKind, EventScope, EventSink};
use std::sync::Arc;
pub(crate) struct ToolEventSink(pub Arc<dyn EventSink>);
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
                summary: "tool arguments stored in encrypted history".into(),
            },
            EventKind::ToolCompleted {
                call_id,
                duration_ms,
                ..
            } => EventKind::ToolCompleted {
                call_id,
                duration_ms,
                summary: "tool output stored in encrypted history".into(),
            },
            EventKind::ToolFailed { call_id, code, .. } => EventKind::ToolFailed {
                call_id,
                code,
                message: "tool failed; private details remain in encrypted history".into(),
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
