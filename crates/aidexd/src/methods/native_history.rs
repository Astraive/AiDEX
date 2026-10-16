//! Scoped, paged private history and checkpoint content adapters.
use crate::{
    errors::RpcError,
    methods::{native::native_error, native_view as view},
    router::{Router, RouterError},
    rpc::{parse_params, to_result},
    services::Services,
};
use aidex_providers::ChatMessage;
use serde_json::Value;
use std::sync::Arc;
fn messages(
    s: &Services,
    session: Option<&str>,
    run: Option<&str>,
) -> Result<Vec<ChatMessage>, RpcError> {
    match (session, run) {
        (Some(session), None) => Ok(s
            .native()
            .store()
            .get(session)
            .map_err(native_error)?
            .messages),
        _ => scoped(s, session, run).map(|(messages, _)| messages),
    }
}
/// The scope's messages and, for session history, the skills invoked by and
/// the attachments sent with its user messages.
fn scoped(
    s: &Services,
    session: Option<&str>,
    run: Option<&str>,
) -> Result<(Vec<ChatMessage>, view::MessageExtras), RpcError> {
    match (session, run) {
        (Some(session), None) => {
            let detail = s.native().store().get(session).map_err(native_error)?;
            let attachments = s
                .native()
                .store()
                .message_attachments(session)
                .map_err(native_error)?;
            let extras = view::MessageExtras::of(&detail, attachments);
            Ok((detail.messages, extras))
        }
        (None, Some(run)) => Ok((
            s.native()
                .store()
                .run(run)
                .map_err(native_error)?
                .transcript,
            view::MessageExtras::default(),
        )),
        _ => Err(RpcError::invalid_params(
            "choose exactly one session_id or run_id",
        )),
    }
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register_passive("history.list", move |_, p| {
        let p: view::HistoryParams = parse_params(p)?;
        let (messages, extras) = scoped(&s, p.session_id.as_deref(), p.run_id.as_deref())?;
        view::history(&messages, p.offset, p.limit, &extras)
    })?;
    let s = services.clone();
    router.register("history.read", move |_, p| {
        let p: view::HistoryReadParams = parse_params(p)?;
        let messages = messages(&s, p.session_id.as_deref(), p.run_id.as_deref())?;
        let message = messages
            .get(p.index)
            .ok_or_else(|| RpcError::invalid_params("message index exceeds history"))?;
        let content = match p.field.as_deref().unwrap_or("content") {
            "content" => message.content.clone(),
            "tool_arguments" => message
                .tool_calls
                .get(p.call_index.unwrap_or(0))
                .ok_or_else(|| RpcError::invalid_params("tool call index exceeds message"))?
                .arguments
                .clone(),
            "provider_metadata" => message
                .tool_calls
                .get(p.call_index.unwrap_or(0))
                .ok_or_else(|| RpcError::invalid_params("tool call index exceeds message"))?
                .provider_metadata
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_default(),
            _ => {
                return Err(RpcError::invalid_params(
                    "field must be content, tool_arguments or provider_metadata",
                ));
            }
        };
        to_result(&view::chunk(Some(&content), p.offset, p.limit)?)
    })?;
    let s = services.clone();
    router.register("checkpoint.content", move |_, p| {
        let p: view::CheckpointContentParams = parse_params(p)?;
        let detail = s
            .native()
            .store()
            .get(&p.session_id)
            .map_err(native_error)?;
        let checkpoint = detail
            .checkpoints
            .iter()
            .find(|c| c.id == p.id)
            .ok_or_else(|| native_error("checkpoint not found".into()))?;
        to_result(&view::chunk(
            checkpoint.before.as_deref(),
            p.offset,
            p.limit,
        )?)
    })?;
    let s = services.clone();
    router.register("approval.arguments", move |_, p| {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Params {
            id: String,
            #[serde(default)]
            offset: usize,
            #[serde(default)]
            limit: Option<usize>,
        }
        let p: Params = parse_params(p)?;
        let approval = s
            .native()
            .approvals(None)
            .map_err(native_error)?
            .into_iter()
            .find(|a| a.id == p.id)
            .ok_or_else(|| native_error("approval not found".into()))?;
        to_result(&view::chunk(
            Some(&approval.arguments.to_string()),
            p.offset,
            p.limit,
        )?)
    })?;
    Ok(())
}
