use aidex_providers::ChatMessage;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    Chat,
    Plan,
    Agent,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    #[default]
    Supervised,
    AutoAcceptEdits,
    Auto,
    FullAccess,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl RunStatus {
    pub fn active(self) -> bool {
        matches!(self, Self::Pending | Self::Running | Self::WaitingApproval)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    #[serde(default)]
    pub runtime_id: Option<String>,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub fallback_model: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub mode: Mode,
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub permission_mode: PermissionMode,
    #[serde(default = "default_steps")]
    pub max_steps: u32,
    #[serde(default = "default_tokens")]
    pub max_tokens: u32,
}
fn default_steps() -> u32 {
    16
}
fn default_tokens() -> u32 {
    4096
}
impl SessionConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.max_steps == 0
            || self.max_steps > 128
            || self.max_tokens == 0
            || self.max_tokens > 32768
            || self.title.len() > 512
            || self.instructions.len() > 32 * 1024
        {
            return Err("invalid session limits".into());
        }
        aidex_providers::ProviderId::new(&self.provider_id).map_err(|e| e.to_string())?;
        aidex_providers::ModelId::new(&self.model_id).map_err(|e| e.to_string())?;
        if let Some(fallback) = &self.fallback_model {
            aidex_providers::ModelRef::parse(fallback).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub session_id: String,
    pub parent_run_id: Option<String>,
    pub status: RunStatus,
    pub steps: u32,
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub transcript: Vec<ChatMessage>,
    #[serde(default)]
    pub pending_calls: Vec<aidex_providers::ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_input_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_used: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ref: Option<serde_json::Value>,
    /// Skill ids the user invoked for this run (`<scope>:<name>`). `Run` has no
    /// `deny_unknown_fields`, so daemons that predate the field ignore it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    /// The request turned on web tools for this run (Chat mode); per run, never
    /// part of the session configuration.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web_search: bool,
    /// Index of the run's user message in the session history (root runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_index: Option<usize>,
    /// Attachment ids sent with the run's user message, in order. A cache
    /// only: the attachments table is authoritative (an older daemon drops
    /// this field when it re-saves the session).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// Session mode when the run began (`None` for runs from before the field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    /// Session permission mode when the run began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// Session workspace when the run began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// Inference attempts repeated after a retryable provider failure. Each
    /// one also used a step; see [`crate::SessionStore::reserve_inference_retry`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub inference_retries: u32,
    /// The most older messages any request of this run left out of its replay
    /// view to fit the model's context window. History itself is unchanged.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub context_trimmed_messages: u32,
    /// The complete output of each workspace tool call of this run whose
    /// stored result is a model view (an applied diff reduced to its
    /// statistics), in the order they ran. History messages hold only the
    /// view, which any daemon may send to a model as it is; an older daemon
    /// ignores this field and drops it when it saves the session again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_outputs: Vec<ToolOutputRecord>,
    /// Task graph and node that created this run, when it is a graph child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_graph_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_node_id: Option<String>,
}
fn is_zero(value: &u32) -> bool {
    *value == 0
}
/// The complete serialized output of one tool call ([`Run::tool_outputs`]).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolOutputRecord {
    pub call_id: String,
    pub output: String,
}
/// Per-request options of a new run. Nothing here is persisted in the
/// session configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RunOptions {
    /// Invoked skill ids, already validated by the caller.
    pub skills: Vec<String>,
    /// Offer web tools in Chat mode for this run.
    pub web_search: bool,
    /// Staged attachment ids to send with the user message, in order (root
    /// runs only). They join the command fingerprint only when present.
    pub attachments: Vec<String>,
    /// Graph/node identity for an internally scheduled child run.
    pub task: Option<TaskRunRef>,
}
impl RunOptions {
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && !self.web_search
            && self.attachments.is_empty()
            && self.task.is_none()
    }
}
/// Where a forked conversation came from: its first `message_count` messages
/// are a copy of `session_id`'s history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ForkOrigin {
    pub session_id: String,
    pub message_count: usize,
}
/// A [`fork`](crate::SessionStore::fork) of a conversation into a new one.
#[derive(Clone, Debug)]
pub struct ForkRequest {
    /// Configuration of the new conversation (validated like `create`).
    pub configuration: SessionConfig,
    /// Replaces the configuration's title when present.
    pub title: Option<String>,
    /// Makes a retried fork return the conversation the first call created
    /// (1-128 bytes).
    pub command_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub run_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub fingerprint: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ref: Option<serde_json::Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommandRecord {
    pub fingerprint: String,
    pub run_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub run_id: String,
    pub path: String,
    pub before: Option<String>,
    pub after_digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub workspace_id: String,
    pub key: String,
    pub content: String,
    pub updated_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub session_id: String,
    pub status: RunStatus,
}
/// Identity assigned to a run created for one task graph node.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskRunRef {
    pub graph_id: String,
    pub node_id: String,
}
/// A persisted bounded orchestration graph.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskGraph {
    pub id: String,
    pub root_run_id: String,
    pub status: TaskGraphStatus,
    pub max_parallel: u32,
    pub max_steps: u32,
    #[serde(default)]
    pub steps_used: u32,
    #[serde(default)]
    pub nodes: Vec<TaskNode>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskGraphStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskNode {
    pub id: String,
    pub prompt: String,
    pub role: WorkerRole,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    pub status: TaskNodeStatus,
    #[serde(default)]
    pub input_artifacts: Vec<String>,
    #[serde(default)]
    pub output_artifacts: Vec<String>,
    pub step_budget: u32,
    #[serde(default)]
    pub steps_used: u32,
    #[serde(default)]
    pub verification: Option<TaskVerification>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskNodeStatus {
    #[default]
    Pending,
    Running,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerRole {
    Worker,
    Reviewer,
    Verifier,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskVerification {
    pub passed: bool,
    pub summary: String,
}

impl TaskGraph {
    /// Validate persisted graph structure and runtime bounds before mutation.
    pub fn validate(&self) -> Result<(), String> {
        const ID_MAX: usize = 128;
        const TEXT_MAX: usize = 4096;
        let valid_id = |s: &str| !s.is_empty() && s.len() <= ID_MAX;
        if !valid_id(&self.id) || !valid_id(&self.root_run_id) {
            return Err("invalid task graph identifier".into());
        }
        if !(1..=4).contains(&self.max_parallel) || !(1..=128).contains(&self.max_steps)
            || self.steps_used > self.max_steps {
            return Err("invalid task graph limits".into());
        }
        if self.nodes.len() > 128 {
            return Err("too many task graph nodes".into());
        }
        let mut indexes = std::collections::BTreeMap::new();
        for (i, node) in self.nodes.iter().enumerate() {
            if !valid_id(&node.id) || indexes.insert(node.id.as_str(), i).is_some()
                || node.step_budget == 0 || node.step_budget > self.max_steps
                || node.steps_used > node.step_budget {
                return Err("invalid or duplicate task node".into());
            }
            if node.run_id.as_deref().is_some_and(|id| !valid_id(id))
                || node.depends_on.len() > 128
                || node.input_artifacts.len() > 128 || node.output_artifacts.len() > 128
                || node.prompt.len() > 16 * 1024
                || node.error.as_deref().is_some_and(|s| s.len() > TEXT_MAX)
                || node.verification.as_ref().is_some_and(|v| v.summary.len() > TEXT_MAX) {
                return Err("invalid task node fields".into());
            }
            for artifact in node.input_artifacts.iter().chain(&node.output_artifacts) {
                if !valid_id(artifact) { return Err("invalid artifact identifier".into()); }
            }
            if node.depends_on.iter().any(|dependency| !valid_id(dependency)) {
                return Err("invalid dependency identifier".into());
            }
        }
        let mut marks = vec![0u8; self.nodes.len()];
        fn visit(i: usize, nodes: &[TaskNode], indexes: &std::collections::BTreeMap<&str, usize>, marks: &mut [u8]) -> Result<(), String> {
            if marks[i] == 1 { return Err("cyclic task graph dependency".into()); }
            if marks[i] == 2 { return Ok(()); }
            marks[i] = 1;
            for dep in &nodes[i].depends_on {
                let j = *indexes.get(dep.as_str()).ok_or("unknown task graph dependency")?;
                visit(j, nodes, indexes, marks)?;
            }
            marks[i] = 2;
            Ok(())
        }
        for i in 0..self.nodes.len() { visit(i, &self.nodes, &indexes, &mut marks)?; }
        if self.status == TaskGraphStatus::Completed
            && !self.nodes.iter().any(|n| n.role == WorkerRole::Verifier) {
            return Err("completed task graph requires a verifier".into());
        }
        Ok(())
    }
}

impl SessionDetail {
    pub fn validate_task_graph(&self, graph: &TaskGraph) -> Result<(), String> {
        graph.validate()?;
        if self.task_graphs.iter().any(|existing| existing.id == graph.id) {
            return Err("duplicate task graph identifier".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionDetail {
    pub id: String,
    #[serde(flatten)]
    pub configuration: SessionConfig,
    pub pinned: bool,
    pub created_at: u64,
    pub messages: Vec<ChatMessage>,
    pub runs: Vec<Run>,
    pub approvals: Vec<Approval>,
    pub checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    pub commands: BTreeMap<String, CommandRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_ref: Option<serde_json::Value>,
    /// Set on a conversation created by [`fork`](crate::SessionStore::fork).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkOrigin>,
    #[serde(default)]
    pub task_graphs: Vec<TaskGraph>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub status: String,
    pub locked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
}
