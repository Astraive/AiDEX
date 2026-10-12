//! `session.*` methods over the drive's durable `aidex-sessions` store.
//!
//! The daemon records and reports session state; it does not run agents yet.
//! `session.resume` adopts an interrupted session on this host and returns its
//! resume plan, but continuing the work is the native harness's job (#19) and is
//! reported as unavailable rather than simulated. Domain errors carry `data.reason`
//! (see `SessionError::reason`).

use crate::{
    errors::RpcError,
    methods::workspace::workspace_error,
    router::{Router, RouterError},
    rpc::{MAX_MESSAGE_BYTES, parse_params, to_result},
    services::Services,
};
use aidex_core::EntityId;
use aidex_sessions::{
    CheckpointId, CheckpointMeta, EventGap, HostFingerprint, MessagePage, NewSession, ResumePlan,
    Session, SessionError, SessionFilter, SessionState, SessionStateKind, UnrecoverableItem,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

pub use aidex_sessions::SessionSummary;

pub const CREATE: &str = "session.create";
pub const LIST: &str = "session.list";
pub const SHOW: &str = "session.show";
pub const MESSAGES: &str = "session.messages";
pub const CHECKPOINTS: &str = "session.checkpoints";
pub const RESUME_PLAN: &str = "session.resume_plan";
pub const RESUME: &str = "session.resume";
pub const CANCEL: &str = "session.cancel";
pub const BRANCH: &str = "session.branch";

/// Agent recorded when `session.create` names none: the native AiDEX agent.
pub const DEFAULT_AGENT: &str = "aidex";
/// Default `session.messages` page size.
pub const DEFAULT_MESSAGE_LIMIT: usize = 100;
/// Most checkpoint records (newest) and plan items returned in one result.
pub const MAX_LISTED: usize = 500;
/// What `session.resume` reports about continuing the work.
pub const CONTINUATION_UNAVAILABLE: &str = "continuation is not available yet: the native agent harness (#19) is not wired to the daemon; the session was adopted on this host and left paused";

/// Encoded bytes per message page, leaving headroom under the 1 MiB message limit.
const PAGE_BYTE_BUDGET: usize = MAX_MESSAGE_BYTES - 256 * 1024;

/// Params of `session.create`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateParams {
    /// Exact id of a registered workspace.
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub title: String,
}

/// Params of `session.list`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<SessionStateKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Params of `session.show|checkpoints|resume_plan|resume|cancel`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdParams {
    pub id: String,
}

/// Params of `session.messages`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MessagesParams {
    pub id: String,
    /// Return messages with `seq > after` (default 0).
    #[serde(default)]
    pub after: u64,
    /// 1-500, default [`DEFAULT_MESSAGE_LIMIT`]; pages are also cut to fit one message.
    #[serde(default = "default_message_limit")]
    pub limit: usize,
}

fn default_message_limit() -> usize {
    DEFAULT_MESSAGE_LIMIT
}

/// Params of `session.branch`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BranchParams {
    pub id: String,
    pub checkpoint_id: String,
}

/// Result of `session.list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionList {
    pub sessions: Vec<Session>,
}

/// Result of `session.checkpoints`: the newest [`MAX_LISTED`] records, oldest first.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckpointList {
    pub checkpoints: Vec<CheckpointMeta>,
    pub total: usize,
}

/// Result of `session.resume_plan` and `session.resume`: a [`ResumePlan`] without the
/// harness payload and message bodies, bounded to fit one IPC message.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ResumePlanView {
    pub session: Session,
    pub checkpoint: Option<CheckpointMeta>,
    /// Host the checkpoint was written on.
    pub checkpoint_host: Option<HostFingerprint>,
    pub recovered_from_corruption: bool,
    pub corrupt_checkpoints: Vec<CheckpointId>,
    /// Messages after the checkpoint; read them with `session.messages {after}`.
    pub messages_since_checkpoint: u64,
    /// `after` to pass to `session.messages` to read them.
    pub messages_after: u64,
    pub unrecoverable: Vec<UnrecoverableItem>,
    pub unrecoverable_total: usize,
    pub different_host: bool,
    pub host_changes: Vec<String>,
    pub event_gaps: Vec<EventGap>,
    pub event_gaps_total: usize,
    pub resumable: bool,
    pub blocked_reason: Option<String>,
    /// `true` only for `session.resume` once the session was adopted on this host.
    pub adopted: bool,
    /// Whether this daemon can continue the work itself (always `false` for now).
    pub continuation_available: bool,
    pub continuation: String,
}

impl ResumePlanView {
    pub fn from_plan(plan: ResumePlan, adopted: bool) -> Self {
        let messages_after = plan
            .checkpoint
            .as_ref()
            .map_or(0, |checkpoint| checkpoint.marks.message_seq);
        let unrecoverable_total = plan.unrecoverable.len();
        let event_gaps_total = plan.event_gaps.len();
        let (checkpoint, checkpoint_host) = match plan.checkpoint {
            Some(checkpoint) => (Some(checkpoint.meta), Some(checkpoint.host_fingerprint)),
            None => (None, None),
        };
        Self {
            session: plan.session,
            checkpoint,
            checkpoint_host,
            recovered_from_corruption: plan.recovered_from_corruption,
            corrupt_checkpoints: plan
                .corrupt_checkpoints
                .into_iter()
                .take(MAX_LISTED)
                .collect(),
            messages_since_checkpoint: plan.messages_since_checkpoint.len() as u64,
            messages_after,
            unrecoverable: plan.unrecoverable.into_iter().take(MAX_LISTED).collect(),
            unrecoverable_total,
            different_host: plan.different_host,
            host_changes: plan.host_changes,
            event_gaps: plan.event_gaps.into_iter().take(MAX_LISTED).collect(),
            event_gaps_total,
            resumable: plan.resumable,
            blocked_reason: plan.blocked_reason,
            adopted,
            continuation_available: false,
            continuation: CONTINUATION_UNAVAILABLE.into(),
        }
    }
}

/// Map a session error to its stable wire code with `data.reason`.
pub fn session_error(error: SessionError) -> RpcError {
    let reason = error.reason();
    RpcError::service(error.code(), error.to_string()).with_data(json!({ "reason": reason }))
}

fn exact_id(field: &str, id: &str) -> Result<EntityId, RpcError> {
    EntityId::new(id.trim()).map_err(|error| RpcError::invalid_params(format!("{field}: {error}")))
}

/// Drop trailing messages until the page fits one IPC message.
pub fn bounded_page(mut page: MessagePage) -> MessagePage {
    let encoded_len =
        |page: &MessagePage| serde_json::to_vec(page).map_or(usize::MAX, |bytes| bytes.len());
    while page.messages.len() > 1 && encoded_len(&page) > PAGE_BYTE_BUDGET {
        let keep = page.messages.len() / 2;
        page.messages.truncate(keep);
        page.has_more = true;
        if let Some(last) = page.messages.last() {
            page.next_after = last.seq;
        }
    }
    page
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(CREATE, move |_context, params| {
        let params: CreateParams = parse_params(params)?;
        let workspace = exact_id("workspace_id", &params.workspace_id)?;
        shared
            .workspaces()
            .get(&workspace)
            .map_err(workspace_error)?;
        let agent = exact_id(
            "agent_id",
            params.agent_id.as_deref().unwrap_or(DEFAULT_AGENT),
        )?;
        let session = shared
            .sessions()
            .create(NewSession::new(workspace, agent, params.title))
            .map_err(session_error)?;
        to_result(&session)
    })?;
    let shared = Arc::clone(services);
    router.register(LIST, move |_context, params| {
        let params: ListParams = parse_params(params)?;
        let filter = SessionFilter {
            workspace_id: params
                .workspace_id
                .as_deref()
                .map(|id| exact_id("workspace_id", id))
                .transpose()?,
            state: params.state,
            limit: params.limit,
        };
        let sessions = shared.sessions().list(&filter).map_err(session_error)?;
        to_result(&SessionList { sessions })
    })?;
    let shared = Arc::clone(services);
    router.register(SHOW, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let mut summary = shared
            .sessions()
            .summary(&exact_id("id", &params.id)?)
            .map_err(session_error)?;
        summary.event_gaps.truncate(MAX_LISTED);
        to_result(&summary)
    })?;
    let shared = Arc::clone(services);
    router.register(MESSAGES, move |_context, params| {
        let params: MessagesParams = parse_params(params)?;
        let page = shared
            .sessions()
            .messages(&exact_id("id", &params.id)?, params.after, params.limit)
            .map_err(session_error)?;
        to_result(&bounded_page(page))
    })?;
    let shared = Arc::clone(services);
    router.register(CHECKPOINTS, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let mut checkpoints = shared
            .sessions()
            .checkpoints(&exact_id("id", &params.id)?)
            .map_err(session_error)?;
        let total = checkpoints.len();
        let skip = total.saturating_sub(MAX_LISTED);
        checkpoints.drain(..skip);
        to_result(&CheckpointList { checkpoints, total })
    })?;
    let shared = Arc::clone(services);
    router.register(RESUME_PLAN, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let store = shared.sessions();
        let plan = store
            .resume_plan(&exact_id("id", &params.id)?, store.host())
            .map_err(session_error)?;
        to_result(&ResumePlanView::from_plan(plan, false))
    })?;
    let shared = Arc::clone(services);
    router.register(RESUME, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let id = exact_id("id", &params.id)?;
        let store = shared.sessions();
        // The plan is computed against the host the session last ran on, so host
        // changes are reported before the adoption records this host.
        let plan = store
            .resume_plan(&id, store.host())
            .map_err(session_error)?;
        if !plan.resumable {
            return Err(session_error(SessionError::InvalidTransition {
                from: plan.session.kind(),
                to: SessionStateKind::Paused,
            })
            .with_data(json!({
                "reason": "not_resumable",
                "blocked_reason": plan.blocked_reason,
            })));
        }
        let session = store.mark_resumed(&id).map_err(session_error)?;
        let mut view = ResumePlanView::from_plan(plan, true);
        view.session = session;
        to_result(&view)
    })?;
    let shared = Arc::clone(services);
    router.register(CANCEL, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let id = exact_id("id", &params.id)?;
        let session = shared
            .sessions()
            .transition(&id, SessionState::Cancelled)
            .map_err(session_error)?;
        // A cancelled session's running calls stop and its approvals and session
        // grants can no longer authorize anything.
        shared
            .broker()
            .cancel_session(&id, &aidex_broker::Principal::LocalUser)
            .map_err(|error| crate::methods::approvals::broker_error(&error))?;
        to_result(&session)
    })?;
    let shared = Arc::clone(services);
    router.register(BRANCH, move |_context, params| {
        let params: BranchParams = parse_params(params)?;
        let session = shared
            .sessions()
            .branch(
                &exact_id("id", &params.id)?,
                &exact_id("checkpoint_id", &params.checkpoint_id)?,
            )
            .map_err(session_error)?;
        to_result(&session)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        errors::ErrorCode,
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use aidex_sessions::{LogicalState, MessageRole, NewMessage};
    use serde_json::Value;

    fn fixture() -> (
        tempfile::TempDir,
        Arc<Services>,
        Router,
        CallContext,
        EntityId,
    ) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let workspace = services.workspaces().create_drive("main").unwrap().id;
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        (root, services, router, context, workspace)
    }

    fn call<T: serde::de::DeserializeOwned>(
        router: &Router,
        context: &CallContext,
        method: &str,
        params: Value,
    ) -> T {
        serde_json::from_value(router.call(context, method, params).unwrap()).unwrap()
    }

    #[test]
    fn create_requires_a_registered_workspace() {
        let (_root, _services, router, context, workspace) = fixture();
        let session: Session = call(
            &router,
            &context,
            CREATE,
            json!({"workspace_id": workspace.as_str(), "title": "first"}),
        );
        assert_eq!(session.agent_id.as_str(), DEFAULT_AGENT);
        let missing = router
            .call(
                &context,
                CREATE,
                json!({"workspace_id": "wsp_missing", "title": "x"}),
            )
            .unwrap_err();
        assert_eq!(missing.kind(), Some(ErrorCode::NotFound));
        assert_eq!(missing.data.unwrap()["reason"], "workspace_not_found");
        let list: SessionList = call(&router, &context, LIST, json!({"state": "created"}));
        assert_eq!(list.sessions, [session]);
    }

    #[test]
    fn params_are_closed_and_errors_carry_reasons() {
        let (_root, services, router, context, workspace) = fixture();
        for (method, params) in [
            (LIST, json!({"state": "sleeping"})),
            (SHOW, json!({"id": "bad id!"})),
            (SHOW, json!({"id": "ses_x", "extra": 1})),
            (BRANCH, json!({"id": "ses_x"})),
            (CREATE, json!({"workspace_id": workspace.as_str()})),
        ] {
            let error = router.call(&context, method, params.clone()).unwrap_err();
            assert_eq!(
                error.kind(),
                Some(ErrorCode::InvalidParams),
                "{method} {params}"
            );
        }
        let missing = router
            .call(&context, SHOW, json!({"id": "ses_missing"}))
            .unwrap_err();
        assert_eq!(missing.kind(), Some(ErrorCode::NotFound));
        assert_eq!(missing.data.unwrap()["reason"], "session_not_found");

        let session = services
            .sessions()
            .create(NewSession::new(
                workspace,
                EntityId::new("aidex").unwrap(),
                "t",
            ))
            .unwrap();
        let cancelled: Session = call(
            &router,
            &context,
            CANCEL,
            json!({"id": session.id.as_str()}),
        );
        assert_eq!(cancelled.state, SessionState::Cancelled);
        let again = router
            .call(&context, CANCEL, json!({"id": session.id.as_str()}))
            .unwrap_err();
        assert_eq!(again.kind(), Some(ErrorCode::Conflict));
        assert_eq!(again.data.unwrap()["reason"], "invalid_transition");
        let resume = router
            .call(&context, RESUME, json!({"id": session.id.as_str()}))
            .unwrap_err();
        assert_eq!(resume.data.unwrap()["reason"], "not_resumable");
    }

    #[test]
    fn show_messages_checkpoints_and_branch_round_trip() {
        let (_root, services, router, context, workspace) = fixture();
        let store = services.sessions();
        let session = store
            .create(NewSession::new(
                workspace,
                EntityId::new("aidex").unwrap(),
                "work",
            ))
            .unwrap();
        for index in 0..3 {
            store
                .append_message(
                    &session.id,
                    NewMessage::new(MessageRole::User, format!("m{index}")),
                )
                .unwrap();
        }
        let checkpoint = store
            .checkpoint(&session.id, Some("one"), LogicalState::new(json!({"a": 1})))
            .unwrap();
        let id = session.id.as_str();
        let summary: SessionSummary = call(&router, &context, SHOW, json!({"id": id}));
        assert_eq!((summary.messages, summary.checkpoints), (3, 1));
        assert_eq!(summary.latest_checkpoint.unwrap().id, checkpoint.id);
        let page: MessagePage = call(
            &router,
            &context,
            MESSAGES,
            json!({"id": id, "after": 1, "limit": 1}),
        );
        assert_eq!(page.messages[0].seq, 2);
        assert!(page.has_more);
        let list: CheckpointList = call(&router, &context, CHECKPOINTS, json!({"id": id}));
        assert_eq!(list.total, 1);
        let branch: Session = call(
            &router,
            &context,
            BRANCH,
            json!({"id": id, "checkpoint_id": checkpoint.id.as_str()}),
        );
        assert_eq!(branch.branched_from, Some(checkpoint.id));
        let plan: ResumePlanView = call(&router, &context, RESUME_PLAN, json!({"id": id}));
        assert!(!plan.resumable);
        assert!(!plan.continuation_available);
    }

    #[test]
    fn oversized_message_pages_are_cut_to_fit_one_message() {
        let messages = (1..=8)
            .map(|seq| aidex_sessions::Message {
                session_id: EntityId::new("ses_x").unwrap(),
                seq,
                role: MessageRole::User,
                content: "x".repeat(200 * 1024),
                tool_call_id: None,
                redacted: false,
                created_at: aidex_core::Timestamp::now(),
            })
            .collect();
        let page = bounded_page(MessagePage {
            messages,
            next_after: 8,
            has_more: false,
        });
        assert!(page.has_more);
        assert!(serde_json::to_vec(&page).unwrap().len() < MAX_MESSAGE_BYTES);
        assert_eq!(page.next_after, page.messages.last().unwrap().seq);
    }
}
