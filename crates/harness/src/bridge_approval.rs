//! Human decisions for an in-flight SDK MCP call; never start a second SDK actor.
use crate::{NativeEngine, broker::Prepared};
use aidex_core::{CancelToken, EntityId, Timestamp};
use aidex_events::{ApprovalScope, EventKind};
use aidex_native_sessions::{Approval, Run, RunStatus};
use aidex_providers::ToolCall;
use aidex_runtime_bridge::BridgeError;
use aidex_tools::PermitIssuer;
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant, SystemTime},
};

#[cfg(not(test))]
const TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(test)]
const TIMEOUT: Duration = Duration::from_millis(500);
struct Waiting {
    run_id: String,
    deadline: Instant,
    decision: Mutex<Decision>,
    changed: Condvar,
}
enum Decision {
    Pending,
    Decided(bool),
    Closed(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidex_events::{Cursor, EventError, EventScope, EventSink, MemoryEventSink};
    use aidex_native_sessions::SessionStore;
    use aidex_providers::{NoVault, OfflineMode, ProviderRegistry};
    use aidex_vault::{KdfParams, SecretString, VaultService, VaultServiceConfig};
    use serde_json::json;

    /// Hold only notification delivery so the real approval/store remains usable.
    #[derive(Default)]
    struct HeldCreated {
        events: MemoryEventSink,
        state: Mutex<(bool, bool)>,
        changed: Condvar,
    }
    impl EventSink for HeldCreated {
        fn emit(&self, scope: EventScope, event: EventKind) -> Result<Cursor, EventError> {
            let created = matches!(event, EventKind::ApprovalCreated { .. });
            let cursor = self.events.emit(scope, event)?;
            if created {
                let mut state = self.state.lock().unwrap();
                state.0 = true;
                self.changed.notify_all();
                while !state.1 {
                    state = self.changed.wait(state).unwrap();
                }
            }
            Ok(cursor)
        }
    }
    impl HeldCreated {
        fn wait(&self) {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut state = self.state.lock().unwrap();
            while !state.0 {
                assert!(Instant::now() < deadline, "approval was not published");
                state = self
                    .changed
                    .wait_timeout(state, Duration::from_millis(10))
                    .unwrap()
                    .0;
            }
        }
        fn release(&self) {
            self.state.lock().unwrap().1 = true;
            self.changed.notify_all();
        }
    }
    struct Fixture {
        _drive: tempfile::TempDir,
        workspace: tempfile::TempDir,
        engine: Arc<NativeEngine>,
        events: Arc<HeldCreated>,
        run: Run,
        worker: Option<std::thread::JoinHandle<Result<(), BridgeError>>>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.events.release();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    impl Fixture {
        fn approval(&self) -> Approval {
            self.events.wait();
            self.engine.approvals(Some(&self.run.id)).unwrap().remove(0)
        }
        fn finish(&mut self) -> Result<(), BridgeError> {
            self.events.release();
            self.worker.take().unwrap().join().unwrap()
        }
    }
    fn fixture() -> Fixture {
        let drive = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        aidex_drive::init_with(
            drive.path(),
            aidex_drive::InitOptions {
                require_removable: false,
            },
        )
        .unwrap();
        let paths = aidex_drive::DrivePaths::open(drive.path()).unwrap();
        let vault = Arc::new(
            VaultService::open(
                paths.vault(),
                VaultServiceConfig {
                    kdf: KdfParams::new(8, 1, 1).unwrap(),
                    idle_timeout: None,
                },
            )
            .unwrap(),
        );
        vault
            .init(&SecretString::from("fixture passphrase".to_owned()), false)
            .unwrap();
        let store =
            Arc::new(SessionStore::open(paths.sessions().join("native.sqlite3"), vault).unwrap());
        let workspaces = Arc::new(aidex_workspace::WorkspaceRegistry::open(&paths).unwrap());
        let selected = workspaces.add_host(workspace.path(), None).unwrap();
        let registry = Arc::new(
            ProviderRegistry::load(
                &paths,
                Arc::new(NoVault::default()),
                Arc::new(OfflineMode::new(false)),
            )
            .unwrap(),
        );
        let events = Arc::new(HeldCreated::default());
        let engine = NativeEngine::new(
            store.clone(),
            registry,
            workspaces,
            events.clone(),
            drive.path().join("scratch"),
        );
        let session = store.create(serde_json::from_value(json!({"workspace_id":selected.id,"mode":"agent","provider_id":"fixture","model_id":"fixture"})).unwrap()).unwrap();
        let run = store
            .begin(&session.id, "write the file", "once", None)
            .unwrap();
        let run = store
            .update(&session.id, |session| {
                let active = session.runs.iter_mut().find(|r| r.id == run.id).unwrap();
                active.status = RunStatus::Running;
                Ok(active.clone())
            })
            .unwrap();
        let call = ToolCall {
            id: "bridge_fixture".into(),
            name: "fs_write".into(),
            arguments: json!({"path":"result.txt","content":"must not execute after expiry"})
                .to_string(),
            provider_metadata: None,
        };
        let prepared = engine
            .broker
            .prepare(&store.get(&session.id).unwrap(), &call)
            .unwrap_or_else(|e| panic!("{e}"));
        let actor = engine.clone();
        let active = run.clone();
        let worker = std::thread::spawn(move || {
            let permit = actor.bridge_approvals.request(
                &actor,
                &active,
                &call,
                &prepared,
                &CancelToken::new(),
            )?;
            actor
                .broker
                .execute(&prepared, permit)
                .map_err(|_| problem("tool_failed"))?;
            Ok(())
        });
        Fixture {
            _drive: drive,
            workspace,
            engine,
            events,
            run,
            worker: Some(worker),
        }
    }

    #[test]
    fn expired_response_is_rejected_before_a_permit_or_file_effect() {
        let mut fixture = fixture();
        let approval = fixture.approval();
        std::thread::sleep(TIMEOUT + Duration::from_millis(20));
        let response = fixture
            .engine
            .respond(&approval.id, &approval.fingerprint, true, "late");
        let outcome = fixture.finish();
        assert!(response.is_err(), "an expired decision was accepted");
        assert_eq!(outcome.unwrap_err().code, "approval_expired");
        assert!(!fixture.workspace.path().join("result.txt").exists());
        assert_eq!(
            fixture
                .events
                .events
                .names()
                .iter()
                .filter(|name| **name == "approval.expired")
                .count(),
            1
        );
        assert!(!fixture.events.events.names().contains(&"approval.approved"));
        assert_eq!(
            fixture.engine.approvals(Some(&fixture.run.id)).unwrap()[0].status,
            "expired"
        );
        assert!(
            !fixture
                .engine
                .bridge_approvals
                .pending_for_run(&fixture.run.id)
        );
    }

    #[test]
    fn a_delayed_waiter_cannot_consume_an_approval_after_expiry() {
        let mut fixture = fixture();
        let approval = fixture.approval();
        fixture
            .engine
            .respond(&approval.id, &approval.fingerprint, true, "before-expiry")
            .unwrap();
        std::thread::sleep(TIMEOUT + Duration::from_millis(20));
        let outcome = fixture.finish();
        assert_eq!(outcome.unwrap_err().code, "approval_expired");
        assert!(!fixture.workspace.path().join("result.txt").exists());
        assert_eq!(
            fixture
                .events
                .events
                .names()
                .iter()
                .filter(|name| **name == "approval.expired")
                .count(),
            1
        );
        assert_eq!(
            fixture.engine.approvals(Some(&fixture.run.id)).unwrap()[0].status,
            "expired"
        );
        assert!(
            fixture
                .engine
                .respond(&approval.id, &approval.fingerprint, true, "replay")
                .is_err()
        );
    }
}
#[derive(Default)]
pub(crate) struct BridgeApprovals {
    pending: Mutex<HashMap<String, Arc<Waiting>>>,
}
fn problem(code: &'static str) -> BridgeError {
    BridgeError::new(
        code,
        match code {
            "approval_denied" => "User denied this extension tool; no effect was started",
            "approval_expired" => "Approval expired; request a fresh tool call",
            "approval_pending" => "This run is already waiting for an extension approval",
            "cancelled" => "Run cancelled before extension execution",
            _ => "Extension approval is unavailable; no effect was started",
        },
    )
}
/// Called while holding the waiter decision lock, before removing its entry.
fn expire_record(
    engine: &NativeEngine,
    run: &Run,
    id: &str,
    retract_unconsumed_approval: bool,
) -> Result<Run, String> {
    let (active, changed) = engine.store.update(&run.session_id, |session| {
        let mut changed = false;
        if let Some(approval) = session.approvals.iter_mut().find(|a| a.id == id)
            && (approval.status == "pending"
                || (retract_unconsumed_approval && approval.status == "approved"))
        {
            approval.status = "expired".into();
            changed = true;
        }
        let pending = session
            .approvals
            .iter()
            .any(|a| a.run_id == run.id && a.status == "pending");
        let active = session
            .runs
            .iter_mut()
            .find(|r| r.id == run.id)
            .ok_or("run not found")?;
        if !pending && active.status == RunStatus::WaitingApproval {
            active.status = RunStatus::Running;
            active.updated_at = aidex_native_sessions::now();
        }
        Ok((active.clone(), changed))
    })?;
    if changed {
        emit_expiry(engine, &active, id);
    }
    Ok(active)
}
fn emit_expiry(engine: &NativeEngine, run: &Run, id: &str) {
    if let Ok(approval_id) = EntityId::new(id) {
        engine.emit(&run.session_id, EventKind::ApprovalExpired { approval_id });
    }
}
impl BridgeApprovals {
    pub(crate) fn pending_for_run(&self, run: &str) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|w| w.run_id == run)
    }
    pub(crate) fn request(
        &self,
        engine: &NativeEngine,
        run: &Run,
        call: &ToolCall,
        prepared: &Prepared,
        cancel: &CancelToken,
    ) -> Result<PermitIssuer, BridgeError> {
        if cancel.is_stopped() {
            return Err(problem("cancelled"));
        }
        let deadline = Instant::now() + TIMEOUT;
        let expires_at = SystemTime::now() + TIMEOUT;
        let id =
            aidex_native_sessions::id("approval").map_err(|_| problem("approval_unavailable"))?;
        let approval = Approval {
            id: id.clone(),
            run_id: run.id.clone(),
            tool_name: call.name.clone(),
            arguments: prepared.plan.input().clone(),
            fingerprint: prepared.plan.fingerprint().to_string(),
            status: "pending".into(),
            bridge_call_id: Some(call.id.clone()),
            runtime_id: None,
            runtime_request_id: None,
            runtime_ref: None,
            preview: prepared
                .plan
                .preview()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|_| problem("approval_unavailable"))?,
        };
        let waiting = Arc::new(Waiting {
            run_id: run.id.clone(),
            deadline,
            decision: Mutex::new(Decision::Pending),
            changed: Condvar::new(),
        });
        {
            let mut pending = self
                .pending
                .lock()
                .map_err(|_| problem("approval_unavailable"))?;
            if pending.len() >= 64 || pending.values().any(|w| w.run_id == run.id) {
                return Err(problem("approval_pending"));
            }
            pending.insert(id.clone(), waiting.clone());
        }
        let saved = engine.store.update(&run.session_id, |session| {
            if session
                .approvals
                .iter()
                .any(|a| a.run_id == run.id && a.status == "pending")
            {
                return Err("run already has a pending approval".into());
            }
            let active = session
                .runs
                .iter_mut()
                .find(|r| r.id == run.id)
                .ok_or("run not found")?;
            if active.status != RunStatus::Running || cancel.is_stopped() {
                return Err("run is not executing".into());
            }
            active.status = RunStatus::WaitingApproval;
            active.updated_at = aidex_native_sessions::now();
            session.approvals.push(approval.clone());
            Ok(())
        });
        if saved.is_err() {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(problem("approval_unavailable"));
        }
        let approval_id = EntityId::new(&id).map_err(|_| problem("approval_unavailable"))?;
        engine.emit(
            &run.session_id,
            EventKind::ApprovalCreated {
                approval_id: approval_id.clone(),
                intent_id: prepared.plan.call_id().clone(),
                summary: "review external runtime extension tool".into(),
                scope: ApprovalScope::Once,
                expires_at: Timestamp::from_system_time(expires_at).ok(),
            },
        );
        let result = (|| -> Result<PermitIssuer, BridgeError> {
            loop {
                let mut decision = waiting
                    .decision
                    .lock()
                    .map_err(|_| problem("approval_unavailable"))?;
                if cancel.is_stopped() {
                    *decision = Decision::Closed("cancelled");
                    break Err(problem("cancelled"));
                }
                if Instant::now() >= waiting.deadline {
                    *decision = Decision::Closed("approval_expired");
                    break Err(problem("approval_expired"));
                }
                if let Decision::Decided(approved) = *decision {
                    break if approved {
                        Ok(PermitIssuer::Approval {
                            approval_id: approval_id.clone(),
                        })
                    } else {
                        Err(problem("approval_denied"))
                    };
                }
                if let Decision::Closed(code) = *decision {
                    break Err(problem(code));
                }
                let active = engine
                    .store
                    .run(&run.id)
                    .map_err(|_| problem("approval_unavailable"))?;
                if !active.status.active() {
                    *decision = Decision::Closed("cancelled");
                    break Err(problem("cancelled"));
                }
                let _ = waiting
                    .changed
                    .wait_timeout(decision, Duration::from_millis(50))
                    .map_err(|_| problem("approval_unavailable"))?;
            }
        })();
        if let Err(error) = &result {
            let mut decision = waiting.decision.lock().unwrap_or_else(|e| e.into_inner());
            *decision = Decision::Closed(error.code);
            // Expiry retracts an approval that never produced a permit. Other
            // committed decisions stay immutable; terminal runs are not revived.
            let _ = expire_record(engine, run, &id, error.code == "approval_expired");
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            waiting.changed.notify_all();
        } else {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        }
        result
    }
    /// Called with the engine operation lock held by the ordinary approval RPC.
    pub(crate) fn respond(
        &self,
        engine: &NativeEngine,
        approval: &Approval,
        approved: bool,
    ) -> Result<Run, String> {
        let waiting = self
            .pending
            .lock()
            .map_err(|_| "bridge approvals poisoned")?
            .get(&approval.id)
            .cloned()
            .ok_or("MCP approval interrupted; request a fresh tool call")?;
        let mut decision = waiting
            .decision
            .lock()
            .map_err(|_| "bridge decision poisoned")?;
        match *decision {
            Decision::Pending => {}
            Decision::Decided(_) => return Err("MCP approval already decided".into()),
            Decision::Closed(code) => return Err(problem(code).message),
        }
        let run = engine.store.run(&approval.run_id)?;
        if Instant::now() >= waiting.deadline {
            *decision = Decision::Closed("approval_expired");
            let saved = expire_record(engine, &run, &approval.id, false);
            waiting.changed.notify_all();
            saved?;
            return Err(problem("approval_expired").message);
        }
        let (run, expired) = engine.store.update(&run.session_id, |session| {
            let current = session
                .approvals
                .iter_mut()
                .find(|a| a.id == approval.id)
                .ok_or("approval not found")?;
            if current.status != "pending"
                || current.fingerprint != approval.fingerprint
                || current.bridge_call_id != approval.bridge_call_id
            {
                return Err("MCP approval changed or expired".into());
            }
            let run = session
                .runs
                .iter_mut()
                .find(|r| r.id == approval.run_id)
                .ok_or("run not found")?;
            if run.status != RunStatus::WaitingApproval {
                return Err("approval requires a waiting run".into());
            }
            let expired = Instant::now() >= waiting.deadline;
            current.status = if expired {
                "expired"
            } else if approved {
                "approved"
            } else {
                "denied"
            }
            .into();
            if !session
                .approvals
                .iter()
                .any(|a| a.run_id == approval.run_id && a.status == "pending")
            {
                run.status = RunStatus::Running;
            }
            run.updated_at = aidex_native_sessions::now();
            Ok((run.clone(), expired))
        })?;
        if expired || Instant::now() >= waiting.deadline {
            *decision = Decision::Closed("approval_expired");
            if !expired {
                let saved = expire_record(engine, &run, &approval.id, approved);
                waiting.changed.notify_all();
                saved?;
            } else {
                emit_expiry(engine, &run, &approval.id);
                waiting.changed.notify_all();
            }
            return Err(problem("approval_expired").message);
        }
        *decision = Decision::Decided(approved);
        let id = EntityId::new(&approval.id).map_err(|e| e.to_string())?;
        engine.emit(
            &run.session_id,
            if approved {
                EventKind::ApprovalApproved {
                    approval_id: id,
                    scope: ApprovalScope::Once,
                }
            } else {
                EventKind::ApprovalDenied {
                    approval_id: id,
                    reason: None,
                }
            },
        );
        waiting.changed.notify_all();
        Ok(run)
    }
}
