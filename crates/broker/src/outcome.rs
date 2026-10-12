//! What callers submit to the broker and what they get back.

use std::fmt;

use aidex_core::{EntityId, ServiceErrorCode};
use aidex_events::RiskLevel;
use aidex_policy::SandboxLevel;
use aidex_tools::{PermitIssuer, ToolId, ToolOutput};
use serde::Serialize;
use serde_json::Value;

use crate::{
    Origin,
    approvals::{ApprovalCard, CardDiff},
    intent::{CombinedDecision, IntentDecision},
};

/// A request to run one tool. The broker plans it, evaluates policy on the plan's
/// facts, and executes only with a permit bound to that exact plan.
#[derive(Clone)]
pub struct ToolInvocation {
    pub session_id: EntityId,
    pub workspace_id: EntityId,
    /// The agent the call runs under (selects agent-layer policy rules).
    pub agent_id: Option<EntityId>,
    pub tool_id: ToolId,
    /// Untrusted tool input; validated against the tool's closed schema.
    pub input: Value,
    pub origin: Origin,
    /// Caller-assigned call id (the harness's tool-call id); generated when `None`.
    pub call_id: Option<EntityId>,
}

impl ToolInvocation {
    pub fn new(
        session_id: EntityId,
        workspace_id: EntityId,
        tool_id: ToolId,
        input: Value,
        origin: Origin,
    ) -> Self {
        Self {
            session_id,
            workspace_id,
            agent_id: None,
            tool_id,
            input,
            origin,
            call_id: None,
        }
    }

    pub fn with_agent(mut self, agent_id: EntityId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    pub fn with_call_id(mut self, call_id: EntityId) -> Self {
        self.call_id = Some(call_id);
        self
    }
}

/// Input may contain file content or credentials: never printed.
impl fmt::Debug for ToolInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolInvocation")
            .field("session_id", &self.session_id)
            .field("workspace_id", &self.workspace_id)
            .field("agent_id", &self.agent_id)
            .field("tool_id", &self.tool_id)
            .field("origin", &self.origin)
            .field("call_id", &self.call_id)
            .finish_non_exhaustive()
    }
}

/// How an executed call was authorized.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PermitInfo {
    pub issuer: PermitIssuer,
    /// Isolation the call's processes ran at (at least this level).
    pub sandbox: SandboxLevel,
}

/// A tool that was permitted but failed, or input the tool rejected.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ToolFailure {
    /// The tool's stable reason (`invalid_input`, `conflict`, `process_failed`, ...).
    pub reason: String,
    pub code: ServiceErrorCode,
    /// Redacted message naming the caller's own inputs.
    pub message: String,
}

/// The result of [`crate::Broker::submit`] or
/// [`crate::Broker::continue_after_approval`].
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum BrokerOutcome {
    /// The tool ran under a permit and returned output (untrusted data).
    Completed {
        call_id: EntityId,
        output: ToolOutput,
        permit: PermitInfo,
    },
    /// A human must decide; continue with `continue_after_approval(approval_id)`
    /// once `approval.approved` arrives.
    PendingApproval {
        call_id: EntityId,
        approval_id: EntityId,
        card: Box<ApprovalCard>,
    },
    /// Policy (or a failed sandbox negotiation, or a rate limit) refused the call;
    /// nothing ran.
    Denied {
        call_id: EntityId,
        reason: String,
        rule: String,
    },
    /// The call was refused by the tool (invalid input, conflict) or failed while
    /// running.
    Failed {
        call_id: EntityId,
        failure: ToolFailure,
    },
}

impl BrokerOutcome {
    pub fn call_id(&self) -> &EntityId {
        match self {
            Self::Completed { call_id, .. }
            | Self::PendingApproval { call_id, .. }
            | Self::Denied { call_id, .. }
            | Self::Failed { call_id, .. } => call_id,
        }
    }

    /// Stable label: `completed`, `pending_approval`, `denied`, `failed`.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Completed { .. } => "completed",
            Self::PendingApproval { .. } => "pending_approval",
            Self::Denied { .. } => "denied",
            Self::Failed { .. } => "failed",
        }
    }
}

/// A dry run of [`crate::Broker::submit`]: the plan and policy decision, with no
/// execution, approval, or event.
#[derive(Clone, Debug, Serialize)]
pub struct Explanation {
    pub tool_id: String,
    pub summary: String,
    pub effect: String,
    pub risk: RiskLevel,
    pub decision: CombinedDecision,
    pub intents: Vec<IntentDecision>,
    /// The session grant (approval id) that would satisfy an approval verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_grant: Option<EntityId>,
    /// Why the required sandbox cannot be provided here (the call would be denied).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_unavailable: Option<String>,
    pub targets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_preview: Option<CardDiff>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}
