//! Approvals end to end: approve once, approve for the session (grants), deny,
//! revoke, expiry, the cancel-vs-approve race, altered intents, single use, and
//! restart behaviour.

mod common;

use std::{
    sync::{Arc, Barrier},
    thread,
};

use aidex_broker::{
    ApprovalRefusal, BrokerError, BrokerOutcome, Principal,
    approvals::{ApprovalFilter, ApprovalResolution, ApprovalState, ApprovalStatus},
};
use aidex_policy::{ApprovalScope, Profile};
use aidex_tools::PermitIssuer;
use common::{Fixture, Options, agent_call, assert_completed, id, pending};
use serde_json::json;

const ASK_WRITES: &str = r#"
schema_version = 1
scope = "global"

[[rules]]
id = "ask-writes"
action = "fs.write"
verdict = "require_approval"
approval_scopes = ["once", "session"]
expires_in_secs = 300
"#;

fn ask_writes() -> Fixture {
    Fixture::new(Options::default().document("10-ask.toml", ASK_WRITES))
}

fn refusal(result: Result<impl std::fmt::Debug, BrokerError>) -> ApprovalRefusal {
    match result {
        Err(BrokerError::Approval { refusal, .. }) => refusal,
        other => panic!("expected an approval refusal, got {other:?}"),
    }
}

fn write(fixture: &Fixture, path: &str, content: &str) -> BrokerOutcome {
    fixture.submit("fs.write", json!({"path": path, "content": content}))
}

#[test]
fn approve_once_runs_exactly_once() {
    let fixture = Fixture::new(Options::profile(Profile::Safe));
    fixture.write("notes.txt", "old\n");
    let outcome = fixture.submit("fs.delete", json!({"path": "notes.txt"}));
    let BrokerOutcome::PendingApproval {
        approval_id, card, ..
    } = &outcome
    else {
        panic!("expected approval, got {outcome:?}");
    };
    assert_eq!(
        card.scope_options,
        [ApprovalScope::Once, ApprovalScope::Session]
    );
    assert!(
        card.effect
            .starts_with("Deletes notes.txt in workspace demo"),
        "{}",
        card.effect
    );
    assert!(card.diff_preview.is_some());
    assert_eq!(card.requester.agent_id, Some(id(common::AGENT)));
    assert_eq!(
        refusal(fixture.broker.continue_after_approval(approval_id)),
        ApprovalRefusal::Pending
    );
    let record = fixture
        .broker
        .resolve_approval(
            approval_id,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser,
        )
        .unwrap();
    assert!(matches!(
        record.state,
        ApprovalState::Approved {
            scope: ApprovalScope::Once,
            ..
        }
    ));
    assert!(
        fixture.path("notes.txt").exists(),
        "approval alone runs nothing"
    );
    let outcome = fixture.broker.continue_after_approval(approval_id).unwrap();
    let BrokerOutcome::Completed { permit, .. } = &outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert_eq!(
        permit.issuer,
        PermitIssuer::Approval {
            approval_id: approval_id.clone()
        }
    );
    assert!(!fixture.path("notes.txt").exists());
    assert_eq!(
        refusal(fixture.broker.continue_after_approval(approval_id)),
        ApprovalRefusal::AlreadyConsumed
    );
    let names = fixture.names();
    for expected in ["approval.created", "approval.approved", "tool.completed"] {
        assert!(names.contains(&expected), "{names:?}");
    }
    assert!(
        fixture
            .broker
            .approval(approval_id)
            .unwrap()
            .consumed_at
            .is_some()
    );
}

#[test]
fn approve_session_grants_identical_calls_in_that_session_only() {
    let fixture = ask_writes();
    let first = pending(&write(&fixture, "a.txt", "one\n"));
    fixture
        .broker
        .resolve_approval(
            &first,
            ApprovalResolution::ApproveSession,
            &Principal::LocalUser,
        )
        .unwrap();
    assert_completed(&fixture.broker.continue_after_approval(&first).unwrap());
    // Same tool, action, and target (new content): covered by the grant.
    let outcome = write(&fixture, "a.txt", "two\n");
    let BrokerOutcome::Completed { permit, .. } = &outcome else {
        panic!("expected the grant to apply, got {outcome:?}");
    };
    assert_eq!(
        permit.issuer,
        PermitIssuer::Approval {
            approval_id: first.clone()
        }
    );
    assert_eq!(
        std::fs::read_to_string(fixture.path("a.txt")).unwrap(),
        "two\n"
    );
    assert!(
        fixture
            .decision_rules()
            .contains(&format!("session_grant:{first}"))
    );
    // A different target needs its own approval.
    pending(&write(&fixture, "b.txt", "x\n"));
    // Grants never cross sessions.
    let mut other = agent_call("fs.write", json!({"path": "a.txt", "content": "three\n"}));
    other.session_id = id("ses_other");
    pending(&fixture.broker.submit(other).unwrap());
    // Revoking the grant restores the approval requirement.
    let record = fixture
        .broker
        .revoke_approval(&first, &Principal::LocalUser, None)
        .unwrap();
    assert_eq!(record.status(), ApprovalStatus::Revoked);
    assert!(!record.grant.unwrap().active);
    pending(&write(&fixture, "a.txt", "four\n"));
    assert_eq!(
        std::fs::read_to_string(fixture.path("a.txt")).unwrap(),
        "two\n"
    );
}

#[test]
fn deny_and_revoke_pending_requests() {
    let fixture = ask_writes();
    let denied = pending(&write(&fixture, "a.txt", "x"));
    let record = fixture
        .broker
        .resolve_approval(&denied, ApprovalResolution::Deny, &Principal::LocalUser)
        .unwrap();
    assert!(matches!(
        record.state,
        ApprovalState::Denied {
            by: Principal::LocalUser,
            ..
        }
    ));
    assert_eq!(
        refusal(fixture.broker.continue_after_approval(&denied)),
        ApprovalRefusal::Denied
    );
    assert_eq!(
        refusal(fixture.broker.resolve_approval(
            &denied,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser
        )),
        ApprovalRefusal::Denied
    );

    let revoked = pending(&write(&fixture, "a.txt", "y"));
    fixture
        .broker
        .revoke_approval(&revoked, &Principal::LocalUser, Some("changed my mind"))
        .unwrap();
    assert_eq!(
        refusal(fixture.broker.resolve_approval(
            &revoked,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser
        )),
        ApprovalRefusal::Revoked
    );
    assert_eq!(
        refusal(fixture.broker.continue_after_approval(&revoked)),
        ApprovalRefusal::Revoked
    );
    assert!(!fixture.path("a.txt").exists());
    let names = fixture.names();
    assert!(names.contains(&"approval.denied") && names.contains(&"approval.revoked"));
    let pending_now = fixture
        .broker
        .list_approvals(&ApprovalFilter::pending())
        .unwrap();
    assert!(pending_now.is_empty());
}

#[test]
fn approvals_expire_and_cannot_be_approved_afterwards() {
    let fixture = ask_writes();
    let first = pending(&write(&fixture, "a.txt", "x"));
    let second = pending(&write(&fixture, "b.txt", "x"));
    fixture.clock.advance_millis(301_000);
    // Resolving after the deadline expires the request instead.
    assert_eq!(
        refusal(fixture.broker.resolve_approval(
            &first,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser
        )),
        ApprovalRefusal::Expired
    );
    assert!(
        fixture.broker.expire_due().unwrap().is_empty(),
        "already expired above"
    );
    for id in [&first, &second] {
        assert_eq!(
            fixture.broker.approval(id).unwrap().status(),
            ApprovalStatus::Expired
        );
        assert_eq!(
            refusal(fixture.broker.continue_after_approval(id)),
            ApprovalRefusal::Expired
        );
    }
    assert_eq!(
        fixture
            .names()
            .iter()
            .filter(|name| **name == "approval.expired")
            .count(),
        2
    );
}

#[test]
fn expire_due_expires_only_overdue_requests() {
    let fixture = ask_writes();
    let first = pending(&write(&fixture, "a.txt", "x"));
    fixture.clock.advance_millis(200_000);
    let second = pending(&write(&fixture, "b.txt", "x"));
    fixture.clock.advance_millis(101_000);
    let expired = fixture.broker.expire_due().unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].approval_id, first);
    assert_eq!(
        fixture.broker.approval(&second).unwrap().status(),
        ApprovalStatus::Pending
    );
}

#[test]
fn cancel_and_approve_races_have_exactly_one_winner() {
    let fixture = Arc::new(ask_writes());
    for round in 0..20 {
        let approval = pending(&write(&fixture, &format!("f{round}.txt"), "x"));
        let barrier = Arc::new(Barrier::new(2));
        let approver = {
            let (fixture, barrier, approval) = (fixture.clone(), barrier.clone(), approval.clone());
            thread::spawn(move || {
                barrier.wait();
                fixture
                    .broker
                    .resolve_approval(
                        &approval,
                        ApprovalResolution::ApproveOnce,
                        &Principal::LocalUser,
                    )
                    .is_ok()
            })
        };
        let canceller = {
            let (fixture, barrier, approval) = (fixture.clone(), barrier.clone(), approval.clone());
            thread::spawn(move || {
                barrier.wait();
                fixture
                    .broker
                    .cancel_approval(&approval, "requester gave up")
                    .is_ok()
            })
        };
        let approved = approver.join().unwrap();
        let cancelled = canceller.join().unwrap();
        assert!(
            approved ^ cancelled,
            "round {round}: approved={approved} cancelled={cancelled}"
        );
        let status = fixture.broker.approval(&approval).unwrap().status();
        assert_eq!(
            status,
            if approved {
                ApprovalStatus::Approved
            } else {
                ApprovalStatus::Cancelled
            }
        );
        let terminal: Vec<String> = fixture
            .events
            .events()
            .iter()
            .map(|envelope| serde_json::to_value(&envelope.event).unwrap())
            .filter(|wire| wire["data"]["approval_id"] == approval.as_str())
            .map(|wire| wire["type"].as_str().unwrap().to_owned())
            .filter(|name| name != "approval.created")
            .collect();
        assert_eq!(terminal.len(), 1, "round {round}: {terminal:?}");
    }
}

#[test]
fn altered_intents_are_rejected_after_approval() {
    let fixture = ask_writes();
    fixture.write("a.txt", "original\n");
    let approval = pending(&write(&fixture, "a.txt", "approved content\n"));
    fixture
        .broker
        .resolve_approval(
            &approval,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser,
        )
        .unwrap();
    // The target changed after the human saw the diff.
    fixture.write("a.txt", "someone else edited this\n");
    assert_eq!(
        refusal(fixture.broker.continue_after_approval(&approval)),
        ApprovalRefusal::IntentChanged
    );
    assert_eq!(
        std::fs::read_to_string(fixture.path("a.txt")).unwrap(),
        "someone else edited this\n"
    );
    // Restoring the approved state makes the bound intent match again.
    fixture.write("a.txt", "original\n");
    assert_completed(&fixture.broker.continue_after_approval(&approval).unwrap());
    assert_eq!(
        std::fs::read_to_string(fixture.path("a.txt")).unwrap(),
        "approved content\n"
    );
}

#[test]
fn session_scope_is_refused_when_not_offered() {
    let fixture = Fixture::new(Options::default());
    let approval = pending(&write(&fixture, ".env", "A=1\n"));
    assert_eq!(
        refusal(fixture.broker.resolve_approval(
            &approval,
            ApprovalResolution::ApproveSession,
            &Principal::LocalUser
        )),
        ApprovalRefusal::ScopeNotOffered
    );
    assert_eq!(
        fixture.broker.approval(&approval).unwrap().status(),
        ApprovalStatus::Pending
    );
}

#[test]
fn pending_approvals_do_not_survive_a_restart() {
    let fixture = ask_writes();
    let waiting = pending(&write(&fixture, "a.txt", "x"));
    let approved = pending(&write(&fixture, "b.txt", "x"));
    fixture
        .broker
        .resolve_approval(
            &approved,
            ApprovalResolution::ApproveOnce,
            &Principal::LocalUser,
        )
        .unwrap();
    let Fixture {
        state, workspace, ..
    } = fixture;
    let restarted = Fixture::open(
        state,
        workspace,
        Options::default().document("10-ask.toml", ASK_WRITES),
    );
    assert_eq!(
        restarted.broker.approval(&waiting).unwrap().status(),
        ApprovalStatus::Cancelled
    );
    assert_eq!(restarted.names(), ["approval.cancelled"]);
    // The approved invocation lived in the old broker's memory only.
    assert_eq!(
        refusal(restarted.broker.continue_after_approval(&approved)),
        ApprovalRefusal::InvocationLost
    );
    assert!(!restarted.path("b.txt").exists());
}
