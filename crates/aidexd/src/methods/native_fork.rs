//! `conversation.fork`: continue a conversation in a new one ("Continue in a
//! new Agent chat").
use crate::{
    errors::RpcError,
    methods::{
        attachments::attachment_error,
        native::{native_error, validate_workspace},
        native_view as view,
    },
    router::{Router, RouterError},
    rpc::parse_params,
    services::Services,
};
use aidex_native_sessions::{ForkRequest, SessionConfig};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkParams {
    source_id: String,
    /// Configuration of the new conversation, validated like
    /// `conversation.create`.
    configuration: SessionConfig,
    /// Replaces the configuration's title when present.
    #[serde(default)]
    title: Option<String>,
    /// Makes a retried fork return the conversation the first call created
    /// (1-128 bytes).
    #[serde(default)]
    command_id: Option<String>,
}

pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register("conversation.fork", move |_, p| {
        let p: ForkParams = parse_params(p)?;
        if p.command_id
            .as_deref()
            .is_some_and(|id| id.is_empty() || id.len() > 128)
        {
            return Err(RpcError::invalid_params("command_id must be 1-128 bytes"));
        }
        let mut configuration = p.configuration;
        if let Some(title) = p.title {
            configuration.title = title;
        }
        if configuration.title.trim().is_empty() {
            configuration.title = "New conversation".into();
        }
        configuration.validate().map_err(|error| {
            RpcError::invalid_params(aidex_events::redact_text(&error).into_owned())
        })?;
        validate_workspace(&s, &configuration)?;
        super::runtime::ensure_session_runtime(&s, &configuration);
        let store = s.native().store();
        let detail = store
            .fork(
                &p.source_id,
                ForkRequest {
                    configuration,
                    title: None,
                    command_id: p.command_id,
                },
            )
            .map_err(attachment_error)?;
        let attachments = store
            .message_attachments(&detail.id)
            .map_err(native_error)?;
        view::session(
            &detail,
            None,
            None,
            &view::MessageExtras::of(&detail, attachments),
        )
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
    use aidex_native_sessions::{NewAttachment, RunOptions, RunStatus};
    use aidex_providers::ChatMessage;
    use aidex_vault::SecretString;
    use serde_json::{Value, json};
    use zeroize::Zeroizing;

    #[test]
    fn fork_copies_history_and_attachments_into_a_new_agent_conversation() {
        let drive = crate::services::testing::temp_drive();
        let ws = tempfile::tempdir().unwrap();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        services
            .vault()
            .init(&SecretString::from("fixture-passphrase".to_owned()), false)
            .unwrap();
        let workspace = services.workspaces().add_host(ws.path(), None).unwrap();
        let mut router = Router::new();
        crate::methods::native::register(&mut router, &services).unwrap();
        crate::methods::attachments::register(&mut router, &services).unwrap();
        let ctx = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        let call = |method: &str, params: Value| router.call(&ctx, method, params);
        let store = services.native().store();
        let source = call(
            "conversation.create",
            json!({"title":"research","mode":"chat","provider_id":"fixture","model_id":"small"}),
        )
        .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let attachment = store
            .create_attachment(
                &source,
                NewAttachment {
                    name: "brief.txt".into(),
                    mime: None,
                    source: None,
                    bytes: Zeroizing::new(b"the brief".to_vec()),
                },
            )
            .unwrap();
        let run = store
            .begin_with(
                &source,
                "read this",
                "first",
                None,
                &RunOptions {
                    attachments: vec![attachment.id.clone()],
                    ..RunOptions::default()
                },
            )
            .unwrap();
        store
            .update(&source, |s| {
                s.messages.push(ChatMessage::assistant("done"));
                s.runs
                    .iter_mut()
                    .for_each(|r| r.status = RunStatus::Completed);
                Ok(())
            })
            .unwrap();
        let configuration = json!({"title":"","mode":"agent","provider_id":"fixture","model_id":"small","workspace_id":workspace.id,"permission_mode":"auto_accept_edits"});
        let fork = |extra: Value| {
            let mut params = json!({"source_id": source, "configuration": configuration});
            for (key, value) in extra.as_object().unwrap() {
                params[key] = value.clone();
            }
            call("conversation.fork", params)
        };

        let forked = fork(json!({"title": "research (agent)", "command_id": "fork-1"})).unwrap();
        let id = forked["id"].as_str().unwrap().to_owned();
        assert_ne!(id, source);
        assert_eq!(forked["title"], "research (agent)");
        assert_eq!(forked["mode"], "agent");
        assert_eq!(forked["workspace_id"], json!(workspace.id));
        assert_eq!(forked["permission_mode"], "auto_accept_edits");
        assert_eq!(
            forked["forked_from"],
            json!({"session_id": source, "message_count": 2})
        );
        assert_eq!(forked["total_messages"], 2);
        assert_eq!(forked["messages"][0]["content"], "read this");
        assert_eq!(forked["messages"][1]["content"], "done");
        assert_eq!(forked["total_runs"], 0, "runs are not copied");
        let copy = &forked["messages"][0]["attachments"][0];
        assert_eq!(copy["name"], "brief.txt");
        assert_ne!(copy["id"], attachment.id.as_str(), "copies get new ids");
        let copy = call("attachment.get", json!({"id": copy["id"]})).unwrap();
        assert_eq!(copy["session_id"], id.as_str());
        assert_eq!(copy["state"], "sent");
        assert_eq!(copy["message_index"], 0);
        assert!(copy.get("run_id").is_none());
        let original = call("attachment.get", json!({"id": attachment.id})).unwrap();
        assert_eq!(original["session_id"], source.as_str());
        assert_eq!(original["run_id"], run.id.as_str());

        // A retried fork returns the same conversation; other arguments conflict.
        let again = fork(json!({"title": "research (agent)", "command_id": "fork-1"})).unwrap();
        assert_eq!(again["id"], id.as_str());
        let reused = fork(json!({"title": "other", "command_id": "fork-1"})).unwrap_err();
        assert_eq!(reused.kind(), Some(ErrorCode::Conflict));
        let untitled = fork(json!({})).unwrap();
        assert_eq!(untitled["title"], "New conversation");
        assert_eq!(services.native().store().list().unwrap().len(), 3);

        let mut unknown = configuration.clone();
        unknown["workspace_id"] = json!("olympus");
        for (params, kind) in [
            (
                json!({"source_id": "session_missing", "configuration": configuration}),
                ErrorCode::NotFound,
            ),
            // Like `conversation.create` and `agent.configuration.set`.
            (
                json!({"source_id": source, "configuration": unknown}),
                ErrorCode::Conflict,
            ),
            (
                json!({"source_id": source, "configuration": configuration, "command_id": ""}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"source_id": source, "configuration": {"provider_id": "fixture", "model_id": "small", "max_steps": 0}}),
                ErrorCode::InvalidParams,
            ),
            (
                json!({"source_id": source, "configuration": configuration, "extra": 1}),
                ErrorCode::InvalidParams,
            ),
        ] {
            let error = call("conversation.fork", params.clone()).unwrap_err();
            assert_eq!(error.kind(), Some(kind), "{params}: {}", error.message);
        }
        assert_eq!(services.native().store().list().unwrap().len(), 3);

        // Refused while the source has an active run.
        store.begin(&source, "again", "second", None).unwrap();
        let busy = fork(json!({})).unwrap_err();
        assert_eq!(busy.kind(), Some(ErrorCode::Conflict));

        services.vault().lock();
        assert_eq!(fork(json!({})).unwrap_err().code, -32020);
    }
}
