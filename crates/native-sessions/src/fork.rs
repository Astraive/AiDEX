//! Forking a conversation into a new one ("Continue in a new Agent chat").
use crate::*;
use aidex_providers::{ChatMessage, Role};
use std::collections::HashSet;

impl SessionStore {
    /// A new conversation with `request.configuration` whose history is a copy
    /// of `source_id`'s, and whose sent attachments are re-sealed under the
    /// new session (new ids, same message index and order). Message indices
    /// are kept: assistant tool calls that never got a result are stripped in
    /// place, and only trailing empty assistant placeholders are dropped. Runs,
    /// commands, checkpoints, approvals, artifacts, staged attachments and the
    /// external runtime thread are not copied. Refused while the source has an
    /// active run, and (`too_large`) when the attachments to re-seal exceed
    /// [`MAX_FORK_ATTACHMENT_BYTES`]. A retry with the same `command_id` and
    /// arguments returns the conversation the first call created.
    pub fn fork(&self, source_id: &str, request: ForkRequest) -> Result<SessionDetail, StoreError> {
        let ForkRequest {
            mut configuration,
            title,
            command_id,
        } = request;
        if let Some(title) = title {
            configuration.title = title;
        }
        configuration.validate()?;
        if command_id
            .as_deref()
            .is_some_and(|c| c.is_empty() || c.len() > 128)
        {
            return Err("invalid message or command id".into());
        }
        let cipher = self.attachment_cipher()?;
        let source = self.get(source_id)?;
        let command = command_id.map(|c| format!("fork:{c}"));
        let fingerprint = digest(
            format!(
                "fork\0{source_id}\0{}",
                serde_json::to_string(&configuration).map_err(|e| e.to_string())?
            )
            .as_bytes(),
        );
        if let Some(existing) = command.as_ref().and_then(|c| source.commands.get(c)) {
            if existing.fingerprint != fingerprint {
                return Err("command id reused with different arguments".into());
            }
            return self.get(&existing.run_id);
        }
        if source.runs.iter().any(|r| r.status.active()) {
            return Err("cancel active runs before forking the conversation".into());
        }
        let messages = fork_messages(&source.messages);
        let mut sent = self.sent_attachment_rows(&cipher, source_id)?;
        sent.retain(|(meta, _)| {
            meta.message_index
                .and_then(|index| messages.get(index))
                .is_some_and(|m| m.role == Role::User)
        });
        let total = sent
            .iter()
            .fold(0u64, |total, (meta, _)| total.saturating_add(meta.size));
        if total > MAX_FORK_ATTACHMENT_BYTES {
            return Err(AttachmentReason::TooLarge.error(format!(
                "a fork can copy at most {MAX_FORK_ATTACHMENT_BYTES} attachment bytes"
            )));
        }
        let detail = SessionDetail {
            id: id("session")?,
            configuration,
            pinned: false,
            created_at: now(),
            forked_from: Some(ForkOrigin {
                session_id: source_id.into(),
                message_count: messages.len(),
            }),
            task_graphs: vec![],
            messages,
            runs: vec![],
            approvals: vec![],
            checkpoints: vec![],
            commands: Default::default(),
            runtime_ref: None,
        };
        // Blobs are written before the transaction that references them.
        let mut copies = Vec::with_capacity(sent.len());
        let resealed = (|| {
            for (meta, blob) in &sent {
                let copy = AttachmentMeta {
                    id: id("att")?,
                    session_id: detail.id.clone(),
                    command_id: None,
                    run_id: None,
                    ..meta.clone()
                };
                copies.push(self.reseal_attachment(&cipher, meta, blob.as_deref(), copy)?);
            }
            Ok::<_, StoreError>(())
        })();
        // The source is checked again inside the transaction: a run may have
        // started, or a concurrent retry of the same command may have won.
        let committed = resealed.and_then(|()| {
            let mut db = self.db.lock().map_err(|_| "session store poisoned")?;
            let tx = db.transaction().map_err(|e| e.to_string())?;
            let mut source = self.load(&tx, source_id)?;
            if let Some(existing) = command.as_ref().and_then(|c| source.commands.get(c)) {
                if existing.fingerprint != fingerprint {
                    return Err("command id reused with different arguments".into());
                }
                return Ok(Some(existing.run_id.clone()));
            }
            if source.runs.iter().any(|r| r.status.active()) {
                return Err("cancel active runs before forking the conversation".into());
            }
            if let Some(command) = &command {
                source.commands.insert(
                    command.clone(),
                    CommandRecord {
                        fingerprint,
                        run_id: detail.id.clone(),
                    },
                );
                self.save(&tx, &source)?;
            }
            self.save(&tx, &detail)?;
            Self::insert_copies(&tx, &cipher, &copies)?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(None)
        });
        if !matches!(committed, Ok(None)) {
            for blob in copies.iter().filter_map(|copy| copy.blob()) {
                self.attachment_blobs.remove(blob);
            }
        }
        match committed? {
            None => Ok(detail),
            Some(existing) => self.get(&existing),
        }
    }
}

/// The source history with assistant tool calls that never got a result
/// stripped in place, and trailing empty assistant placeholders dropped. No
/// other message is removed, so every message keeps its index.
pub(crate) fn fork_messages(source: &[ChatMessage]) -> Vec<ChatMessage> {
    let answered: HashSet<&str> = source
        .iter()
        .filter(|m| m.role == Role::Tool)
        .filter_map(|m| m.tool_call_id.as_deref())
        .collect();
    let mut messages: Vec<ChatMessage> = source
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if m.role == Role::Assistant {
                m.tool_calls
                    .retain(|call| answered.contains(call.id.as_str()));
            }
            m
        })
        .collect();
    while messages.last().is_some_and(|m| {
        m.role == Role::Assistant && m.content.trim().is_empty() && m.tool_calls.is_empty()
    }) {
        messages.pop();
    }
    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidex_providers::ToolCall;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "web_search".into(),
            arguments: "{}".into(),
            provider_metadata: None,
        }
    }

    #[test]
    fn fork_history_keeps_indices_and_strips_unanswered_calls() {
        let mut asked = ChatMessage::assistant("");
        asked.tool_calls = vec![call("a"), call("b")];
        let mut pending = ChatMessage::assistant("checking");
        pending.tool_calls = vec![call("c")];
        let source = vec![
            ChatMessage::user("one"),
            asked,
            ChatMessage::tool_result("a", "result"),
            ChatMessage::assistant(""),
            ChatMessage::user("two"),
            pending,
            ChatMessage::user("three"),
            ChatMessage::assistant(""),
            ChatMessage::assistant(" "),
        ];
        let forked = fork_messages(&source);
        assert_eq!(forked.len(), 7, "only trailing placeholders are dropped");
        assert_eq!(forked[1].tool_calls, vec![call("a")]);
        assert_eq!(
            forked[3].content, "",
            "a mid-history placeholder keeps its slot"
        );
        assert!(forked[5].tool_calls.is_empty());
        assert_eq!(forked[5].content, "checking");
        for (index, message) in forked.iter().enumerate() {
            assert_eq!(message.role, source[index].role);
            assert_eq!(message.content, source[index].content);
        }
    }
}
