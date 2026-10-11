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
    pub skill_ids: Vec<String>,
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
            || self.skill_ids.len() > 32
            || self.skill_ids.iter().any(|id| {
                id.is_empty()
                    || id.len() > 64
                    || !id.as_bytes()[0].is_ascii_lowercase()
                    || !id.bytes().all(|b| {
                        b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
                    })
            })
            || self
                .skill_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.skill_ids.len()
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
    #[serde(default)]
    pub inference_retries: u32,
    #[serde(default)]
    pub context_trimmed_messages: u32,
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
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub run_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub fingerprint: String,
    pub status: String,
    /// Internal correlation for a waiting MCP call, never a runtime credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bridge_call_id: Option<String>,
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
