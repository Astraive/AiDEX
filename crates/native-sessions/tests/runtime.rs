//! The external runtime contract: resumable references and the native identity event.
use aidex_native_sessions::{RuntimeEvent, is_resumable_reference};
use serde_json::json;

#[test]
fn only_references_bound_to_a_native_branch_and_snapshot_are_resumable() {
    // The shape main saved before native continuation.
    let legacy = json!({
        "runtime_id": "codex",
        "native_session_id": "thread-1",
        "native_turn_id": "turn-1",
        "snapshot_id": "runtime-0a1b",
        "run_id": "run_1",
        "session_id": "session_1"
    });
    assert!(!is_resumable_reference(&legacy));
    let mut bound = legacy.clone();
    bound["native_branch_id"] = json!("branch-root");
    bound["snapshot_digest"] = json!("5f0e");
    assert!(is_resumable_reference(&bound));
    for (key, value) in [
        ("native_branch_id", json!("")),
        ("native_branch_id", json!(7)),
        ("snapshot_digest", json!("  ")),
        ("snapshot_digest", json!(null)),
    ] {
        let mut broken = bound.clone();
        broken[key] = value;
        assert!(!is_resumable_reference(&broken), "{broken}");
    }
    for key in ["native_branch_id", "snapshot_digest"] {
        let mut missing = bound.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(!is_resumable_reference(&missing), "{missing}");
    }
    for other in [json!(null), json!("branch-root"), json!([bound])] {
        assert!(!is_resumable_reference(&other), "{other}");
    }
}

#[test]
fn session_started_uses_the_tagged_runtime_event_shape() {
    let event = RuntimeEvent::SessionStarted {
        runtime_ref: json!({"native_branch_id": "branch-root"}),
    };
    let wire = serde_json::to_value(&event).unwrap();
    assert_eq!(
        wire,
        json!({"type": "session_started", "runtime_ref": {"native_branch_id": "branch-root"}})
    );
    let decoded: RuntimeEvent = serde_json::from_value(wire).unwrap();
    assert!(matches!(
        decoded,
        RuntimeEvent::SessionStarted { runtime_ref } if runtime_ref["native_branch_id"] == "branch-root"
    ));
}
