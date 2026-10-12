//! Emergency stop: kills running sandboxed children, cancels pending approvals,
//! revokes unused approvals, freezes new execution, and resumes on request. Also:
//! the broker's launcher refuses plans it did not permit.

mod common;

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use aidex_broker::{
    ApprovalRefusal, BrokerError, BrokerOutcome, Principal,
    approvals::{ApprovalResolution, ApprovalStatus},
};
use aidex_events::NullEventSink;
use aidex_tools::{ExecutionPermit, PermitIssuer, ToolContext, ToolError, ToolId, ToolRegistry};
use aidex_workspace::ConfinedRoot;
use common::{ALLOW_WAITING, Fixture, Options, SESSION, WORKSPACE, id, long_running, pending};
use serde_json::json;

const ASK_DELETES: &str = r#"
schema_version = 1
scope = "global"

[[rules]]
id = "ask-deletes"
action = "fs.delete"
verdict = "require_approval"
"#;

#[test]
fn emergency_stop_kills_children_cancels_approvals_and_freezes_execution() {
    let fixture = Arc::new(Fixture::new(
        Options::default()
            .document("10-allow.toml", ALLOW_WAITING)
            .document("20-ask.toml", ASK_DELETES),
    ));
    fixture.write("a.txt", "x\n");
    fixture.write("b.txt", "x\n");
    let waiting = pending(&fixture.submit("fs.delete", json!({"path": "a.txt"})));
    let approved = pending(&fixture.submit("fs.delete", json!({"path": "b.txt"})));
    fixture
        .broker
        .resolve_approval(
            &approved,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser,
        )
        .unwrap();

    let (program, args) = long_running(60);
    let runner = {
        let fixture = fixture.clone();
        thread::spawn(move || {
            fixture.submit("shell.exec", json!({"program": program, "args": args}))
        })
    };
    let started = Instant::now();
    while fixture.broker.sandbox().registry().is_empty() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the child never started"
        );
        thread::sleep(Duration::from_millis(20));
    }

    let report = fixture
        .broker
        .emergency_stop("operator pressed stop", &Principal::LocalUser)
        .unwrap();
    assert_eq!(report.terminated_processes, 1, "{report:?}");
    assert_eq!(report.cancelled_calls, 1);
    assert_eq!(report.cancelled_approvals, std::slice::from_ref(&waiting));
    assert_eq!(report.revoked_approvals, std::slice::from_ref(&approved));
    let outcome = runner.join().unwrap();
    assert!(
        matches!(outcome, BrokerOutcome::Failed { .. }),
        "the killed call must not complete: {outcome:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(45));
    assert!(fixture.broker.sandbox().registry().is_empty());

    assert_eq!(
        fixture.broker.approval(&waiting).unwrap().status(),
        ApprovalStatus::Cancelled
    );
    assert_eq!(
        fixture.broker.approval(&approved).unwrap().status(),
        ApprovalStatus::Revoked
    );
    assert!(matches!(
        fixture
            .broker
            .submit(common::agent_call("fs.read", json!({"path": "a.txt"}))),
        Err(BrokerError::Stopped { .. })
    ));
    assert!(matches!(
        fixture.broker.continue_after_approval(&approved),
        Err(BrokerError::Stopped { .. })
    ));
    let names = fixture.names();
    for expected in [
        "approval.cancelled",
        "approval.revoked",
        "emergency.stopped",
    ] {
        assert!(names.contains(&expected), "{names:?}");
    }
    assert!(fixture.path("a.txt").exists() && fixture.path("b.txt").exists());

    assert!(
        fixture
            .broker
            .resume_operations(&Principal::LocalUser)
            .unwrap()
    );
    assert!(
        !fixture
            .broker
            .resume_operations(&Principal::LocalUser)
            .unwrap()
    );
    assert!(fixture.names().contains(&"emergency.resumed"));
    assert!(matches!(
        fixture.broker.continue_after_approval(&approved),
        Err(BrokerError::Approval {
            refusal: ApprovalRefusal::Revoked,
            ..
        })
    ));
    let (program, args) = long_running(0);
    let outcome = fixture.submit("shell.exec", json!({"program": program, "args": args}));
    assert!(
        matches!(outcome, BrokerOutcome::Completed { .. }),
        "{outcome:?}"
    );
}

#[test]
fn remote_devices_cannot_resume_or_change_policy() {
    let fixture = Fixture::new(Options::default());
    let device = Principal::RemoteDevice {
        device_id: id("dev_phone"),
    };
    fixture
        .broker
        .emergency_stop("drill", &Principal::LocalUser)
        .unwrap();
    assert!(matches!(
        fixture.broker.resume_operations(&device),
        Err(BrokerError::Unauthorized(_))
    ));
    assert!(matches!(
        fixture.broker.change_policy(
            aidex_broker::PolicyChange::Profile(aidex_policy::Profile::Autonomous),
            &device
        ),
        Err(BrokerError::Unauthorized(_))
    ));
    let status = fixture
        .broker
        .change_policy(
            aidex_broker::PolicyChange::Profile(aidex_policy::Profile::Safe),
            &Principal::LocalUser,
        )
        .unwrap();
    assert_eq!(status.settings.profile, aidex_policy::Profile::Safe);
    assert!(fixture.names().contains(&"policy.changed"));
}

#[test]
fn the_launcher_refuses_plans_the_broker_did_not_permit() {
    let fixture = Fixture::new(Options::default().document("10-allow.toml", ALLOW_WAITING));
    // A registry outside the broker, wired to the broker's launcher: even with a
    // self-made permit, the launch is refused because the broker never issued one.
    let registry = ToolRegistry::with_builtin_tools();
    let ctx = ToolContext::new(
        id(SESSION),
        id(WORKSPACE),
        id("call_rogue"),
        Arc::new(ConfinedRoot::new(fixture.root()).unwrap()),
        Arc::new(NullEventSink),
    )
    .with_launcher(fixture.broker.launcher());
    let (program, args) = long_running(0);
    let tool = ToolId::new("shell.exec").unwrap();
    let plan = registry
        .plan(&ctx, &tool, json!({"program": program, "args": args}))
        .unwrap();
    let permit = ExecutionPermit::for_plan(&plan, PermitIssuer::Development);
    let error = registry.execute(&ctx, &plan, permit).unwrap_err();
    assert!(matches!(error, ToolError::Launch(_)), "{error:?}");
    assert!(fixture.broker.sandbox().registry().is_empty());
}
