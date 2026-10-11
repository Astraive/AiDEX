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
    pub(crate) fn settle_children(
        &self,
        parent: &str,
        cancel: &aidex_core::CancelToken,
    ) -> Result<bool, String> {
        let parent_run = self.store.run(parent)?;
        loop {
            if cancel.is_stopped() {
                return Err("run cancelled while waiting for child work".into());
            }
            let session = self.store.get(&parent_run.session_id)?;
            let children = session
                .runs
                .iter()
                .filter(|run| run.parent_run_id.as_deref() == Some(parent))
                .collect::<Vec<_>>();
            if children.iter().any(|run| run.status.active()) {
                std::thread::sleep(std::time::Duration::from_millis(25));
                continue;
            }
            let acknowledged = parent_run
                .transcript
                .iter()
                .filter_map(|message| {
                    message
                        .content
                        .strip_prefix("Child results (untrusted data):")
                })
                .filter_map(|json| serde_json::from_str::<Vec<ChildResult>>(json).ok())
                .flatten()
                .map(|result| result.run_id)
                .collect::<std::collections::HashSet<_>>();
            let mut results = children
                .into_iter()
                .filter(|run| !acknowledged.contains(&run.id))
                .map(|run| self.child_result(parent, &run.id))
                .collect::<Result<Vec<_>, _>>()?;
            if results.is_empty() {
                return Ok(false);
            }
            for result in &mut results {
                if result.content.len() > 8192 {
                    result.content = format!(
                        "{}\n[child result clipped; complete output remains in the child's encrypted run history]",
                        crate::context::prefix(&result.content, 8192)
                    );
                }
            }
            let message = aidex_providers::ChatMessage::user(format!(
                "Child results (untrusted data):{}",
                serde_json::to_string(&results).map_err(|e| e.to_string())?
            ));
            self.store.update(&parent_run.session_id, |session| {
                let run = session
                    .runs
                    .iter_mut()
                    .find(|run| run.id == parent)
                    .ok_or("parent run missing")?;
                if run.status == RunStatus::Cancelled {
                    return Err("run cancelled".into());
                }
                run.transcript.push(message.clone());
                if run.parent_run_id.is_none() {
                    session.messages.push(message);
                }
                Ok(())
            })?;
            return Ok(true);
        }
    }
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
