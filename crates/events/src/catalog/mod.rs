//! The closed, typed catalog of AiDEX product events (spec §19, §81, §94).
//!
//! Wire form: `{"type": "<dotted.name>", "data": {...}}`; unknown types and unknown
//! data fields are rejected. Payloads use only `aidex-core` primitives and the small
//! enums in [`values`]. Identifiers are [`EntityId`]s; `String` fields are free text
//! that is redacted and bounded on append. Never put secret values in any field:
//! secrets are referenced by handle and name only.

mod legacy;
mod values;

pub(crate) use legacy::import_v1_body;
pub use legacy::{LegacyApprovalDecision, LegacyRecord};
pub use values::{
    ApprovalScope, DaemonStopReason, ExtensionKind, FileChange, MessageRole, Outcome,
    ProcessStream, RiskLevel, SandboxLevel, Verdict, WorkspaceKind,
};

use std::fmt;

use aidex_core::{EntityId, Timestamp, WorkspacePath};
use serde::{Deserialize, Serialize};

/// Every event AiDEX emits. See `protocol/events/README.md` for the field table.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", deny_unknown_fields)]
pub enum EventKind {
    // --- session.* (scope: workspace + session) ---
    #[serde(rename = "session.created")]
    SessionCreated {
        agent_id: Option<EntityId>,
        title: Option<String>,
    },
    #[serde(rename = "session.started")]
    SessionStarted {},
    #[serde(rename = "session.paused")]
    SessionPaused {},
    #[serde(rename = "session.resumed")]
    SessionResumed {},
    #[serde(rename = "session.completed")]
    SessionCompleted {},
    #[serde(rename = "session.failed")]
    SessionFailed { reason: String },
    #[serde(rename = "session.cancelled")]
    SessionCancelled {},
    #[serde(rename = "session.interrupted")]
    SessionInterrupted { reason: String, recoverable: bool },
    #[serde(rename = "session.checkpointed")]
    SessionCheckpointed {
        checkpoint_id: EntityId,
        label: Option<String>,
    },

    // --- task.* ---
    #[serde(rename = "task.created")]
    TaskCreated {
        task_id: EntityId,
        parent_task_id: Option<EntityId>,
        role: String,
    },
    #[serde(rename = "task.started")]
    TaskStarted { task_id: EntityId },
    #[serde(rename = "task.completed")]
    TaskCompleted { task_id: EntityId },
    #[serde(rename = "task.failed")]
    TaskFailed { task_id: EntityId, reason: String },
    #[serde(rename = "task.cancelled")]
    TaskCancelled { task_id: EntityId },
    #[serde(rename = "task.verification")]
    TaskVerification {
        task_id: EntityId,
        passed: bool,
        summary: String,
    },

    // --- agent.* ---
    #[serde(rename = "agent.started")]
    AgentStarted {
        agent_id: EntityId,
        model: Option<EntityId>,
    },
    #[serde(rename = "agent.message")]
    AgentMessage { role: MessageRole, content: String },
    /// Live-only incremental output of a conversation's root runs (a child
    /// run's output is only in `agent.run_delta`); never stored or replayed.
    #[serde(rename = "agent.delta")]
    AgentDelta { text: String },
    /// Live-only incremental output of one run (`agent_id` is the run id),
    /// for every run, root and child; never stored or replayed.
    #[serde(rename = "agent.run_delta")]
    AgentRunDelta { agent_id: EntityId, text: String },
    #[serde(rename = "agent.subagent.created")]
    AgentSubagentCreated {
        child_session_id: EntityId,
        role: String,
    },
    #[serde(rename = "agent.error")]
    AgentError { code: String, message: String },
    #[serde(rename = "agent.retry")]
    AgentRetry { attempt: u32, reason: String },
    #[serde(rename = "agent.finished")]
    AgentFinished {
        agent_id: EntityId,
        outcome: Outcome,
    },

    // --- tool.* ---
    #[serde(rename = "tool.requested")]
    ToolRequested {
        call_id: EntityId,
        tool_id: EntityId,
        summary: String,
        risk: RiskLevel,
    },
    #[serde(rename = "tool.started")]
    ToolStarted { call_id: EntityId },
    #[serde(rename = "tool.completed")]
    ToolCompleted {
        call_id: EntityId,
        duration_ms: u64,
        summary: String,
    },
    #[serde(rename = "tool.failed")]
    ToolFailed {
        call_id: EntityId,
        code: String,
        message: String,
    },
    #[serde(rename = "tool.denied")]
    ToolDenied { call_id: EntityId, reason: String },

    // --- approval.* ---
    #[serde(rename = "approval.created")]
    ApprovalCreated {
        approval_id: EntityId,
        intent_id: EntityId,
        summary: String,
        scope: ApprovalScope,
        expires_at: Option<Timestamp>,
    },
    #[serde(rename = "approval.approved")]
    ApprovalApproved {
        approval_id: EntityId,
        scope: ApprovalScope,
    },
    #[serde(rename = "approval.denied")]
    ApprovalDenied {
        approval_id: EntityId,
        reason: Option<String>,
    },
    #[serde(rename = "approval.expired")]
    ApprovalExpired { approval_id: EntityId },
    #[serde(rename = "approval.revoked")]
    ApprovalRevoked {
        approval_id: EntityId,
        reason: Option<String>,
    },
    #[serde(rename = "approval.cancelled")]
    ApprovalCancelled { approval_id: EntityId },

    // --- policy.* ---
    #[serde(rename = "policy.decision")]
    PolicyDecision {
        intent_id: EntityId,
        decision: Verdict,
        rule: String,
    },
    #[serde(rename = "policy.changed")]
    PolicyChanged {
        profile: Option<EntityId>,
        summary: String,
    },

    // --- process.* ---
    #[serde(rename = "process.started")]
    ProcessStarted {
        process_id: EntityId,
        command_summary: String,
        sandbox_level: SandboxLevel,
    },
    /// Live-only output chunk; never stored or replayed.
    #[serde(rename = "process.output")]
    ProcessOutput {
        process_id: EntityId,
        stream: ProcessStream,
        chunk: String,
    },
    #[serde(rename = "process.exited")]
    ProcessExited {
        process_id: EntityId,
        exit_code: Option<i32>,
        signal: Option<i32>,
    },

    // --- file.* / diff.* ---
    #[serde(rename = "file.changed")]
    FileChanged {
        path: WorkspacePath,
        change: FileChange,
    },
    #[serde(rename = "diff.created")]
    DiffCreated {
        diff_id: EntityId,
        files: Vec<WorkspacePath>,
        additions: u64,
        deletions: u64,
    },

    // --- model.* ---
    #[serde(rename = "model.selected")]
    ModelSelected {
        provider_id: EntityId,
        model_id: EntityId,
        reason: String,
    },
    #[serde(rename = "model.loaded")]
    ModelLoaded {
        provider_id: EntityId,
        model_id: EntityId,
    },
    #[serde(rename = "model.unloaded")]
    ModelUnloaded {
        provider_id: EntityId,
        model_id: EntityId,
    },
    #[serde(rename = "model.fallback")]
    ModelFallback {
        from: EntityId,
        to: EntityId,
        reason: String,
    },
    #[serde(rename = "model.error")]
    ModelError {
        provider_id: EntityId,
        model_id: EntityId,
        code: String,
        message: String,
    },

    // --- workspace.* (scope: the workspace) ---
    #[serde(rename = "workspace.added")]
    WorkspaceAdded {
        workspace_id: EntityId,
        kind: WorkspaceKind,
        name: String,
    },
    #[serde(rename = "workspace.removed")]
    WorkspaceRemoved { workspace_id: EntityId },
    #[serde(rename = "workspace.selected")]
    WorkspaceSelected { workspace_id: EntityId },
    #[serde(rename = "workspace.ephemeral_created")]
    WorkspaceEphemeralCreated {
        workspace_id: EntityId,
        source_workspace_id: EntityId,
    },
    #[serde(rename = "workspace.ephemeral_applied")]
    WorkspaceEphemeralApplied {
        workspace_id: EntityId,
        files_changed: u64,
    },
    #[serde(rename = "workspace.ephemeral_discarded")]
    WorkspaceEphemeralDiscarded { workspace_id: EntityId },

    // --- vault.* / secret.* (audit; never values) ---
    #[serde(rename = "vault.unlocked")]
    VaultUnlocked { vault_id: EntityId },
    #[serde(rename = "vault.locked")]
    VaultLocked { vault_id: EntityId },
    #[serde(rename = "secret.granted")]
    SecretGranted {
        handle_id: EntityId,
        secret_name: String,
        consumer: EntityId,
    },
    #[serde(rename = "secret.revoked")]
    SecretRevoked {
        handle_id: EntityId,
        reason: Option<String>,
    },

    // --- sandbox.* ---
    #[serde(rename = "sandbox.negotiated")]
    SandboxNegotiated { level: SandboxLevel, enforced: bool },
    #[serde(rename = "sandbox.violation")]
    SandboxViolation {
        process_id: Option<EntityId>,
        detail: String,
    },

    // --- cleanup.* / drive.* ---
    #[serde(rename = "cleanup.started")]
    CleanupStarted { cleanup_id: EntityId },
    #[serde(rename = "cleanup.completed")]
    CleanupCompleted {
        cleanup_id: EntityId,
        cleaned: u64,
        failed: u64,
        unmanaged: u64,
    },
    #[serde(rename = "drive.safe_to_remove")]
    DriveSafeToRemove {},
    #[serde(rename = "drive.removed")]
    DriveRemoved { unexpected: bool },

    // --- remote.* ---
    #[serde(rename = "remote.paired")]
    RemotePaired { device_id: EntityId, name: String },
    #[serde(rename = "remote.revoked")]
    RemoteRevoked { device_id: EntityId },
    #[serde(rename = "remote.connected")]
    RemoteConnected { device_id: EntityId },
    #[serde(rename = "remote.disconnected")]
    RemoteDisconnected {
        device_id: EntityId,
        reason: Option<String>,
    },

    // --- daemon.* (scope: global) ---
    #[serde(rename = "daemon.started")]
    DaemonStarted { pid: u32, version: String },
    #[serde(rename = "daemon.stopping")]
    DaemonStopping { reason: DaemonStopReason },

    // --- emergency.* (scope: global; spec §48) ---
    /// An emergency stop froze tool execution, cancelled approvals, terminated
    /// managed processes, and revoked secret handles.
    #[serde(rename = "emergency.stopped")]
    EmergencyStopped {
        reason: String,
        cancelled_approvals: u64,
        revoked_approvals: u64,
        terminated_processes: u64,
        revoked_handles: u64,
    },
    /// Tool execution resumed after an emergency stop.
    #[serde(rename = "emergency.resumed")]
    EmergencyResumed {},

    // --- extension.* ---
    #[serde(rename = "extension.enabled")]
    ExtensionEnabled {
        extension_id: EntityId,
        kind: ExtensionKind,
    },
    #[serde(rename = "extension.disabled")]
    ExtensionDisabled {
        extension_id: EntityId,
        kind: ExtensionKind,
    },

    // --- update.* (scope: global; `aidex-updater`) ---
    /// An update package was extracted into `app/staging/<update_id>/`.
    #[serde(rename = "update.staged")]
    UpdateStaged {
        update_id: EntityId,
        version: String,
        target: String,
    },
    /// The staged bundle passed signature, integrity, target, version, and schema
    /// checks. `signer_key_id` is `null` only for an unsigned bundle accepted through
    /// the explicit development opt-in (`unsigned_allowed: true`).
    #[serde(rename = "update.verified")]
    UpdateVerified {
        update_id: EntityId,
        version: String,
        signer_key_id: Option<String>,
        unsigned_allowed: bool,
    },
    /// `app/current.json` now selects `to_version` (takes effect on next launch).
    #[serde(rename = "update.switched")]
    UpdateSwitched {
        update_id: EntityId,
        operation: String,
        from_version: Option<String>,
        to_version: String,
    },
    /// The active bundle was switched back (`aidex update rollback`) or an interrupted
    /// operation was rolled back during recovery.
    #[serde(rename = "update.rolled_back")]
    UpdateRolledBack {
        update_id: EntityId,
        from_version: Option<String>,
        to_version: Option<String>,
        reason: String,
    },
    /// An update, rollback, or recovery step failed; the active bundle is unchanged.
    #[serde(rename = "update.failed")]
    UpdateFailed {
        update_id: Option<EntityId>,
        stage: String,
        code: String,
        message: String,
    },

    // --- legacy.* ---
    /// A PR #40 (event log v1) record imported without a faithful equivalent above.
    /// Never emitted by new code.
    #[serde(rename = "legacy.v1")]
    LegacyV1 {
        record: LegacyRecord,
        subject_id: Option<EntityId>,
        outcome: Option<Outcome>,
        decision: Option<LegacyApprovalDecision>,
        exit_code: Option<i32>,
    },
}

/// Every event name in the catalog, in declaration order.
pub const EVENT_NAMES: &[&str] = &[
    "session.created",
    "session.started",
    "session.paused",
    "session.resumed",
    "session.completed",
    "session.failed",
    "session.cancelled",
    "session.interrupted",
    "session.checkpointed",
    "task.created",
    "task.started",
    "task.completed",
    "task.failed",
    "task.cancelled",
    "task.verification",
    "agent.started",
    "agent.message",
    "agent.delta",
    "agent.run_delta",
    "agent.subagent.created",
    "agent.error",
    "agent.retry",
    "agent.finished",
    "tool.requested",
    "tool.started",
    "tool.completed",
    "tool.failed",
    "tool.denied",
    "approval.created",
    "approval.approved",
    "approval.denied",
    "approval.expired",
    "approval.revoked",
    "approval.cancelled",
    "policy.decision",
    "policy.changed",
    "process.started",
    "process.output",
    "process.exited",
    "file.changed",
    "diff.created",
    "model.selected",
    "model.loaded",
    "model.unloaded",
    "model.fallback",
    "model.error",
    "workspace.added",
    "workspace.removed",
    "workspace.selected",
    "workspace.ephemeral_created",
    "workspace.ephemeral_applied",
    "workspace.ephemeral_discarded",
    "vault.unlocked",
    "vault.locked",
    "secret.granted",
    "secret.revoked",
    "sandbox.negotiated",
    "sandbox.violation",
    "cleanup.started",
    "cleanup.completed",
    "drive.safe_to_remove",
    "drive.removed",
    "remote.paired",
    "remote.revoked",
    "remote.connected",
    "remote.disconnected",
    "daemon.started",
    "daemon.stopping",
    "emergency.stopped",
    "emergency.resumed",
    "extension.enabled",
    "extension.disabled",
    "update.staged",
    "update.verified",
    "update.switched",
    "update.rolled_back",
    "update.failed",
    "legacy.v1",
];

impl EventKind {
    /// The dotted wire name, e.g. `tool.requested`.
    pub fn name(&self) -> &'static str {
        match self {
            Self::SessionCreated { .. } => "session.created",
            Self::SessionStarted {} => "session.started",
            Self::SessionPaused {} => "session.paused",
            Self::SessionResumed {} => "session.resumed",
            Self::SessionCompleted {} => "session.completed",
            Self::SessionFailed { .. } => "session.failed",
            Self::SessionCancelled {} => "session.cancelled",
            Self::SessionInterrupted { .. } => "session.interrupted",
            Self::SessionCheckpointed { .. } => "session.checkpointed",
            Self::TaskCreated { .. } => "task.created",
            Self::TaskStarted { .. } => "task.started",
            Self::TaskCompleted { .. } => "task.completed",
            Self::TaskFailed { .. } => "task.failed",
            Self::TaskCancelled { .. } => "task.cancelled",
            Self::TaskVerification { .. } => "task.verification",
            Self::AgentStarted { .. } => "agent.started",
            Self::AgentMessage { .. } => "agent.message",
            Self::AgentDelta { .. } => "agent.delta",
            Self::AgentRunDelta { .. } => "agent.run_delta",
            Self::AgentSubagentCreated { .. } => "agent.subagent.created",
            Self::AgentError { .. } => "agent.error",
            Self::AgentRetry { .. } => "agent.retry",
            Self::AgentFinished { .. } => "agent.finished",
            Self::ToolRequested { .. } => "tool.requested",
            Self::ToolStarted { .. } => "tool.started",
            Self::ToolCompleted { .. } => "tool.completed",
            Self::ToolFailed { .. } => "tool.failed",
            Self::ToolDenied { .. } => "tool.denied",
            Self::ApprovalCreated { .. } => "approval.created",
            Self::ApprovalApproved { .. } => "approval.approved",
            Self::ApprovalDenied { .. } => "approval.denied",
            Self::ApprovalExpired { .. } => "approval.expired",
            Self::ApprovalRevoked { .. } => "approval.revoked",
            Self::ApprovalCancelled { .. } => "approval.cancelled",
            Self::PolicyDecision { .. } => "policy.decision",
            Self::PolicyChanged { .. } => "policy.changed",
            Self::ProcessStarted { .. } => "process.started",
            Self::ProcessOutput { .. } => "process.output",
            Self::ProcessExited { .. } => "process.exited",
            Self::FileChanged { .. } => "file.changed",
            Self::DiffCreated { .. } => "diff.created",
            Self::ModelSelected { .. } => "model.selected",
            Self::ModelLoaded { .. } => "model.loaded",
            Self::ModelUnloaded { .. } => "model.unloaded",
            Self::ModelFallback { .. } => "model.fallback",
            Self::ModelError { .. } => "model.error",
            Self::WorkspaceAdded { .. } => "workspace.added",
            Self::WorkspaceRemoved { .. } => "workspace.removed",
            Self::WorkspaceSelected { .. } => "workspace.selected",
            Self::WorkspaceEphemeralCreated { .. } => "workspace.ephemeral_created",
            Self::WorkspaceEphemeralApplied { .. } => "workspace.ephemeral_applied",
            Self::WorkspaceEphemeralDiscarded { .. } => "workspace.ephemeral_discarded",
            Self::VaultUnlocked { .. } => "vault.unlocked",
            Self::VaultLocked { .. } => "vault.locked",
            Self::SecretGranted { .. } => "secret.granted",
            Self::SecretRevoked { .. } => "secret.revoked",
            Self::SandboxNegotiated { .. } => "sandbox.negotiated",
            Self::SandboxViolation { .. } => "sandbox.violation",
            Self::CleanupStarted { .. } => "cleanup.started",
            Self::CleanupCompleted { .. } => "cleanup.completed",
            Self::DriveSafeToRemove {} => "drive.safe_to_remove",
            Self::DriveRemoved { .. } => "drive.removed",
            Self::RemotePaired { .. } => "remote.paired",
            Self::RemoteRevoked { .. } => "remote.revoked",
            Self::RemoteConnected { .. } => "remote.connected",
            Self::RemoteDisconnected { .. } => "remote.disconnected",
            Self::DaemonStarted { .. } => "daemon.started",
            Self::DaemonStopping { .. } => "daemon.stopping",
            Self::EmergencyStopped { .. } => "emergency.stopped",
            Self::EmergencyResumed {} => "emergency.resumed",
            Self::ExtensionEnabled { .. } => "extension.enabled",
            Self::ExtensionDisabled { .. } => "extension.disabled",
            Self::UpdateStaged { .. } => "update.staged",
            Self::UpdateVerified { .. } => "update.verified",
            Self::UpdateSwitched { .. } => "update.switched",
            Self::UpdateRolledBack { .. } => "update.rolled_back",
            Self::UpdateFailed { .. } => "update.failed",
            Self::LegacyV1 { .. } => "legacy.v1",
        }
    }

    /// The first name segment, e.g. `agent` for `agent.subagent.created`.
    pub fn domain(&self) -> &'static str {
        let name = self.name();
        name.split_once('.').map_or(name, |(domain, _)| domain)
    }

    /// Durable events are stored, get a cursor, and replay. Ephemeral events
    /// (`agent.delta`, `agent.run_delta`, `process.output`) are delivered to live
    /// subscribers only.
    pub fn is_durable(&self) -> bool {
        !matches!(
            self,
            Self::AgentDelta { .. } | Self::AgentRunDelta { .. } | Self::ProcessOutput { .. }
        )
    }
}

/// Shows the event name and its redacted, bounded data; never raw free text.
impl fmt::Debug for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "EventKind({} {})",
            self.name(),
            crate::sanitize::debug_data(self)
        )
    }
}
