//! Shared fixtures for `aidex-events` integration tests.
#![allow(dead_code)]

use aidex_core::{EntityId, Timestamp, WorkspacePath};
use aidex_events::{
    ApprovalScope, DaemonStopReason, EventKind, EventLog, EventScope, ExtensionKind, FileChange,
    LegacyRecord, MessageRole, Outcome, ProcessStream, RetentionPolicy, RiskLevel, SandboxLevel,
    Verdict, WorkspaceKind,
};
use std::path::{Path, PathBuf};

pub fn id(value: &str) -> EntityId {
    EntityId::new(value).unwrap()
}

pub fn ws() -> EntityId {
    id("ws_main")
}

pub fn session_scope() -> EventScope {
    EventScope::session(ws(), id("ses_one"))
}

pub fn task_started(task: &str) -> EventKind {
    EventKind::TaskStarted { task_id: id(task) }
}

pub struct TempLog {
    pub dir: tempfile::TempDir,
    pub log: EventLog,
}

impl TempLog {
    pub fn path(&self) -> PathBuf {
        db_path(self.dir.path())
    }
}

pub fn db_path(dir: &Path) -> PathBuf {
    dir.join("events.sqlite3")
}

pub fn temp_log(retention: RetentionPolicy) -> TempLog {
    let dir = tempfile::tempdir().unwrap();
    let log = EventLog::open(db_path(dir.path()), retention).unwrap();
    TempLog { dir, log }
}

/// One value of every catalog variant; every free-text field holds `text`.
pub fn samples(text: &str) -> Vec<EventKind> {
    let t = || text.to_owned();
    let path = |value: &str| WorkspacePath::new(value).unwrap();
    vec![
        EventKind::SessionCreated {
            agent_id: Some(id("aidex")),
            title: Some(t()),
        },
        EventKind::SessionStarted {},
        EventKind::SessionPaused {},
        EventKind::SessionResumed {},
        EventKind::SessionCompleted {},
        EventKind::SessionFailed { reason: t() },
        EventKind::SessionCancelled {},
        EventKind::SessionInterrupted {
            reason: t(),
            recoverable: true,
        },
        EventKind::SessionCheckpointed {
            checkpoint_id: id("chk_1"),
            label: Some(t()),
        },
        EventKind::TaskCreated {
            task_id: id("task_1"),
            parent_task_id: Some(id("task_0")),
            role: t(),
        },
        EventKind::TaskStarted {
            task_id: id("task_1"),
        },
        EventKind::TaskCompleted {
            task_id: id("task_1"),
        },
        EventKind::TaskFailed {
            task_id: id("task_1"),
            reason: t(),
        },
        EventKind::TaskCancelled {
            task_id: id("task_1"),
        },
        EventKind::TaskVerification {
            task_id: id("task_1"),
            passed: false,
            summary: t(),
        },
        EventKind::AgentStarted {
            agent_id: id("aidex"),
            model: Some(id("qwen2.5-coder")),
        },
        EventKind::AgentMessage {
            role: MessageRole::Assistant,
            content: t(),
        },
        EventKind::AgentDelta { text: t() },
        EventKind::AgentSubagentCreated {
            child_session_id: id("ses_child"),
            role: t(),
        },
        EventKind::AgentError {
            code: t(),
            message: t(),
        },
        EventKind::AgentRetry {
            attempt: 2,
            reason: t(),
        },
        EventKind::AgentFinished {
            agent_id: id("aidex"),
            outcome: Outcome::Succeeded,
        },
        EventKind::ToolRequested {
            call_id: id("call_1"),
            tool_id: id("shell.exec"),
            summary: t(),
            risk: RiskLevel::High,
        },
        EventKind::ToolStarted {
            call_id: id("call_1"),
        },
        EventKind::ToolCompleted {
            call_id: id("call_1"),
            duration_ms: 12,
            summary: t(),
        },
        EventKind::ToolFailed {
            call_id: id("call_1"),
            code: t(),
            message: t(),
        },
        EventKind::ToolDenied {
            call_id: id("call_1"),
            reason: t(),
        },
        EventKind::ApprovalCreated {
            approval_id: id("apr_1"),
            intent_id: id("int_1"),
            summary: t(),
            scope: ApprovalScope::Once,
            expires_at: Some(Timestamp::new("2026-10-10T12:00:00Z").unwrap()),
        },
        EventKind::ApprovalApproved {
            approval_id: id("apr_1"),
            scope: ApprovalScope::Session,
        },
        EventKind::ApprovalDenied {
            approval_id: id("apr_1"),
            reason: Some(t()),
        },
        EventKind::ApprovalExpired {
            approval_id: id("apr_1"),
        },
        EventKind::ApprovalRevoked {
            approval_id: id("apr_1"),
            reason: Some(t()),
        },
        EventKind::ApprovalCancelled {
            approval_id: id("apr_1"),
        },
        EventKind::PolicyDecision {
            intent_id: id("int_1"),
            decision: Verdict::RequireApproval,
            rule: t(),
        },
        EventKind::PolicyChanged {
            profile: Some(id("balanced")),
            summary: t(),
        },
        EventKind::ProcessStarted {
            process_id: id("proc_1"),
            command_summary: t(),
            sandbox_level: SandboxLevel::Restricted,
        },
        EventKind::ProcessOutput {
            process_id: id("proc_1"),
            stream: ProcessStream::Stderr,
            chunk: t(),
        },
        EventKind::ProcessExited {
            process_id: id("proc_1"),
            exit_code: Some(0),
            signal: None,
        },
        EventKind::FileChanged {
            path: path("src/main.rs"),
            change: FileChange::Modified,
        },
        EventKind::DiffCreated {
            diff_id: id("diff_1"),
            files: vec![path("src/main.rs"), path("README.md")],
            additions: 3,
            deletions: 1,
        },
        EventKind::ModelSelected {
            provider_id: id("local"),
            model_id: id("qwen2.5-coder"),
            reason: t(),
        },
        EventKind::ModelLoaded {
            provider_id: id("local"),
            model_id: id("qwen2.5-coder"),
        },
        EventKind::ModelUnloaded {
            provider_id: id("local"),
            model_id: id("qwen2.5-coder"),
        },
        EventKind::ModelFallback {
            from: id("big"),
            to: id("small"),
            reason: t(),
        },
        EventKind::ModelError {
            provider_id: id("local"),
            model_id: id("qwen2.5-coder"),
            code: t(),
            message: t(),
        },
        EventKind::WorkspaceAdded {
            workspace_id: ws(),
            kind: WorkspaceKind::Drive,
            name: t(),
        },
        EventKind::WorkspaceRemoved { workspace_id: ws() },
        EventKind::WorkspaceSelected { workspace_id: ws() },
        EventKind::WorkspaceEphemeralCreated {
            workspace_id: id("ws_eph"),
            source_workspace_id: ws(),
        },
        EventKind::WorkspaceEphemeralApplied {
            workspace_id: id("ws_eph"),
            files_changed: 2,
        },
        EventKind::WorkspaceEphemeralDiscarded {
            workspace_id: id("ws_eph"),
        },
        EventKind::VaultUnlocked {
            vault_id: id("vault_main"),
        },
        EventKind::VaultLocked {
            vault_id: id("vault_main"),
        },
        EventKind::SecretGranted {
            handle_id: id("hdl_1"),
            secret_name: t(),
            consumer: id("mcp:github"),
        },
        EventKind::SecretRevoked {
            handle_id: id("hdl_1"),
            reason: Some(t()),
        },
        EventKind::SandboxNegotiated {
            level: SandboxLevel::Brokered,
            enforced: true,
        },
        EventKind::SandboxViolation {
            process_id: Some(id("proc_1")),
            detail: t(),
        },
        EventKind::CleanupStarted {
            cleanup_id: id("cln_1"),
        },
        EventKind::CleanupCompleted {
            cleanup_id: id("cln_1"),
            cleaned: 4,
            failed: 0,
            unmanaged: 1,
        },
        EventKind::DriveSafeToRemove {},
        EventKind::DriveRemoved { unexpected: true },
        EventKind::RemotePaired {
            device_id: id("dev_phone"),
            name: t(),
        },
        EventKind::RemoteRevoked {
            device_id: id("dev_phone"),
        },
        EventKind::RemoteConnected {
            device_id: id("dev_phone"),
        },
        EventKind::RemoteDisconnected {
            device_id: id("dev_phone"),
            reason: Some(t()),
        },
        EventKind::DaemonStarted {
            pid: 42,
            version: t(),
        },
        EventKind::DaemonStopping {
            reason: DaemonStopReason::Requested,
        },
        EventKind::EmergencyStopped {
            reason: t(),
            cancelled_approvals: 2,
            revoked_approvals: 1,
            terminated_processes: 3,
            revoked_handles: 4,
        },
        EventKind::EmergencyResumed {},
        EventKind::ExtensionEnabled {
            extension_id: id("skill_review"),
            kind: ExtensionKind::Skill,
        },
        EventKind::ExtensionDisabled {
            extension_id: id("mcp_github"),
            kind: ExtensionKind::Mcp,
        },
        EventKind::UpdateStaged {
            update_id: id("upd_1"),
            version: t(),
            target: t(),
        },
        EventKind::UpdateVerified {
            update_id: id("upd_1"),
            version: t(),
            signer_key_id: Some(t()),
            unsigned_allowed: false,
        },
        EventKind::UpdateSwitched {
            update_id: id("upd_1"),
            operation: t(),
            from_version: Some(t()),
            to_version: t(),
        },
        EventKind::UpdateRolledBack {
            update_id: id("upd_1"),
            from_version: Some(t()),
            to_version: None,
            reason: t(),
        },
        EventKind::UpdateFailed {
            update_id: None,
            stage: t(),
            code: t(),
            message: t(),
        },
        EventKind::LegacyV1 {
            record: LegacyRecord::ToolFinished,
            subject_id: Some(id("tool")),
            outcome: Some(Outcome::Failed),
            decision: None,
            exit_code: None,
        },
    ]
}

/// The PR #40 table layout, exactly as its `EventLog::open` created it.
pub const V1_SCHEMA: &str = "PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
CREATE TABLE IF NOT EXISTS events (
    workspace_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    session_id TEXT,
    body TEXT NOT NULL,
    PRIMARY KEY(workspace_id, sequence)
);";

/// A PR #40 `Event` body.
pub fn v1_body(workspace: &str, sequence: u64, session: Option<&str>, kind: &str) -> String {
    let session = session.map_or("null".to_owned(), |session| format!("\"{session}\""));
    format!(
        r#"{{"version":1,"sequence":{sequence},"workspace_id":"{workspace}","session_id":{session},"kind":{kind}}}"#
    )
}

/// Write a PR #40 database with `rows` of `(workspace, session, kind JSON)` in order.
pub fn write_v1_database(path: &Path, rows: &[(&str, Option<&str>, &str)]) {
    let connection = rusqlite::Connection::open(path).unwrap();
    connection.execute_batch(V1_SCHEMA).unwrap();
    let mut sequences = std::collections::HashMap::<&str, u64>::new();
    for (workspace, session, kind) in rows {
        let sequence = sequences.entry(workspace).or_default();
        *sequence += 1;
        connection
            .execute(
                "INSERT INTO events(workspace_id, sequence, session_id, body) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    workspace,
                    *sequence as i64,
                    session,
                    v1_body(workspace, *sequence, *session, kind)
                ],
            )
            .unwrap();
    }
}
