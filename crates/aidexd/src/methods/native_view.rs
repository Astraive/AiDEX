//! Bounded wire projections. Complete encrypted records never get truncated.
use crate::errors::RpcError;
use crate::methods::attachments::{AttachmentRef, attachment_ref};
use aidex_native_sessions::{
    Approval, AttachmentMeta, Checkpoint, ForkOrigin, Mode, PermissionMode, Run, SessionConfig,
    SessionDetail,
};
use aidex_providers::{ChatMessage, Role};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
pub const WIRE_BUDGET: usize = 900 * 1024;
pub const CHUNK_LIMIT: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct RunView {
    pub id: String,
    pub session_id: String,
    pub parent_run_id: Option<String>,
    pub status: aidex_native_sessions::RunStatus,
    pub steps: u32,
    pub error: Option<String>,
    pub error_truncated: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub effective_max_tokens: Option<u32>,
    pub estimated_input_tokens: Option<u32>,
    pub model_used: Option<String>,
    pub has_runtime_ref: bool,
    pub transcript_omitted: bool,
    pub pending_call_count: usize,
    /// Index of the run's user message in the conversation history (root runs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_index: Option<usize>,
    /// Conversation mode, permission mode and project when the run began;
    /// absent for runs recorded before these fields existed (or re-saved by
    /// an older daemon).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct CheckpointView {
    pub id: String,
    pub run_id: String,
    pub path: String,
    pub before_present: bool,
    pub before_bytes: Option<usize>,
    pub after_digest: String,
    pub content_omitted: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct CallView {
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub arguments_bytes: usize,
    pub arguments_truncated: bool,
    pub provider_metadata_omitted: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct MessageView {
    pub original_index: usize,
    pub role: Role,
    pub content: String,
    pub content_bytes: usize,
    pub content_truncated: bool,
    pub tool_calls: Vec<CallView>,
    pub tool_calls_total: usize,
    pub tool_calls_truncated: bool,
    pub tool_call_id: Option<String>,
    /// Skill ids the user invoked with this message (user messages of runs
    /// started with `skills`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// Attachments sent with this user message, in the order they were sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<AttachmentRef>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ApprovalView {
    pub id: String,
    pub run_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub arguments_bytes: usize,
    pub arguments_truncated: bool,
    pub fingerprint: String,
    pub status: String,
    pub preview: Option<Value>,
    pub preview_truncated: bool,
    pub runtime_id: Option<String>,
    pub runtime_request_id: Option<String>,
    pub runtime_ref_omitted: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct SessionView {
    pub id: String,
    #[serde(flatten)]
    pub configuration: SessionConfig,
    pub pinned: bool,
    pub created_at: u64,
    pub messages: Vec<MessageView>,
    pub messages_offset: usize,
    pub total_messages: usize,
    pub messages_truncated: bool,
    pub next_message_offset: Option<usize>,
    pub runs: Vec<RunView>,
    pub total_runs: usize,
    pub runs_truncated: bool,
    pub approvals: Vec<ApprovalView>,
    pub total_approvals: usize,
    pub approvals_truncated: bool,
    pub checkpoints: Vec<CheckpointView>,
    pub total_checkpoints: usize,
    pub checkpoints_truncated: bool,
    pub runtime_ref_omitted: bool,
    pub commands_omitted: bool,
    /// Set on a conversation created by `conversation.fork`: its first
    /// `message_count` messages are a copy of `session_id`'s history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkOrigin>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ContentChunk {
    pub content: String,
    pub exists: bool,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub total_bytes: usize,
    pub truncated: bool,
    pub digest: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetParams {
    pub id: String,
    #[serde(default)]
    pub message_offset: Option<usize>,
    #[serde(default)]
    pub message_limit: Option<usize>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageParams {
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryParams {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryReadParams {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    pub index: usize,
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub call_index: Option<usize>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointContentParams {
    pub session_id: String,
    pub id: String,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub limit: Option<usize>,
}

pub fn run(r: &Run) -> RunView {
    RunView {
        id: r.id.clone(),
        session_id: r.session_id.clone(),
        parent_run_id: r.parent_run_id.clone(),
        status: r.status,
        steps: r.steps,
        error: r.error.as_ref().map(|e| prefix(e, 2048)),
        error_truncated: r.error.as_ref().is_some_and(|e| e.len() > 2048),
        created_at: r.created_at,
        updated_at: r.updated_at,
        effective_max_tokens: r.effective_max_tokens,
        estimated_input_tokens: r.estimated_input_tokens,
        model_used: r.model_used.clone(),
        has_runtime_ref: r.runtime_ref.is_some(),
        transcript_omitted: true,
        pending_call_count: r.pending_calls.len(),
        message_index: r.message_index,
        mode: r.mode,
        permission_mode: r.permission_mode,
        workspace_id: r.workspace_id.clone(),
    }
}
pub fn checkpoint(c: &Checkpoint) -> CheckpointView {
    CheckpointView {
        id: c.id.clone(),
        run_id: c.run_id.clone(),
        path: c.path.clone(),
        before_present: c.before.is_some(),
        before_bytes: c.before.as_ref().map(String::len),
        after_digest: c.after_digest.clone(),
        content_omitted: true,
    }
}
/// Invoked skill ids by the session-history index of each root run's user
/// message.
pub type SkillIndex = HashMap<usize, Vec<String>>;
/// The [`SkillIndex`] of a session.
pub fn skill_index(s: &SessionDetail) -> SkillIndex {
    s.runs
        .iter()
        .filter(|r| r.parent_run_id.is_none() && !r.skills.is_empty())
        .filter_map(|r| Some((r.message_index?, r.skills.clone())))
        .collect()
}
/// Sent attachments by the session-history index of their user message.
pub type AttachmentIndex = HashMap<usize, Vec<AttachmentRef>>;
/// What session history adds to its user messages: invoked skills (from the
/// runs) and sent attachments (from the attachments table, which is
/// authoritative).
#[derive(Clone, Debug, Default)]
pub struct MessageExtras {
    pub skills: SkillIndex,
    pub attachments: AttachmentIndex,
}
impl MessageExtras {
    /// The extras of `s`, with its sent attachments as listed by
    /// `SessionStore::message_attachments`.
    pub fn of(s: &SessionDetail, attachments: BTreeMap<usize, Vec<AttachmentMeta>>) -> Self {
        Self {
            skills: skill_index(s),
            attachments: attachments
                .into_iter()
                .map(|(index, metas)| (index, metas.iter().map(attachment_ref).collect()))
                .collect(),
        }
    }
}
/// [`message`] with the skills invoked and the attachments sent with that
/// message, if any (user messages only).
pub fn message_with_extras(m: &ChatMessage, index: usize, extras: &MessageExtras) -> MessageView {
    let mut view = message(m, index);
    if m.role == Role::User {
        view.skills = extras.skills.get(&index).cloned();
        view.attachments = extras
            .attachments
            .get(&index)
            .filter(|refs| !refs.is_empty())
            .cloned();
    }
    view
}
pub fn message(m: &ChatMessage, index: usize) -> MessageView {
    MessageView {
        original_index: index,
        role: m.role,
        content: prefix(&m.content, 4096),
        content_bytes: m.content.len(),
        content_truncated: m.content.len() > 4096,
        tool_calls: m
            .tool_calls
            .iter()
            .take(4)
            .map(|c| CallView {
                id: prefix(&c.id, 256),
                name: prefix(&c.name, 128),
                arguments: prefix(&c.arguments, 1024),
                arguments_bytes: c.arguments.len(),
                arguments_truncated: c.arguments.len() > 1024,
                provider_metadata_omitted: c.provider_metadata.is_some(),
            })
            .collect(),
        tool_calls_total: m.tool_calls.len(),
        tool_calls_truncated: m.tool_calls.len() > 4,
        tool_call_id: m.tool_call_id.as_ref().map(|id| prefix(id, 256)),
        skills: None,
        attachments: None,
    }
}
pub fn approval(a: &Approval) -> ApprovalView {
    let arguments_bytes = a.arguments.to_string().len();
    let mut nodes = 0;
    let arguments = bounded_value(&a.arguments, 0, &mut nodes);
    let arguments_truncated = arguments != a.arguments;
    let preview = a.preview.as_ref().map(|v| {
        let mut nodes = 0;
        bounded_value(v, 0, &mut nodes)
    });
    ApprovalView {
        id: a.id.clone(),
        run_id: a.run_id.clone(),
        tool_name: prefix(&a.tool_name, 128),
        arguments,
        arguments_bytes,
        arguments_truncated,
        fingerprint: a.fingerprint.clone(),
        status: a.status.clone(),
        preview_truncated: preview != a.preview,
        preview,
        runtime_id: a.runtime_id.clone(),
        runtime_request_id: a.runtime_request_id.as_ref().map(|id| prefix(id, 256)),
        runtime_ref_omitted: a.runtime_ref.is_some(),
    }
}
pub fn session(
    s: &SessionDetail,
    offset: Option<usize>,
    limit: Option<usize>,
    extras: &MessageExtras,
) -> Result<Value, RpcError> {
    let limit = page_limit(limit)?;
    let offset = offset
        .unwrap_or_else(|| s.messages.len().saturating_sub(limit))
        .min(s.messages.len());
    let end = (offset + limit).min(s.messages.len());
    let view = SessionView {
        id: s.id.clone(),
        configuration: s.configuration.clone(),
        pinned: s.pinned,
        created_at: s.created_at,
        messages: s.messages[offset..end]
            .iter()
            .enumerate()
            .map(|(n, m)| message_with_extras(m, offset + n, extras))
            .collect(),
        messages_offset: offset,
        total_messages: s.messages.len(),
        messages_truncated: offset > 0 || end < s.messages.len(),
        next_message_offset: (end < s.messages.len()).then_some(end),
        runs: s.runs.iter().rev().take(100).rev().map(run).collect(),
        total_runs: s.runs.len(),
        runs_truncated: s.runs.len() > 100,
        approvals: s
            .approvals
            .iter()
            .rev()
            .take(20)
            .rev()
            .map(approval)
            .collect(),
        total_approvals: s.approvals.len(),
        approvals_truncated: s.approvals.len() > 20,
        checkpoints: s
            .checkpoints
            .iter()
            .rev()
            .take(100)
            .rev()
            .map(checkpoint)
            .collect(),
        total_checkpoints: s.checkpoints.len(),
        checkpoints_truncated: s.checkpoints.len() > 100,
        runtime_ref_omitted: s.runtime_ref.is_some(),
        commands_omitted: true,
        forked_from: s.forked_from.clone(),
    };
    bounded(
        serde_json::to_value(view).map_err(|e| RpcError::invalid_params(e.to_string()))?,
        true,
    )
}
pub fn history(
    messages: &[ChatMessage],
    offset: usize,
    limit: Option<usize>,
    extras: &MessageExtras,
) -> Result<Value, RpcError> {
    let limit = page_limit(limit)?;
    if offset > messages.len() {
        return Err(RpcError::invalid_params(
            "history offset exceeds total messages",
        ));
    }
    let end = (offset + limit).min(messages.len());
    bounded(
        json!({"messages":messages[offset..end].iter().enumerate().map(|(n,m)|message_with_extras(m,offset+n,extras)).collect::<Vec<_>>(),"offset":offset,"total_messages":messages.len(),"next_offset":(end<messages.len()).then_some(end),"truncated":end<messages.len()}),
        false,
    )
}
pub fn chunk(
    content: Option<&str>,
    offset: usize,
    limit: Option<usize>,
) -> Result<ContentChunk, RpcError> {
    let Some(content) = content else {
        return Ok(ContentChunk {
            content: String::new(),
            exists: false,
            offset: 0,
            next_offset: None,
            total_bytes: 0,
            truncated: false,
            digest: None,
        });
    };
    let limit = limit.unwrap_or(CHUNK_LIMIT).min(CHUNK_LIMIT);
    if limit == 0 || offset > content.len() || !content.is_char_boundary(offset) {
        return Err(RpcError::invalid_params(
            "invalid byte offset or chunk limit",
        ));
    }
    let mut end = (offset + limit).min(content.len());
    while !content.is_char_boundary(end) {
        end -= 1;
    }
    Ok(ContentChunk {
        content: content[offset..end].into(),
        exists: true,
        offset,
        next_offset: (end < content.len()).then_some(end),
        total_bytes: content.len(),
        truncated: end < content.len(),
        digest: Some(aidex_native_sessions::digest(content.as_bytes())),
    })
}
pub fn page_limit(limit: Option<usize>) -> Result<usize, RpcError> {
    let limit = limit.unwrap_or(100);
    if !(1..=100).contains(&limit) {
        return Err(RpcError::invalid_params("page limit must be 1-100"));
    }
    Ok(limit)
}
pub fn bounded(mut value: Value, recent: bool) -> Result<Value, RpcError> {
    while serde_json::to_vec(&value)
        .map_err(|e| RpcError::invalid_params(e.to_string()))?
        .len()
        > WIRE_BUDGET
    {
        let mut removed = false;
        for field in [
            "messages",
            "approvals",
            "checkpoints",
            "runs",
            "sessions",
            "memories",
            "entries",
            "matches",
            "commits",
        ] {
            if let Some(items) = value.get_mut(field).and_then(Value::as_array_mut)
                && !items.is_empty()
            {
                if recent {
                    items.remove(0);
                } else {
                    items.pop();
                }
                value[format!("{field}_truncated")] = json!(true);
                value["truncated"] = json!(true);
                removed = true;
                break;
            }
        }
        if !removed {
            return Err(RpcError::invalid_params(
                "bounded result metadata exceeds the wire budget",
            ));
        }
    }
    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        let first = messages
            .first()
            .and_then(|m| m.get("original_index"))
            .and_then(Value::as_u64)
            .unwrap_or_else(|| value.get("offset").and_then(Value::as_u64).unwrap_or(0));
        let next = first + messages.len() as u64;
        let total = value
            .get("total_messages")
            .and_then(Value::as_u64)
            .unwrap_or(next);
        if value.get("messages_offset").is_some() {
            value["messages_offset"] = json!(first);
            value["next_message_offset"] = if next < total {
                json!(next)
            } else {
                Value::Null
            };
        } else {
            value["next_offset"] = if next < total {
                json!(next)
            } else {
                Value::Null
            };
        }
    }
    Ok(value)
}
fn prefix(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].into()
}
fn bounded_value(value: &Value, depth: usize, nodes: &mut usize) -> Value {
    *nodes += 1;
    if depth >= 4 || *nodes >= 64 {
        return json!("[truncated]");
    }
    match value {
        Value::String(text) => json!(prefix(text, 512)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(8)
                .map(|v| bounded_value(v, depth + 1, nodes))
                .collect(),
        ),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .take(16)
                .map(|(k, v)| (prefix(k, 96), bounded_value(v, depth + 1, nodes)))
                .collect(),
        ),
        other => other.clone(),
    }
}
