//! The broker over authenticated IPC: tool invocation, approval list/show/resolve/
//! revoke/continue, policy show/profile/explain, session cancellation, emergency
//! stop and resume, and the audit trail they leave.

mod common;

use aidex_sessions::Session;
use aidex_workspace::Workspace;
use aidexd::{
    Client, ClientError, ErrorCode,
    methods::{
        approvals, emergency,
        events::{REPLAY, ReplayPage},
        policy, sessions, tools,
        workspace::CREATE_DRIVE,
    },
};
use common::{options, start};
use serde_json::{Value, json};
use std::path::PathBuf;

struct Setup {
    client: Client,
    workspace: Workspace,
    session: Session,
}

impl Setup {
    fn file(&self, name: &str) -> PathBuf {
        PathBuf::from(&self.workspace.path).join(name)
    }

    fn invoke(&mut self, tool: &str, input: Value) -> Value {
        self.client
            .call(
                tools::INVOKE,
                json!({"session": self.session.id.as_str(), "tool": tool, "input": input}),
            )
            .unwrap()
    }

    /// Request deleting a fresh file under the Safe profile (needs approval).
    fn pending_delete(&mut self, name: &str) -> String {
        std::fs::write(self.file(name), "x\n").unwrap();
        let outcome = self.invoke("fs.delete", json!({"path": name}));
        assert_eq!(outcome["outcome"], "pending_approval", "{outcome}");
        outcome["approval_id"].as_str().unwrap().to_owned()
    }

    fn resolve(&mut self, id: &str, decision: &str) -> Value {
        self.client
            .call(approvals::RESOLVE, json!({"id": id, "decision": decision}))
            .unwrap()
    }
}

fn setup(client: Client) -> Setup {
    let mut client = client;
    let workspace: Workspace = client
        .call_as(CREATE_DRIVE, json!({"name": "project"}))
        .unwrap();
    let session: Session = client
        .call_as(
            sessions::CREATE,
            json!({"workspace_id": workspace.id.as_str(), "agent_id": "aidex", "title": "broker"}),
        )
        .unwrap();
    Setup {
        client,
        workspace,
        session,
    }
}

fn reason(error: &ClientError) -> Option<String> {
    match error {
        ClientError::Rpc {
            data: Some(data), ..
        } => data["reason"].as_str().map(str::to_owned),
        _ => None,
    }
}

#[test]
fn tools_and_approvals_are_served_through_the_broker() {
    let root = common::temp_drive();
    let _daemon = start(root.path(), options(), |_| {});
    let mut setup = setup(Client::connect(root.path(), "test-broker").unwrap());

    let listed = setup.client.call(tools::LIST, json!({})).unwrap();
    let push = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["id"] == "git.push")
        .expect("git.push is registered");
    assert_eq!(push["risk"]["action"], "git.push");
    assert_eq!(push["risk"]["remote_write"], true);

    let status = setup.client.call(policy::SHOW, json!({})).unwrap();
    assert_eq!(status["settings"]["profile"], "balanced");
    let status = setup
        .client
        .call(policy::PROFILE_SET, json!({"profile": "safe"}))
        .unwrap();
    assert_eq!(status["settings"]["profile"], "safe");
    assert!(
        setup
            .client
            .call(policy::PROFILE_SET, json!({"profile": "reckless"}))
            .is_err()
    );

    // Approve once, then continue: the call runs exactly once.
    let approval = setup.pending_delete("a.txt");
    let pending: approvals::ApprovalList = setup
        .client
        .call_as(approvals::LIST, json!({"state": "pending"}))
        .unwrap();
    assert_eq!(pending.approvals.len(), 1);
    let shown: approvals::ApprovalRecord = setup
        .client
        .call_as(approvals::SHOW, json!({"id": approval}))
        .unwrap();
    assert!(
        shown.card.effect.starts_with("Deletes a.txt"),
        "{}",
        shown.card.effect
    );
    let approved = setup.resolve(&approval, "approve_once");
    assert_eq!(approved["state"]["status"], "approved");
    assert!(setup.file("a.txt").exists());
    let outcome = setup
        .client
        .call(tools::CONTINUE, json!({"approval_id": approval}))
        .unwrap();
    assert_eq!(outcome["outcome"], "completed", "{outcome}");
    assert!(!setup.file("a.txt").exists());
    let again = setup
        .client
        .call(tools::CONTINUE, json!({"approval_id": approval}))
        .unwrap_err();
    assert_eq!(again.rpc_kind(), Some(ErrorCode::Conflict));
    assert_eq!(reason(&again).as_deref(), Some("already_consumed"));

    // Deny: nothing runs.
    let denied = setup.pending_delete("b.txt");
    setup.resolve(&denied, "deny");
    let error = setup
        .client
        .call(tools::CONTINUE, json!({"approval_id": denied}))
        .unwrap_err();
    assert_eq!(reason(&error).as_deref(), Some("approval_denied"));
    assert!(setup.file("b.txt").exists());

    // Revoke a pending request.
    let revoked = setup.pending_delete("c.txt");
    let record = setup
        .client
        .call(approvals::REVOKE, json!({"id": revoked}))
        .unwrap();
    assert_eq!(record["state"]["status"], "revoked");

    // Dry run: no approval or execution.
    let explanation = setup
        .client
        .call(
            policy::EXPLAIN,
            json!({"tool": "fs.delete", "input": {"path": "c.txt"}, "workspace": setup.workspace.id.as_str()}),
        )
        .unwrap();
    assert_eq!(
        explanation["decision"]["verdict"]["kind"],
        "require_approval"
    );
    assert!(setup.file("c.txt").exists());

    // Cancelling the session cancels its pending approvals.
    let waiting = setup.pending_delete("d.txt");
    setup
        .client
        .call(sessions::CANCEL, json!({"id": setup.session.id.as_str()}))
        .unwrap();
    let record = setup
        .client
        .call(approvals::SHOW, json!({"id": waiting}))
        .unwrap();
    assert_eq!(record["state"]["status"], "cancelled");

    let page: ReplayPage = setup
        .client
        .call_as(REPLAY, json!({"after": 0, "limit": 256}))
        .unwrap();
    let names: Vec<&str> = page
        .events
        .iter()
        .map(|envelope| envelope.event.name())
        .collect();
    for expected in [
        "policy.changed",
        "tool.requested",
        "policy.decision",
        "approval.created",
        "approval.approved",
        "tool.completed",
        "approval.denied",
        "approval.revoked",
        "approval.cancelled",
    ] {
        assert!(names.contains(&expected), "{expected} missing: {names:?}");
    }
}

#[test]
fn emergency_stop_freezes_tools_until_resumed() {
    let root = common::temp_drive();
    let _daemon = start(root.path(), options(), |_| {});
    let mut setup = setup(Client::connect(root.path(), "test-emergency").unwrap());
    setup
        .client
        .call(policy::PROFILE_SET, json!({"profile": "safe"}))
        .unwrap();
    let waiting = setup.pending_delete("a.txt");

    let report: emergency::EmergencyReport = setup
        .client
        .call_as(
            emergency::STOP,
            json!({"reason": "operator drill", "lock_vault": true}),
        )
        .unwrap();
    assert_eq!(report.cancelled_approvals.len(), 1);
    assert_eq!(report.cancelled_approvals[0].as_str(), waiting);
    assert!(report.vault_locked);
    let error = setup
        .client
        .call(
            tools::INVOKE,
            json!({"session": setup.session.id.as_str(), "tool": "fs.read", "input": {"path": "a.txt"}}),
        )
        .unwrap_err();
    assert_eq!(error.rpc_kind(), Some(ErrorCode::PolicyDenied));
    assert_eq!(reason(&error).as_deref(), Some("emergency_stop"));
    let status = setup.client.call(policy::SHOW, json!({})).unwrap();
    assert_eq!(status["emergency_stop"], "operator drill");

    let resumed: emergency::ResumeResult =
        setup.client.call_as(emergency::RESUME, json!({})).unwrap();
    assert!(resumed.resumed);
    let outcome = setup.invoke("fs.read", json!({"path": "a.txt"}));
    assert_eq!(outcome["outcome"], "completed", "{outcome}");
}
