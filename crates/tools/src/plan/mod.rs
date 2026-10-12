//! Two-phase execution: immutable, fingerprinted plans and the permits that
//! authorize executing exactly one of them.

mod facts;
mod fingerprint;
mod permit;

use std::{fmt, io};

use aidex_core::EntityId;
use aidex_events::RiskLevel;
use aidex_policy::{Action, Intent};
use serde::Serialize;
use serde_json::Value;

pub use facts::{EntryKind, NetworkFacts, PathAccess, PlanFacts, PlanPath};
pub use fingerprint::PlanFingerprint;
pub use permit::{ExecutionPermit, PermitIssuer};

use crate::{DiffPreview, ToolContext, ToolDescriptor, ToolError, ToolId, text};

/// Longest plan summary in bytes (after redaction).
pub const MAX_SUMMARY_BYTES: usize = 512;

/// What a tool's `plan` hands to [`ToolPlan::new`].
#[derive(Clone, Debug)]
pub struct PlanParts {
    /// The validated input re-serialized from its typed struct (defaults filled in).
    pub input: Value,
    pub facts: PlanFacts,
    /// Human summary for approval cards (redacted and bounded on construction).
    pub summary: String,
    /// Call-specific risk; the plan's risk is the maximum of this and the
    /// descriptor's base risk.
    pub risk: Option<RiskLevel>,
    /// The diff a write will apply, computed without side effects.
    pub preview: Option<DiffPreview>,
    /// Tool-private state recorded at plan time (expected hashes, tree digests).
    /// Fingerprinted like everything else.
    pub state: Value,
}

impl PlanParts {
    pub fn new(input: Value, summary: impl Into<String>) -> Self {
        Self {
            input,
            facts: PlanFacts::default(),
            summary: summary.into(),
            risk: None,
            preview: None,
            state: Value::Null,
        }
    }

    pub fn with_facts(mut self, facts: PlanFacts) -> Self {
        self.facts = facts;
        self
    }

    pub fn with_risk(mut self, risk: RiskLevel) -> Self {
        self.risk = Some(risk);
        self
    }

    pub fn with_preview(mut self, preview: DiffPreview) -> Self {
        self.preview = Some(preview);
        self
    }

    pub fn with_state(mut self, state: Value) -> Self {
        self.state = state;
        self
    }
}

/// An immutable, single-use description of exactly what one tool call will do.
///
/// Produced by `Tool::plan` with no side effects. The broker evaluates policy on its
/// facts (`intent()`), shows the summary and preview for approval, and then issues an
/// [`ExecutionPermit`] bound to [`ToolPlan::fingerprint`]. Execution uses only the
/// plan's normalized input, so what runs is what was approved; a plan whose content
/// no longer hashes to its fingerprint is refused.
#[derive(Clone, Serialize)]
pub struct ToolPlan {
    plan_id: EntityId,
    tool_id: ToolId,
    tool_version: u32,
    action: Action,
    session_id: EntityId,
    workspace_id: EntityId,
    call_id: EntityId,
    root: String,
    input: Value,
    facts: PlanFacts,
    risk: RiskLevel,
    summary: String,
    preview: Option<DiffPreview>,
    state: Value,
    fingerprint: PlanFingerprint,
}

impl ToolPlan {
    /// Seal a plan for `ctx`'s call: assigns a fresh plan id, bounds and redacts the
    /// summary, and computes the fingerprint.
    pub fn new(
        ctx: &ToolContext,
        descriptor: &ToolDescriptor,
        parts: PlanParts,
    ) -> Result<Self, ToolError> {
        let plan_id = EntityId::generate("plan").map_err(|error| {
            ToolError::io(
                "cannot generate a plan id",
                io::Error::other(error.to_string()),
            )
        })?;
        let risk = parts.risk.map_or(descriptor.risk.base_risk, |risk| {
            risk.max(descriptor.risk.base_risk)
        });
        let mut plan = Self {
            plan_id,
            tool_id: descriptor.id.clone(),
            tool_version: descriptor.version,
            action: descriptor.risk.action,
            session_id: ctx.session_id.clone(),
            workspace_id: ctx.workspace_id.clone(),
            call_id: ctx.call_id.clone(),
            root: ctx.root_display(),
            input: parts.input,
            facts: parts.facts,
            risk,
            summary: text::bounded(&parts.summary, MAX_SUMMARY_BYTES),
            preview: parts.preview,
            state: parts.state,
            fingerprint: PlanFingerprint::from_bytes([0; 32]),
        };
        plan.fingerprint = plan.compute_fingerprint()?;
        Ok(plan)
    }

    fn compute_fingerprint(&self) -> Result<PlanFingerprint, ToolError> {
        #[derive(Serialize)]
        struct Hashed<'a> {
            plan_id: &'a EntityId,
            tool_id: &'a ToolId,
            tool_version: u32,
            action: Action,
            session_id: &'a EntityId,
            workspace_id: &'a EntityId,
            call_id: &'a EntityId,
            root: &'a str,
            input: &'a Value,
            facts: &'a PlanFacts,
            risk: RiskLevel,
            summary: &'a str,
            preview: &'a Option<DiffPreview>,
            state: &'a Value,
        }
        let value = serde_json::to_value(Hashed {
            plan_id: &self.plan_id,
            tool_id: &self.tool_id,
            tool_version: self.tool_version,
            action: self.action,
            session_id: &self.session_id,
            workspace_id: &self.workspace_id,
            call_id: &self.call_id,
            root: &self.root,
            input: &self.input,
            facts: &self.facts,
            risk: self.risk,
            summary: &self.summary,
            preview: &self.preview,
            state: &self.state,
        })
        .map_err(|error| ToolError::PlanMismatch(format!("plan is not serializable: {error}")))?;
        Ok(PlanFingerprint::of(&value))
    }

    /// Recompute the fingerprint and compare it with the sealed one.
    pub fn verify_integrity(&self) -> Result<(), ToolError> {
        if self.compute_fingerprint()? == self.fingerprint {
            Ok(())
        } else {
            Err(ToolError::PlanMismatch(
                "the plan content no longer matches its fingerprint".into(),
            ))
        }
    }

    /// Refuse a context for a different workspace, session, call, or root.
    pub fn check_context(&self, ctx: &ToolContext) -> Result<(), ToolError> {
        let mismatch = if self.workspace_id != ctx.workspace_id {
            Some("workspace")
        } else if self.session_id != ctx.session_id {
            Some("session")
        } else if self.call_id != ctx.call_id {
            Some("call")
        } else if self.root != ctx.root_display() {
            Some("workspace root")
        } else {
            None
        };
        match mismatch {
            Some(what) => Err(ToolError::PlanMismatch(format!(
                "the plan was made for a different {what}"
            ))),
            None => Ok(()),
        }
    }

    pub fn plan_id(&self) -> &EntityId {
        &self.plan_id
    }

    pub fn tool_id(&self) -> &ToolId {
        &self.tool_id
    }

    pub fn tool_version(&self) -> u32 {
        self.tool_version
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn session_id(&self) -> &EntityId {
        &self.session_id
    }

    pub fn workspace_id(&self) -> &EntityId {
        &self.workspace_id
    }

    pub fn call_id(&self) -> &EntityId {
        &self.call_id
    }

    /// The canonical workspace root (display form) the plan was resolved against.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// The validated, normalized input.
    pub fn input(&self) -> &Value {
        &self.input
    }

    pub fn facts(&self) -> &PlanFacts {
        &self.facts
    }

    pub fn risk(&self) -> RiskLevel {
        self.risk
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn preview(&self) -> Option<&DiffPreview> {
        self.preview.as_ref()
    }

    /// Tool-private state recorded at plan time.
    pub fn state(&self) -> &Value {
        &self.state
    }

    pub fn fingerprint(&self) -> PlanFingerprint {
        self.fingerprint
    }

    /// The policy intent for the tool's own action.
    pub fn intent(&self) -> Intent {
        let mut intent = Intent::new(self.action, self.session_id.clone(), self.summary.clone())
            .with_workspace(self.workspace_id.clone())
            .with_tool(self.tool_id.entity_id().clone());
        intent.attributes = self.facts.intent_attributes();
        intent
    }

    /// The intent for the tool's action followed by one per implied action; the
    /// broker must allow every one of them before issuing a permit.
    pub fn intents(&self) -> Vec<Intent> {
        let primary = self.intent();
        let mut intents = Vec::with_capacity(1 + self.facts.implied_actions.len());
        for &action in &self.facts.implied_actions {
            if action != self.action {
                let mut implied = primary.clone();
                implied.action = action;
                intents.push(implied);
            }
        }
        intents.insert(0, primary);
        intents
    }
}

impl fmt::Debug for ToolPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolPlan")
            .field("plan_id", &self.plan_id)
            .field("tool_id", &self.tool_id)
            .field("action", &self.action)
            .field("call_id", &self.call_id)
            .field("risk", &self.risk)
            .field("summary", &self.summary)
            .field("paths", &self.facts.paths.len())
            .field("fingerprint", &self.fingerprint)
            .finish_non_exhaustive()
    }
}
