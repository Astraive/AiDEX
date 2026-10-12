//! `workspace.*` methods over the `aidex-workspace` registry.
//!
//! Non-destructive lookups (`get`, `select`, `ephemeral.create`) accept a workspace
//! id or its unique name; destructive or state-changing operations on an existing
//! workspace (`remove`, `ephemeral.review|apply|discard`) require the exact id.
//! Domain errors carry `data.reason` (see `WorkspaceError::reason`).

use crate::{
    errors::RpcError,
    methods::record_event,
    router::{Router, RouterError},
    rpc::{MAX_MESSAGE_BYTES, expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_core::{EntityId, WorkspacePath};
use aidex_events::{EventKind, EventScope};
use aidex_workspace::{
    ApplyReport, ApplySelection, Discovery, EphemeralCreated, EphemeralDiff, FileChange,
    SkippedEntry, Workspace, WorkspaceError, discover,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{path::Path, sync::Arc};

pub const ADD: &str = "workspace.add";
pub const CREATE_DRIVE: &str = "workspace.create_drive";
pub const LIST: &str = "workspace.list";
pub const GET: &str = "workspace.get";
pub const SELECT: &str = "workspace.select";
pub const SELECTED: &str = "workspace.selected";
pub const REMOVE: &str = "workspace.remove";
pub const DISCOVER: &str = "workspace.discover";
pub const EPHEMERAL_CREATE: &str = "workspace.ephemeral.create";
pub const EPHEMERAL_REVIEW: &str = "workspace.ephemeral.review";
pub const EPHEMERAL_APPLY: &str = "workspace.ephemeral.apply";
pub const EPHEMERAL_DISCARD: &str = "workspace.ephemeral.discard";

/// Maximum changes (and conflicts / unsupported entries) listed in one review result.
pub const MAX_REVIEW_ENTRIES: usize = 1000;
/// Room left in a response line for the JSON-RPC envelope.
const RESPONSE_HEADROOM: usize = 64 * 1024;

/// Params of `workspace.add`: an absolute host directory the user selected.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AddParams {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Params of `workspace.create_drive`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateDriveParams {
    pub name: String,
}

/// Params of `workspace.get` (id or name), `workspace.ephemeral.review`, and
/// `workspace.ephemeral.discard` (exact id).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IdParams {
    pub id: String,
}

/// Params of `workspace.select`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectParams {
    pub id_or_name: String,
}

/// Params of `workspace.remove`. `delete_data` also deletes an AiDEX-created
/// (drive or ephemeral) workspace's directory; host data is never deleted.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoveParams {
    pub id: String,
    #[serde(default)]
    pub delete_data: bool,
}

/// Params of `workspace.discover`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DiscoverParams {
    pub path: String,
}

/// Params of `workspace.ephemeral.create`: the source workspace id or name.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EphemeralCreateParams {
    pub source: String,
}

/// Params of `workspace.ephemeral.apply`. Omitted `paths` applies every
/// non-conflicting change; `review_digest` (from a review) refuses the apply when the
/// workspace changed since that review.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApplyParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<WorkspacePath>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_digest: Option<String>,
}

/// Result of `workspace.list`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceList {
    pub workspaces: Vec<Workspace>,
    pub selected: Option<EntityId>,
}

/// Result of `workspace.selected`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SelectedWorkspace {
    pub workspace: Option<Workspace>,
}

/// Result of `workspace.remove`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Removed {
    pub workspace: Workspace,
    pub deleted_data: bool,
}

/// Result of `workspace.ephemeral.discard`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Discarded {
    pub workspace: Workspace,
}

/// Result of `workspace.ephemeral.review`: an [`EphemeralDiff`] bounded to fit one
/// IPC message. `digest` always covers every change, listed or not.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EphemeralReview {
    pub workspace_id: EntityId,
    pub source_id: EntityId,
    pub changes: Vec<FileChange>,
    pub conflicts: Vec<WorkspacePath>,
    pub unsupported: Vec<SkippedEntry>,
    pub digest: String,
    pub total_changes: usize,
    pub total_conflicts: usize,
    /// Some entries were omitted (see the totals); diffs may have been dropped to fit.
    pub truncated: bool,
}

/// Map a workspace error to its stable wire code with `data.reason`.
pub fn workspace_error(error: WorkspaceError) -> RpcError {
    let reason = error.reason();
    RpcError::service(error.code(), error.to_string()).with_data(json!({ "reason": reason }))
}

fn exact_id(id: &str) -> Result<EntityId, RpcError> {
    EntityId::new(id.trim()).map_err(|error| RpcError::invalid_params(format!("id: {error}")))
}

fn host_path(path: &str) -> Result<&Path, RpcError> {
    if path.is_empty() {
        return Err(RpcError::invalid_params("path must not be empty"));
    }
    Ok(Path::new(path))
}

/// The event-catalog kind of a workspace.
fn event_kind(kind: &aidex_workspace::WorkspaceKind) -> aidex_events::WorkspaceKind {
    match kind {
        aidex_workspace::WorkspaceKind::Host => aidex_events::WorkspaceKind::Host,
        aidex_workspace::WorkspaceKind::Drive => aidex_events::WorkspaceKind::Drive,
        aidex_workspace::WorkspaceKind::Ephemeral { .. } => aidex_events::WorkspaceKind::Ephemeral,
    }
}

/// `workspace.added`, scoped to the new workspace.
fn record_added(services: &Services, workspace: &Workspace) -> Result<(), RpcError> {
    record_event(
        services,
        EventScope::workspace(workspace.id.clone()),
        EventKind::WorkspaceAdded {
            workspace_id: workspace.id.clone(),
            kind: event_kind(&workspace.kind),
            name: workspace.name.clone(),
        },
    )
}

/// Bound a review so the response fits one message line.
pub fn bounded_review(diff: EphemeralDiff) -> EphemeralReview {
    let total_changes = diff.changes.len();
    let total_conflicts = diff.conflicts.len();
    let mut review = EphemeralReview {
        truncated: total_changes > MAX_REVIEW_ENTRIES
            || total_conflicts > MAX_REVIEW_ENTRIES
            || diff.unsupported.len() > MAX_REVIEW_ENTRIES,
        workspace_id: diff.workspace_id,
        source_id: diff.source_id,
        changes: diff.changes.into_iter().take(MAX_REVIEW_ENTRIES).collect(),
        conflicts: diff
            .conflicts
            .into_iter()
            .take(MAX_REVIEW_ENTRIES)
            .collect(),
        unsupported: diff
            .unsupported
            .into_iter()
            .take(MAX_REVIEW_ENTRIES)
            .collect(),
        digest: diff.digest,
        total_changes,
        total_conflicts,
    };
    let limit = MAX_MESSAGE_BYTES - RESPONSE_HEADROOM;
    let encoded_len = |review: &EphemeralReview| {
        serde_json::to_vec(review).map_or(usize::MAX, |bytes| bytes.len())
    };
    if encoded_len(&review) > limit {
        review.truncated = true;
        for change in &mut review.changes {
            if change.unified_diff.take().is_some() {
                change.diff_truncated = true;
            }
        }
    }
    while encoded_len(&review) > limit
        && (!review.changes.is_empty()
            || !review.unsupported.is_empty()
            || !review.conflicts.is_empty())
    {
        review.truncated = true;
        review.changes.truncate(review.changes.len() / 2);
        review.unsupported.truncate(review.unsupported.len() / 2);
        review.conflicts.truncate(review.conflicts.len() / 2);
    }
    review
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let shared = Arc::clone(services);
    router.register(ADD, move |_context, params| {
        let params: AddParams = parse_params(params)?;
        let workspace = shared
            .workspaces()
            .add_host(host_path(&params.path)?, params.name.as_deref())
            .map_err(workspace_error)?;
        record_added(&shared, &workspace)?;
        to_result(&workspace)
    })?;
    let shared = Arc::clone(services);
    router.register(CREATE_DRIVE, move |_context, params| {
        let params: CreateDriveParams = parse_params(params)?;
        let workspace = shared
            .workspaces()
            .create_drive(&params.name)
            .map_err(workspace_error)?;
        record_added(&shared, &workspace)?;
        to_result(&workspace)
    })?;
    let shared = Arc::clone(services);
    router.register(LIST, move |_context, params| {
        expect_no_params(&params)?;
        let registry = shared.workspaces();
        let workspaces = registry.list().map_err(workspace_error)?;
        let selected = registry
            .selected()
            .map_err(workspace_error)?
            .map(|workspace| workspace.id);
        to_result(&WorkspaceList {
            workspaces,
            selected,
        })
    })?;
    let shared = Arc::clone(services);
    router.register(GET, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let workspace = shared
            .workspaces()
            .find(&params.id)
            .map_err(workspace_error)?;
        to_result(&workspace)
    })?;
    let shared = Arc::clone(services);
    router.register(SELECT, move |_context, params| {
        let params: SelectParams = parse_params(params)?;
        let registry = shared.workspaces();
        let found = registry.find(&params.id_or_name).map_err(workspace_error)?;
        let workspace = registry.select(&found.id).map_err(workspace_error)?;
        record_event(
            &shared,
            EventScope::workspace(workspace.id.clone()),
            EventKind::WorkspaceSelected {
                workspace_id: workspace.id.clone(),
            },
        )?;
        to_result(&workspace)
    })?;
    let shared = Arc::clone(services);
    router.register(SELECTED, move |_context, params| {
        expect_no_params(&params)?;
        let workspace = shared.workspaces().selected().map_err(workspace_error)?;
        to_result(&SelectedWorkspace { workspace })
    })?;
    let shared = Arc::clone(services);
    router.register(REMOVE, move |_context, params| {
        let params: RemoveParams = parse_params(params)?;
        let id = exact_id(&params.id)?;
        let registry = shared.workspaces();
        let workspace = if params.delete_data {
            registry.delete_data(&id)
        } else {
            registry.remove(&id)
        }
        .map_err(workspace_error)?;
        record_event(
            &shared,
            EventScope::workspace(workspace.id.clone()),
            EventKind::WorkspaceRemoved {
                workspace_id: workspace.id.clone(),
            },
        )?;
        to_result(&Removed {
            workspace,
            deleted_data: params.delete_data,
        })
    })?;
    router.register(DISCOVER, |_context, params| {
        let params: DiscoverParams = parse_params(params)?;
        let discovery: Discovery = discover(host_path(&params.path)?).map_err(workspace_error)?;
        to_result(&discovery)
    })?;
    let shared = Arc::clone(services);
    router.register(EPHEMERAL_CREATE, move |_context, params| {
        let params: EphemeralCreateParams = parse_params(params)?;
        let registry = shared.workspaces();
        let source = registry.find(&params.source).map_err(workspace_error)?;
        let created: EphemeralCreated = registry
            .create_ephemeral(&source.id)
            .map_err(workspace_error)?;
        record_event(
            &shared,
            EventScope::workspace(created.workspace.id.clone()),
            EventKind::WorkspaceEphemeralCreated {
                workspace_id: created.workspace.id.clone(),
                source_workspace_id: source.id.clone(),
            },
        )?;
        to_result(&created)
    })?;
    let shared = Arc::clone(services);
    router.register(EPHEMERAL_REVIEW, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let diff = shared
            .workspaces()
            .review(&exact_id(&params.id)?)
            .map_err(workspace_error)?;
        to_result(&bounded_review(diff))
    })?;
    let shared = Arc::clone(services);
    router.register(EPHEMERAL_APPLY, move |_context, params| {
        let params: ApplyParams = parse_params(params)?;
        let selection = match params.paths {
            None => ApplySelection::All,
            Some(paths) => ApplySelection::Paths(paths),
        };
        let id = exact_id(&params.id)?;
        let report: ApplyReport = shared
            .workspaces()
            .apply(&id, &selection, params.review_digest.as_deref())
            .map_err(workspace_error)?;
        record_event(
            &shared,
            EventScope::workspace(id.clone()),
            EventKind::WorkspaceEphemeralApplied {
                workspace_id: id,
                files_changed: report.applied.len() as u64,
            },
        )?;
        to_result(&report)
    })?;
    let shared = Arc::clone(services);
    router.register(EPHEMERAL_DISCARD, move |_context, params| {
        let params: IdParams = parse_params(params)?;
        let workspace = shared
            .workspaces()
            .discard(&exact_id(&params.id)?)
            .map_err(workspace_error)?;
        record_event(
            &shared,
            EventScope::workspace(workspace.id.clone()),
            EventKind::WorkspaceEphemeralDiscarded {
                workspace_id: workspace.id.clone(),
            },
        )?;
        to_result(&Discarded { workspace })
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
    use serde_json::Value;

    fn fixture() -> (tempfile::TempDir, Router, CallContext) {
        let root = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(root.path(), shutdown.clone()).unwrap());
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        (root, router, context)
    }

    #[test]
    fn add_select_and_list_round_trip() {
        let (_root, router, context) = fixture();
        let project = tempfile::tempdir().unwrap();
        let added: Workspace = serde_json::from_value(
            router
                .call(
                    &context,
                    ADD,
                    json!({"path": project.path().to_str().unwrap(), "name": "proj"}),
                )
                .unwrap(),
        )
        .unwrap();
        let selected: Workspace = serde_json::from_value(
            router
                .call(&context, SELECT, json!({"id_or_name": "proj"}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(selected.id, added.id);
        let list: WorkspaceList =
            serde_json::from_value(router.call(&context, LIST, Value::Null).unwrap()).unwrap();
        assert_eq!(list.selected, Some(added.id.clone()));
        assert_eq!(list.workspaces.len(), 1);
        let current: SelectedWorkspace =
            serde_json::from_value(router.call(&context, SELECTED, json!({})).unwrap()).unwrap();
        assert_eq!(current.workspace.unwrap().id, added.id);
    }

    #[test]
    fn domain_errors_keep_their_code_and_reason() {
        let (_root, router, context) = fixture();
        let error = router
            .call(&context, ADD, json!({"path": "relative/path"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidArgument));
        assert_eq!(error.data.unwrap()["reason"], "invalid_path");
        let error = router
            .call(&context, GET, json!({"id": "wsp_missing"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::NotFound));
        assert_eq!(error.data.unwrap()["reason"], "workspace_not_found");
        let error = router
            .call(&context, REMOVE, json!({"id": "bad id!"}))
            .unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidParams));
    }

    #[test]
    fn params_are_closed_schemas() {
        let (_root, router, context) = fixture();
        for (method, params) in [
            (ADD, json!({"path": "/x", "extra": true})),
            (ADD, json!({})),
            (SELECT, json!({"id": "x"})),
            (LIST, json!({"unexpected": 1})),
            (
                EPHEMERAL_APPLY,
                json!({"id": "wsp_x", "paths": ["../escape"]}),
            ),
            (EPHEMERAL_APPLY, json!({"id": "wsp_x", "paths": ["C:/abs"]})),
        ] {
            let error = router.call(&context, method, params.clone()).unwrap_err();
            assert_eq!(
                error.kind(),
                Some(ErrorCode::InvalidParams),
                "{method} {params}"
            );
        }
    }

    #[test]
    fn oversized_reviews_are_bounded_to_one_message() {
        let change = |index: usize| FileChange {
            path: WorkspacePath::new(format!("file{index:05}.txt")).unwrap(),
            kind: aidex_workspace::ChangeKind::Added,
            binary: false,
            size: Some(1),
            sha256: Some("0".repeat(64)),
            unified_diff: Some("\u{1}".repeat(60 * 1024)),
            diff_truncated: false,
            conflict: None,
        };
        let diff = EphemeralDiff {
            workspace_id: EntityId::new("wsp_a").unwrap(),
            source_id: EntityId::new("wsp_b").unwrap(),
            changes: (0..1500).map(change).collect(),
            conflicts: Vec::new(),
            unsupported: Vec::new(),
            digest: "d".into(),
        };
        let review = bounded_review(diff);
        assert!(review.truncated);
        assert_eq!(review.total_changes, 1500);
        assert!(review.changes.len() <= MAX_REVIEW_ENTRIES);
        assert!(serde_json::to_vec(&review).unwrap().len() < MAX_MESSAGE_BYTES);
        assert_eq!(review.digest, "d");
    }

    #[test]
    fn reviews_with_only_unsupported_or_conflicting_paths_fit_one_response() {
        for (with_conflicts, with_unsupported) in [(false, true), (true, false), (true, true)] {
            let paths: Vec<_> = (0..MAX_REVIEW_ENTRIES)
                .map(|index| {
                    WorkspacePath::new(format!("{}file{index}", "nested/".repeat(400))).unwrap()
                })
                .collect();
            let diff = EphemeralDiff {
                workspace_id: EntityId::new("wsp_a").unwrap(),
                source_id: EntityId::new("wsp_b").unwrap(),
                changes: Vec::new(),
                conflicts: if with_conflicts {
                    paths.clone()
                } else {
                    Vec::new()
                },
                unsupported: if with_unsupported {
                    paths
                        .iter()
                        .map(|path| SkippedEntry {
                            path: path.as_str().into(),
                            reason: aidex_workspace::SkipReason::Link,
                        })
                        .collect()
                } else {
                    Vec::new()
                },
                digest: "unchanged-full-review-digest".into(),
            };
            let total_conflicts = diff.conflicts.len();
            let review = bounded_review(diff);
            assert!(review.truncated);
            assert!(review.changes.is_empty());
            assert_eq!(review.total_changes, 0);
            assert_eq!(review.total_conflicts, total_conflicts);
            assert_eq!(review.digest, "unchanged-full-review-digest");
            assert!(
                serde_json::to_vec(&review).unwrap().len() <= MAX_MESSAGE_BYTES - RESPONSE_HEADROOM
            );
            let response = json!({"jsonrpc": "2.0", "id": "review", "result": review});
            assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_MESSAGE_BYTES);
        }
    }

    #[test]
    fn trimming_continues_after_the_last_change_is_removed() {
        let paths: Vec<_> = (0..MAX_REVIEW_ENTRIES)
            .map(|index| {
                WorkspacePath::new(format!("{}file{index}", "nested/".repeat(400))).unwrap()
            })
            .collect();
        let diff = EphemeralDiff {
            workspace_id: EntityId::new("wsp_a").unwrap(),
            source_id: EntityId::new("wsp_b").unwrap(),
            changes: vec![FileChange {
                path: WorkspacePath::new("small.txt").unwrap(),
                kind: aidex_workspace::ChangeKind::Modified,
                binary: false,
                size: Some(1),
                sha256: Some("0".repeat(64)),
                unified_diff: None,
                diff_truncated: false,
                conflict: Some(aidex_workspace::ConflictReason::SourceChanged),
            }],
            conflicts: paths.clone(),
            unsupported: paths
                .iter()
                .map(|path| SkippedEntry {
                    path: path.as_str().into(),
                    reason: aidex_workspace::SkipReason::Link,
                })
                .collect(),
            digest: "full-digest".into(),
        };
        let review = bounded_review(diff);
        assert!(review.truncated);
        assert!(review.changes.is_empty());
        assert!(!review.conflicts.is_empty());
        assert!(!review.unsupported.is_empty());
        assert_eq!(review.total_changes, 1);
        assert_eq!(review.total_conflicts, MAX_REVIEW_ENTRIES);
        assert_eq!(review.digest, "full-digest");
        assert!(
            serde_json::to_vec(&review).unwrap().len() <= MAX_MESSAGE_BYTES - RESPONSE_HEADROOM
        );
    }
}
