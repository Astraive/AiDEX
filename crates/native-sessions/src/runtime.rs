//! Injection contract for separately owned external agent adapters.
//! Resume metadata is private protocol state, never authentication material.
use crate::{Mode, PermissionMode};
use aidex_core::CancelToken;
use aidex_providers::ChatMessage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `SessionConfig.provider_id` placeholder for sessions run by an external
/// runtime; the harness ignores the provider for those sessions.
pub const RUNTIME_PROVIDER_ID: &str = "runtime";
/// `SessionConfig.model_id` that asks an external runtime to use its own
/// configured default model: the harness sends [`RuntimeRequest::model`] as
/// `None` instead of this literal.
pub const RUNTIME_DEFAULT_MODEL: &str = "default";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeDescriptor {
    pub id: String,
    pub name: String,
    pub modes: Vec<Mode>,
    pub permission_modes: Vec<PermissionMode>,
    pub supports_resume: bool,
    pub supports_approvals: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeWorkspace {
    pub workspace_id: String,
    pub root: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeOperation {
    Start,
    Resume,
    Approval,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeApprovalDecision {
    pub request_id: String,
    pub approved: bool,
    pub fingerprint: String,
    pub runtime_ref: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeRequest {
    pub session_id: String,
    pub run_id: String,
    pub parent_run_id: Option<String>,
    pub operation: RuntimeOperation,
    pub mode: Mode,
    pub workspace: Option<RuntimeWorkspace>,
    pub permission_mode: PermissionMode,
    pub instructions: String,
    /// The model to use; `None` means the runtime's own default.
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_steps: u32,
    pub max_tokens: u32,
    pub runtime_ref: Option<Value>,
    pub approvals: Vec<RuntimeApprovalDecision>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeToolStatus {
    Requested,
    Started,
    Completed,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    /// The native identity of this launch, reported before the first turn can
    /// produce effects so a crash mid-turn still leaves a reference to resume.
    /// In memory only: the event is never stored, and the harness decides what
    /// part of the reference it persists.
    SessionStarted {
        runtime_ref: Value,
    },
    TextDelta {
        text: String,
    },
    ToolProgress {
        call_id: String,
        tool_name: String,
        status: RuntimeToolStatus,
        summary: String,
    },
    ApprovalRequest {
        request_id: String,
        tool_name: String,
        arguments: Value,
        summary: String,
        runtime_ref: Value,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeControl {
    Continue,
    Pause,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Completed,
    WaitingApproval,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeResponse {
    pub status: RuntimeStatus,
    pub runtime_ref: Option<Value>,
    pub message: Option<ChatMessage>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
}
impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for RuntimeError {}

/// Whether a saved `runtime_ref` is bound to a native branch and snapshot, so
/// `RuntimeOperation::Resume` may continue it: an object whose
/// `native_branch_id` and `snapshot_digest` are non-empty strings. References
/// saved before native continuation have neither, so their conversations
/// continue with `Start` and a transcript replay instead. Adapters and the
/// harness both decide with this one rule.
pub fn is_resumable_reference(runtime_ref: &Value) -> bool {
    ["native_branch_id", "snapshot_digest"].iter().all(|key| {
        runtime_ref
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    })
}

/// Implementations must validate declared modes/permissions/model support, use
/// the selected confined root, honor cancellation with managed process-tree
/// termination, and pause BEFORE the requested effect when callback returns Pause.
/// A paused response must provide an opaque resume reference; Approval/Resume
/// operations continue that reference and must never blindly replay an old effect.
pub trait RuntimeExecutor: Send + Sync {
    fn descriptor(&self) -> RuntimeDescriptor;
    fn validate(&self, request: &RuntimeRequest) -> Result<(), RuntimeError>;
    fn execute(
        &self,
        request: &RuntimeRequest,
        on_event: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError>;
    /// Release whatever a launch of `run_id` kept for an approval decision
    /// (a paused process, its temporary home and credential): the harness
    /// will not continue it, because the run was cancelled, failed,
    /// interrupted, resumed, or its continuation ended. Called without any
    /// engine lock, possibly more than once and for runs that kept nothing,
    /// so it must be idempotent. A later `Resume` of the run launches anew.
    /// The default keeps nothing to release.
    fn release(&self, _run_id: &str) {}
    /// Whether the launch of `run_id` paused for an approval still waits for
    /// its decision: its process is live and can take one. The harness asks
    /// before it records a decision, so a decision that could no longer reach
    /// the runtime (the paused process ended: its wall timeout, a crash) is
    /// never recorded as made. Its approval expires instead, and the run
    /// returns to `interrupted`. The default assumes it does; `execute` then
    /// refuses the continuation with `approval_expired`, which the harness
    /// settles the same way.
    fn awaits_decision(&self, _run_id: &str) -> bool {
        true
    }
    /// Forget the native state a saved `runtime_ref` of this runtime names
    /// (a snapshot of a refused, superseded or finished native thread): the
    /// harness no longer holds any reference to it. Best effort and
    /// idempotent; a reference that names nothing of this runtime's is
    /// ignored. The default keeps nothing to forget.
    fn forget(&self, _runtime_ref: &Value) {}
}
