//! The `EventSink` contract shared by every implementation.

mod common;

use aidex_core::{Timestamp, WorkspacePath};
use aidex_events::{
    Cursor, EventEnvelope, EventError, EventFilter, EventKind, EventScope, EventSink,
    MAX_EVENT_BYTES, MemoryEventSink, NullEventSink, RetentionPolicy,
};
use common::{id, session_scope, task_started, temp_log};
use std::sync::Arc;

fn emit_sequence(sink: &dyn EventSink) -> Vec<Cursor> {
    vec![
        sink.emit(session_scope(), task_started("a")).unwrap(),
        sink.emit(session_scope(), EventKind::AgentDelta { text: "x".into() })
            .unwrap(),
        sink.emit(session_scope(), task_started("b")).unwrap(),
    ]
}

#[test]
fn event_log_and_memory_sink_assign_the_same_cursors() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let expected = [Cursor::new(1), Cursor::new(1), Cursor::new(2)];
    assert_eq!(emit_sequence(&fixture.log), expected);
    let memory = MemoryEventSink::new();
    assert_eq!(emit_sequence(&memory), expected);
    assert_eq!(
        memory.names(),
        ["task.started", "agent.delta", "task.started"]
    );
    assert_eq!(memory.len(), 3);
    let taken = memory.take();
    assert_eq!(taken[2].cursor, Cursor::new(2));
    assert!(memory.is_empty());
    assert_eq!(
        fixture
            .log
            .replay(&EventFilter::all(), Cursor::ZERO, 10)
            .unwrap()
            .events
            .len(),
        2
    );
}

#[test]
fn every_sink_applies_the_log_validation_and_redaction() {
    let memory = Arc::new(MemoryEventSink::new());
    let sinks: [Arc<dyn EventSink>; 2] = [memory.clone(), Arc::new(NullEventSink)];
    for sink in &sinks {
        assert_eq!(
            sink.emit(
                EventScope {
                    workspace_id: None,
                    session_id: Some(id("ses_one")),
                },
                EventKind::SessionStarted {},
            )
            .map_err(|error| matches!(error, EventError::InvalidScope(_))),
            Err(true)
        );
        assert_eq!(
            sink.emit(
                session_scope(),
                EventKind::TaskStarted {
                    task_id: id(&format!("{}{}", "ghp_", "GHFAKE0fake1FAKE2fake3FAKE4fake5")),
                },
            ),
            Err(EventError::SensitiveIdentifier)
        );
        sink.emit(
            session_scope(),
            EventKind::SessionFailed {
                reason: "token=Zz9Yy8Xx7Ww6Vv5Uu4Tt3".into(),
            },
        )
        .unwrap();
    }
    let recorded = memory.events();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].event,
        EventKind::SessionFailed {
            reason: "token=[REDACTED]".into()
        }
    );
    // Shared references and Arcs are sinks too.
    let by_ref: &dyn EventSink = &*memory;
    assert_eq!(
        (&by_ref).emit(session_scope(), task_started("c")).unwrap(),
        Cursor::new(2)
    );
}

#[test]
fn every_sink_rejects_aggregate_payloads_over_the_envelope_limit() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let memory = MemoryEventSink::new();
    let null = NullEventSink;
    let oversized = EventKind::DiffCreated {
        diff_id: id("diff_large"),
        files: (0..20_000)
            .map(|index| {
                aidex_core::WorkspacePath::new(format!("src/module_{index}/file.rs")).unwrap()
            })
            .collect(),
        additions: 1,
        deletions: 0,
    };
    for (sink, expected) in [
        (&fixture.log as &dyn EventSink, Cursor::new(1)),
        (&memory as &dyn EventSink, Cursor::new(1)),
        (&null as &dyn EventSink, Cursor::ZERO),
    ] {
        assert!(matches!(
            sink.emit(EventScope::workspace(id("wsp_one")), oversized.clone()),
            Err(EventError::TooLarge { limit, .. }) if limit == aidex_events::MAX_EVENT_BYTES
        ));
        assert_eq!(
            sink.emit(session_scope(), task_started("after_rejection"))
                .unwrap(),
            expected
        );
    }
}

/// A synthetic payload whose public wire envelope reaches the limit at `cursor`.
/// Every path stays below its individual bound, so only the aggregate size matters.
fn diff_at_envelope_limit(cursor: Cursor) -> EventKind {
    let mut envelope = EventEnvelope {
        version: EventEnvelope::current_version(),
        cursor,
        scope: EventScope::global(),
        occurred_at: Timestamp::from_unix_millis(0).unwrap(),
        event: EventKind::DiffCreated {
            diff_id: id("diff_boundary"),
            files: vec![WorkspacePath::new("x".repeat(4096)).unwrap(); 63],
            additions: 1,
            deletions: 0,
        },
    };
    // An additional ASCII path contributes its text plus two quotes and a comma.
    let padding_bytes = MAX_EVENT_BYTES - serde_json::to_string(&envelope).unwrap().len() - 3;
    if let EventKind::DiffCreated { files, .. } = &mut envelope.event {
        files.push(WorkspacePath::new("p".repeat(padding_bytes)).unwrap());
    }
    assert_eq!(
        serde_json::to_string(&envelope).unwrap().len(),
        MAX_EVENT_BYTES
    );
    envelope.event
}

#[test]
fn null_sink_rejects_payloads_that_outgrow_the_limit_at_later_cursors() {
    let event = diff_at_envelope_limit(Cursor::ZERO);
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let memory = MemoryEventSink::new();
    for sink in [&fixture.log as &dyn EventSink, &memory as &dyn EventSink] {
        for _ in 0..9 {
            sink.emit(EventScope::global(), EventKind::DriveSafeToRemove {})
                .unwrap();
        }
        // A real cursor of 10 needs one more decimal digit than the zero envelope.
        assert!(matches!(
            sink.emit(EventScope::global(), event.clone()),
            Err(EventError::TooLarge { bytes, limit })
                if bytes == MAX_EVENT_BYTES + 1 && limit == MAX_EVENT_BYTES
        ));
        assert_eq!(
            sink.emit(EventScope::global(), EventKind::DriveSafeToRemove {})
                .unwrap(),
            Cursor::new(10)
        );
    }
    assert!(matches!(
        NullEventSink.emit(EventScope::global(), event),
        Err(EventError::TooLarge { bytes, limit })
            if bytes == MAX_EVENT_BYTES + 19 && limit == MAX_EVENT_BYTES
    ));
}

#[test]
fn null_sink_accepts_the_full_limit_with_maximum_cursor_space_reserved() {
    let mut event = diff_at_envelope_limit(Cursor::new(u64::MAX));
    assert_eq!(
        NullEventSink.emit(EventScope::global(), event.clone()),
        Ok(Cursor::ZERO)
    );
    if let EventKind::DiffCreated { files, .. } = &mut event {
        let last = files.last_mut().unwrap();
        *last = WorkspacePath::new(format!("{}x", last.as_str())).unwrap();
    }
    assert!(matches!(
        NullEventSink.emit(EventScope::global(), event),
        Err(EventError::TooLarge { bytes, limit })
            if bytes == MAX_EVENT_BYTES + 1 && limit == MAX_EVENT_BYTES
    ));
}
