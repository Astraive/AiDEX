//! What a model reads of a tool result.
//!
//! A model reads a workspace tool's `ToolOutput` envelope (`summary`,
//! `content`, `diff`, `changes`, `warnings`) with the applied diff reduced to
//! its statistics (`diff_stats`: each file's path, kind, added and removed
//! lines, and whether it is binary): the unified text of a write, patch, or
//! delete shows the file's earlier lines, which a policy denying `fs.read`
//! keeps from the model, and it would grow every later request. The change
//! itself is in the call's arguments and the tool's `content`. History stores
//! this view as the tool result, and the complete output stays with the run
//! (`Run::tool_outputs`).
//!
//! Every tool result gets this view, whatever call it answers, before it is
//! wrapped as untrusted data or flattened: a result is never trusted to be
//! external by the name of a call that shares its id. An external tool's
//! output that happens to be such an envelope only loses its diff text.
use aidex_providers::{ChatMessage, Role};
use serde::Serialize;
use serde_json::{Map, Value};

/// The fields of a serialized `aidex_tools::ToolOutput`.
const ENVELOPE: [&str; 5] = ["summary", "content", "diff", "changes", "warnings"];

#[derive(Serialize)]
struct ModelView<'a> {
    summary: &'a Value,
    content: &'a Value,
    diff_stats: DiffStats,
    changes: &'a Value,
    warnings: &'a Value,
}

#[derive(Serialize)]
struct DiffStats {
    files: Vec<Map<String, Value>>,
    files_omitted: u64,
}

/// Per-file fields a model may read: no unified text, no digests.
const FILE_STATS: [&str; 5] = ["path", "kind", "additions", "deletions", "binary"];

/// `content` as a model reads it: a `ToolOutput` envelope with an applied
/// diff gets `diff_stats` in its place. `None` for anything else, which the
/// model reads unchanged.
pub(crate) fn model_tool_result(content: &str) -> Option<String> {
    let Ok(Value::Object(envelope)) = serde_json::from_str::<Value>(content) else {
        return None;
    };
    if envelope.len() != ENVELOPE.len() || !ENVELOPE.iter().all(|k| envelope.contains_key(*k)) {
        return None;
    }
    let diff = envelope.get("diff").filter(|diff| !diff.is_null())?;
    let files = diff
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .map(|file| {
                    FILE_STATS
                        .iter()
                        .filter_map(|key| Some(((*key).to_owned(), file.get(*key)?.clone())))
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default();
    let view = ModelView {
        summary: &envelope["summary"],
        content: &envelope["content"],
        diff_stats: DiffStats {
            files,
            files_omitted: diff
                .get("files_omitted")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        },
        changes: &envelope["changes"],
        warnings: &envelope["warnings"],
    };
    serde_json::to_string(&view).ok()
}

/// The content a model reads of `message`: the model view of a tool
/// result ([`model_tool_result`]), or `None` when it reads the message as
/// stored.
pub(crate) fn model_content(message: &ChatMessage) -> Option<String> {
    (message.role == Role::Tool)
        .then(|| model_tool_result(&message.content))
        .flatten()
}

/// Give every tool result in `messages` (a transcript, or its copy in a
/// request) the content a model reads ([`model_content`]).
pub(crate) fn tool_results_for_model(messages: &mut [ChatMessage]) {
    for message in messages {
        if let Some(view) = model_content(message) {
            message.content = view;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidex_providers::ToolCall;
    use serde_json::json;

    fn envelope(diff: Value) -> String {
        json!({
            "summary": "wrote notes.txt",
            "content": {"path": "notes.txt", "sha256": "abc", "additions": 1, "deletions": 1},
            "diff": diff,
            "changes": [{"path": "notes.txt", "change": "modified"}],
            "warnings": [],
        })
        .to_string()
    }

    fn applied() -> Value {
        json!({
            "files": [{
                "path": "notes.txt",
                "kind": "modified",
                "additions": 1,
                "deletions": 1,
                "unified": "--- a/notes.txt\n+++ b/notes.txt\n-EARLIER_SECRET\n+new\n",
                "binary": false,
                "truncated": false,
                "before_sha256": "before",
                "after_sha256": "after",
            }],
            "files_omitted": 2,
        })
    }

    #[test]
    fn an_applied_diff_reaches_the_model_as_statistics() {
        let view: Value =
            serde_json::from_str(&model_tool_result(&envelope(applied())).unwrap()).unwrap();
        assert_eq!(view["summary"], "wrote notes.txt");
        assert_eq!(view["content"]["sha256"], "abc");
        assert_eq!(view["changes"][0]["path"], "notes.txt");
        assert!(view.get("diff").is_none());
        assert_eq!(
            view["diff_stats"],
            json!({
                "files": [{
                    "path": "notes.txt",
                    "kind": "modified",
                    "additions": 1,
                    "deletions": 1,
                    "binary": false,
                }],
                "files_omitted": 2,
            })
        );
        let text = view.to_string();
        for hidden in ["EARLIER_SECRET", "unified", "before_sha256", "after"] {
            assert!(!text.contains(hidden), "{hidden} reached the model");
        }
    }

    #[test]
    fn other_results_are_unchanged() {
        // No applied diff, plain text, other JSON, or an envelope with an
        // extra field.
        assert_eq!(model_tool_result(&envelope(Value::Null)), None);
        assert_eq!(model_tool_result("Tool execution denied by user."), None);
        assert_eq!(model_tool_result(r#"{"is_error":true}"#), None);
        let mut extra: Value = serde_json::from_str(&envelope(applied())).unwrap();
        extra["more"] = json!(1);
        assert_eq!(model_tool_result(&extra.to_string()), None);
        // A malformed diff still never passes its text on.
        let view = model_tool_result(&envelope(json!({"unified": "EARLIER_SECRET"}))).unwrap();
        assert!(!view.contains("EARLIER_SECRET"));
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments: "{}".into(),
            provider_metadata: None,
        }
    }

    #[test]
    fn every_tool_result_reaches_the_model_in_its_view() {
        let mut assistant = ChatMessage::assistant("");
        // A workspace write and an MCP call sharing an id: the result that
        // answers the write is never passed on whole as the MCP tool's.
        assistant.tool_calls = vec![
            call("dup", "fs_write"),
            call("dup", "mcp__x__y"),
            call("c2", "mcp__x__diff"),
        ];
        let mut messages = vec![
            ChatMessage::user(envelope(applied())),
            assistant,
            ChatMessage::tool_result("dup", envelope(applied())),
            ChatMessage::tool_result("dup", r#"{"is_error":true}"#),
            ChatMessage::tool_result("c2", envelope(applied())),
            ChatMessage::tool_result("orphan", envelope(applied())),
        ];
        tool_results_for_model(&mut messages);
        for index in [2, 4, 5] {
            let content = &messages[index].content;
            assert!(!content.contains("EARLIER_SECRET"), "{index}: {content}");
            assert!(content.contains("diff_stats"), "{index}: {content}");
        }
        assert_eq!(messages[3].content, r#"{"is_error":true}"#);
        // Only tool results change.
        assert_eq!(messages[0].content, envelope(applied()));
        assert_eq!(model_content(&messages[0]), None);
        // The view is final: applying it again changes nothing.
        let viewed = messages.clone();
        tool_results_for_model(&mut messages);
        assert_eq!(messages, viewed);
    }
}
