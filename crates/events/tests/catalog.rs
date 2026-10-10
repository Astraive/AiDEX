//! Catalog wire shape, envelope versioning, redaction, and identifier safety.

mod common;

use aidex_events::{
    Cursor, ENVELOPE_VERSION, EVENT_NAMES, EventEnvelope, EventError, EventFilter, EventKind,
    EventScope, MAX_EVENT_BYTES, MessageRole, RetentionPolicy, SubscriptionItem, redact,
};
use common::{id, samples, session_scope, temp_log, ws};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[test]
fn every_variant_has_a_unique_dotted_name_matching_its_wire_type() {
    let kinds = samples("text");
    let names: Vec<_> = kinds.iter().map(EventKind::name).collect();
    assert_eq!(
        names, EVENT_NAMES,
        "samples must cover the catalog in order"
    );
    assert_eq!(names.iter().collect::<BTreeSet<_>>().len(), names.len());
    for kind in &kinds {
        let wire = serde_json::to_value(kind).unwrap();
        assert_eq!(wire["type"], kind.name());
        assert!(wire["data"].is_object(), "{wire}");
        assert_eq!(kind.name().split('.').next(), Some(kind.domain()));
        let decoded: EventKind = serde_json::from_value(wire).unwrap();
        assert_eq!(&decoded, kind);
    }
    let ephemeral: Vec<_> = kinds
        .iter()
        .filter(|kind| !kind.is_durable())
        .map(EventKind::name)
        .collect();
    assert_eq!(ephemeral, ["agent.delta", "process.output"]);
    assert_eq!(
        EventKind::AgentSubagentCreated {
            child_session_id: id("ses_child"),
            role: "reviewer".into()
        }
        .domain(),
        "agent"
    );
}

#[test]
fn the_catalog_is_closed_to_unknown_types_and_fields() {
    for wire in [
        json!({"type": "task.exploded", "data": {"task_id": "t"}}),
        json!({"type": "task.started", "data": {"task_id": "t", "token": "x"}}),
        json!({"type": "task.started", "data": {"task_id": "t"}, "extra": 1}),
        json!({"type": "task.started"}),
        json!({"type": "task.started", "data": {"task_id": "has space"}}),
        json!({"type": "task_started", "task_id": "t"}),
    ] {
        assert!(
            serde_json::from_value::<EventKind>(wire.clone()).is_err(),
            "{wire} was accepted"
        );
    }
}

#[test]
fn envelopes_reject_unknown_versions_before_decoding_the_payload() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let envelope = fixture
        .log
        .append(session_scope(), common::task_started("task_1"))
        .unwrap();
    let mut wire = serde_json::to_value(&envelope).unwrap();
    assert_eq!(wire["version"], ENVELOPE_VERSION);
    assert_eq!(wire["cursor"], 1);
    assert_eq!(
        wire["scope"],
        json!({"workspace_id": "ws_main", "session_id": "ses_one"})
    );
    assert_eq!(wire["event"]["type"], "task.started");
    assert_eq!(EventEnvelope::decode(&wire.to_string()).unwrap(), envelope);

    // A future version is rejected as such even though its payload is unknown too.
    wire["version"] = json!(3);
    wire["event"] = json!({"type": "future.thing", "data": {}});
    assert_eq!(
        EventEnvelope::decode(&wire.to_string()),
        Err(EventError::UnsupportedVersion { found: 3 })
    );
    let serde_error = serde_json::from_value::<EventEnvelope>(wire.clone()).unwrap_err();
    assert!(
        serde_error.to_string().contains("version 3"),
        "{serde_error}"
    );
    wire["version"] = json!(1);
    assert_eq!(
        EventEnvelope::decode(&wire.to_string()),
        Err(EventError::UnsupportedVersion { found: 1 })
    );
    wire.as_object_mut().unwrap().remove("version");
    assert!(matches!(
        EventEnvelope::decode(&wire.to_string()),
        Err(EventError::Decode(_))
    ));
    let mut extra = serde_json::to_value(&envelope).unwrap();
    extra["unexpected"] = json!(true);
    assert!(matches!(
        EventEnvelope::decode(&extra.to_string()),
        Err(EventError::Decode(_))
    ));
}

/// A credential-shaped fixture assembled at runtime, so no secret-shaped literal
/// appears in the source (fixtures are fake, but scanners cannot know that).
fn fake(prefix: &str, body: &str) -> String {
    format!("{prefix}{body}")
}

fn secrets() -> [String; 6] {
    let pem = fake("PRIVATE", " KEY");
    [
        fake("sk-", "ant-api03-FAKE0fake1FAKE2fake3FAKE4"),
        fake("ghp_", "GHFAKE0fake1FAKE2fake3FAKE4fake5"),
        fake("AKIA", "FAKEAWSKEY012345"),
        fake("Bearer ", "bearerFAKE0123456789"),
        fake("password=", "hunter2hunter2"),
        format!("-----BEGIN {pem}-----\nFAKEPEMBODY0123\n-----END {pem}-----"),
    ]
}

fn leaks(value: &Value) -> Vec<&'static str> {
    let text = value.to_string();
    [
        "FAKE0fake1FAKE2fake3",
        "GHFAKE0fake1",
        "FAKEAWSKEY",
        "bearerFAKE",
        "hunter2",
        "FAKEPEMBODY",
    ]
    .into_iter()
    .filter(|needle| text.contains(needle))
    .collect()
}

#[test]
fn every_text_bearing_kind_is_redacted_before_storage_and_delivery() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let mut subscription = fixture
        .log
        .subscribe(EventFilter::all(), Cursor::ZERO)
        .unwrap();
    let marker = "TEXTFIELD";
    let mut text_bearing = 0;
    let secrets = secrets();
    for secret in &secrets {
        let text = format!("{marker} {secret} tail");
        for kind in samples(&text) {
            let has_text = serde_json::to_string(&kind).unwrap().contains(marker);
            let returned = fixture.log.append(session_scope(), kind).unwrap();
            let delivered = subscription
                .next_timeout(std::time::Duration::from_secs(5))
                .unwrap()
                .unwrap();
            assert_eq!(delivered, SubscriptionItem::Event(returned.clone()));
            let wire = serde_json::to_value(&returned).unwrap();
            assert!(leaks(&wire).is_empty(), "{} leaked {wire}", returned.name());
            if has_text {
                text_bearing += 1;
                assert!(wire.to_string().contains(redact::REDACTED), "{wire}");
                assert!(wire.to_string().contains(marker), "{wire}");
            }
            // Debug output is redacted as well.
            assert!(!format!("{returned:?}").contains("hunter2"), "{returned:?}");
        }
    }
    assert!(
        text_bearing >= secrets.len() * 30,
        "only {text_bearing} text-bearing events"
    );
    // Everything persisted is redacted too.
    let mut after = Cursor::ZERO;
    loop {
        let page = fixture.log.replay(&EventFilter::all(), after, 256).unwrap();
        for envelope in &page.events {
            assert!(leaks(&serde_json::to_value(envelope).unwrap()).is_empty());
        }
        after = page.next_cursor;
        if !page.has_more {
            break;
        }
    }
}

#[test]
fn debug_output_of_unsanitized_kinds_never_shows_secrets() {
    let kind = EventKind::AgentMessage {
        role: MessageRole::User,
        content: format!("my key is {}", fake("sk-", "proj-FAKE0fake1FAKE2fake3")),
    };
    let debug = format!("{kind:?}");
    assert!(debug.contains("agent.message"), "{debug}");
    assert!(!debug.contains("FAKE0fake1"), "{debug}");
}

#[test]
fn identifiers_that_look_like_credentials_refuse_the_event() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let secret_id = id(&fake("sk-", "proj-FAKE0fake1FAKE2fake3"));
    assert_eq!(
        fixture.log.append(
            session_scope(),
            EventKind::TaskStarted {
                task_id: secret_id.clone()
            }
        ),
        Err(EventError::SensitiveIdentifier)
    );
    assert_eq!(
        fixture.log.append(
            EventScope::session(ws(), secret_id),
            EventKind::SessionStarted {}
        ),
        Err(EventError::SensitiveIdentifier)
    );
    // Nothing was assigned.
    assert_eq!(fixture.log.latest_cursor().unwrap(), Cursor::ZERO);
}

#[test]
fn session_scope_requires_a_workspace_and_oversized_events_are_refused() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let orphan = EventScope {
        workspace_id: None,
        session_id: Some(id("ses_one")),
    };
    assert!(matches!(
        fixture.log.append(orphan, EventKind::SessionStarted {}),
        Err(EventError::InvalidScope(_))
    ));
    let files = (0..20_000)
        .map(|index| aidex_core::WorkspacePath::new(format!("src/module_{index}/file.rs")).unwrap())
        .collect();
    let error = fixture
        .log
        .append(
            EventScope::workspace(ws()),
            EventKind::DiffCreated {
                diff_id: id("diff_big"),
                files,
                additions: 1,
                deletions: 1,
            },
        )
        .unwrap_err();
    assert!(
        matches!(error, EventError::TooLarge { limit, .. } if limit == MAX_EVENT_BYTES),
        "{error:?}"
    );
    assert_eq!(fixture.log.latest_cursor().unwrap(), Cursor::ZERO);
}

#[test]
fn long_text_fields_are_truncated_with_a_marker() {
    let fixture = temp_log(RetentionPolicy::UNBOUNDED);
    let envelope = fixture
        .log
        .append(
            session_scope(),
            EventKind::AgentMessage {
                role: MessageRole::Assistant,
                content: "x".repeat(100_000),
            },
        )
        .unwrap();
    let EventKind::AgentMessage { content, .. } = &envelope.event else {
        unreachable!()
    };
    assert!(content.len() <= redact::MAX_TEXT_BYTES);
    assert!(content.contains(redact::TRUNCATION_MARKER));
}
