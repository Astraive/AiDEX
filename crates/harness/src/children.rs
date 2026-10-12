//! Persisted child results stay bound to their immediate parent.
use crate::NativeEngine;
use aidex_native_sessions::RunStatus;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChildResult {
    pub run_id: String,
    pub parent_run_id: String,
    pub status: RunStatus,
    pub content: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SpawnArgs {
    pub prompt: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResultArgs {
    pub run_id: String,
}
impl NativeEngine {
    pub fn child_result(&self, parent: &str, child: &str) -> Result<ChildResult, String> {
        let child = self.store.run(child)?;
        if child.parent_run_id.as_deref() != Some(parent) {
            return Err("child result belongs to a different parent".into());
        }
        let parent_run = self.store.run(parent)?;
        if parent_run.session_id != child.session_id {
            return Err("child result scope mismatch".into());
        }
        let content = child
            .transcript
            .iter()
            .rev()
            .find(|m| m.role == aidex_providers::Role::Assistant && m.tool_calls.is_empty())
            .map(|m| m.content.clone())
            .unwrap_or_default();
        Ok(ChildResult {
            run_id: child.id,
            parent_run_id: parent.into(),
            status: child.status,
            content,
        })
    }
}
