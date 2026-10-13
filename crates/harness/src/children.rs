//! Persisted child results stay bound to their immediate parent. A native
//! parent that finishes a turn while its children still work waits for them
//! ([`NativeEngine::settle_children`]) and reads their results before it
//! completes.
use crate::{NativeEngine, native_tools::AGENT_RESULT, prompt::result_names, untrusted::bounded};
use aidex_core::CancelToken;
use aidex_native_sessions::{Run, RunStatus};
use aidex_providers::{ChatMessage, Role};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

/// Longest a parent waits for its active children.
const MAX_CHILD_WAIT: Duration = Duration::from_secs(10 * 60);
/// About the most bytes of child results one message carries.
const CHILD_RESULTS_BYTES: usize = 8 * 1024;
/// Room left in each child's share for its ids, status, and cut note.
const RESULT_FRAMING: usize = 192;
/// How often the run index is read while children work.
const POLL: Duration = Duration::from_millis(50);
/// First line of the transcript message that gives a parent its children's
/// results. The JSON list after it is stored as produced and wrapped as
/// untrusted data when the prompt is assembled.
pub(crate) const CHILD_RESULTS: &str = "[AiDEX child run results]";

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

/// The JSON list of a child-results message, if `content` is one.
pub(crate) fn child_results_body(content: &str) -> Option<&str> {
    content.strip_prefix(CHILD_RESULTS)?.strip_prefix('\n')
}

/// The final result of `child`, a run of `parent`.
fn result_of(parent: &str, child: &Run) -> ChildResult {
    let content = child
        .transcript
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && m.tool_calls.is_empty())
        .map(|m| m.content.clone())
        .unwrap_or_default();
    ChildResult {
        run_id: child.id.clone(),
        parent_run_id: parent.into(),
        status: child.status,
        content,
    }
}

/// Children whose final results `transcript` already holds: from an earlier
/// child-results message, or an `agent_result` call answered after the child
/// stopped.
fn acknowledged(transcript: &[ChatMessage]) -> HashSet<String> {
    transcript
        .iter()
        .zip(result_names(transcript))
        .filter_map(|(message, name)| match message.role {
            Role::User => child_results_body(&message.content)
                .and_then(|body| serde_json::from_str::<Vec<ChildResult>>(body).ok()),
            Role::Tool if name == Some(AGENT_RESULT) => {
                serde_json::from_str::<ChildResult>(&message.content)
                    .ok()
                    .filter(|result| !result.status.active())
                    .map(|result| vec![result])
            }
            _ => None,
        })
        .flatten()
        .map(|result| result.run_id)
        .collect()
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
        Ok(result_of(parent, &child))
    }

    /// Before the native run `parent` (of `session_id`) completes: wait for
    /// its active children, then add the results it has not read yet to its
    /// transcript as one message (each child's share of about 8 KiB; the
    /// complete results stay in the children's runs). `Ok(true)` when results
    /// were added, so the parent takes another turn with them.
    ///
    /// The wait ends with the parent's cancellation (an error) or after the
    /// time left on `cancel`, at most ten minutes; the parent then completes
    /// without waiting further and its children go on.
    pub(crate) fn settle_children(
        &self,
        parent: &str,
        session_id: &str,
        cancel: &CancelToken,
    ) -> Result<bool, String> {
        let wait = cancel
            .remaining()
            .map_or(MAX_CHILD_WAIT, |left| left.min(MAX_CHILD_WAIT));
        let deadline = Instant::now() + wait;
        loop {
            let session = self.store.get(session_id)?;
            let run = session
                .runs
                .iter()
                .find(|r| r.id == parent)
                .ok_or("run not found")?;
            let children: Vec<&Run> = session
                .runs
                .iter()
                .filter(|r| r.parent_run_id.as_deref() == Some(parent))
                .collect();
            let active: HashSet<&str> = children
                .iter()
                .filter(|child| child.status.active())
                .map(|child| child.id.as_str())
                .collect();
            if !active.is_empty() {
                if !self.wait_for(&active, cancel, deadline)? {
                    return Ok(false);
                }
                continue;
            }
            let seen = acknowledged(&run.transcript);
            let mut results: Vec<ChildResult> = children
                .iter()
                .filter(|child| !seen.contains(&child.id))
                .map(|child| result_of(parent, child))
                .collect();
            if results.is_empty() {
                return Ok(false);
            }
            let share = (CHILD_RESULTS_BYTES / results.len()).saturating_sub(RESULT_FRAMING);
            for result in &mut results {
                if result.content.len() > share {
                    result.content = format!(
                        "{}\n[cut; the complete result stays in the child run]",
                        bounded(&result.content, share)
                    );
                }
            }
            let message = ChatMessage::user(format!(
                "{CHILD_RESULTS}\n{}",
                serde_json::to_string(&results).map_err(|e| e.to_string())?
            ));
            // The parent's transcript only: the conversation keeps the
            // answer the parent writes from these results.
            self.store.update(session_id, |s| {
                let run = s
                    .runs
                    .iter_mut()
                    .find(|r| r.id == parent)
                    .ok_or("run not found")?;
                if !run.status.active() {
                    return Err("run is no longer active".into());
                }
                run.transcript.push(message.clone());
                Ok(())
            })?;
            return Ok(true);
        }
    }

    /// Wait until none of the runs in `active` is active. `Ok(false)` at
    /// `deadline`; an error once `cancel` is cancelled.
    fn wait_for(
        &self,
        active: &HashSet<&str>,
        cancel: &CancelToken,
        deadline: Instant,
    ) -> Result<bool, String> {
        loop {
            if cancel.is_cancelled() {
                return Err("run cancelled while waiting for its child runs".into());
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            let still = self
                .store
                .run_summaries()?
                .into_iter()
                .any(|run| active.contains(run.id.as_str()) && run.status.active());
            if !still {
                return Ok(true);
            }
            std::thread::sleep(POLL);
        }
    }
}
