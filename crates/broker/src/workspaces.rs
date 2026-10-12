//! How the broker finds a workspace's confined root.

use std::{collections::HashMap, sync::RwLock};

use aidex_core::{EntityId, ServiceErrorCode};
use aidex_workspace::{ConfinedRoot, WorkspaceRegistry};

use crate::BrokerError;

/// A workspace resolved for one tool call.
#[derive(Clone, Debug)]
pub struct ResolvedWorkspace {
    pub id: EntityId,
    /// Human label for approval cards.
    pub name: String,
    pub root: ConfinedRoot,
}

/// Resolves workspace ids to confined roots. Implemented for the daemon's
/// [`WorkspaceRegistry`]; [`StaticWorkspaces`] serves embedders and tests.
pub trait WorkspaceResolver: Send + Sync {
    fn resolve(&self, workspace_id: &EntityId) -> Result<ResolvedWorkspace, BrokerError>;
}

impl WorkspaceResolver for WorkspaceRegistry {
    fn resolve(&self, workspace_id: &EntityId) -> Result<ResolvedWorkspace, BrokerError> {
        let handle = self.handle(workspace_id)?;
        Ok(ResolvedWorkspace {
            id: workspace_id.clone(),
            name: handle.workspace().name.clone(),
            root: handle.root().clone(),
        })
    }
}

/// A fixed set of workspaces.
#[derive(Debug, Default)]
pub struct StaticWorkspaces {
    workspaces: RwLock<HashMap<EntityId, ResolvedWorkspace>>,
}

impl StaticWorkspaces {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, id: EntityId, name: impl Into<String>, root: ConfinedRoot) {
        let workspace = ResolvedWorkspace {
            id: id.clone(),
            name: name.into(),
            root,
        };
        self.workspaces
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id, workspace);
    }
}

impl WorkspaceResolver for StaticWorkspaces {
    fn resolve(&self, workspace_id: &EntityId) -> Result<ResolvedWorkspace, BrokerError> {
        self.workspaces
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(workspace_id)
            .cloned()
            .ok_or_else(|| BrokerError::Workspace {
                code: ServiceErrorCode::NotFound,
                reason: "not_found",
                message: format!("no workspace `{workspace_id}` is registered"),
            })
    }
}
