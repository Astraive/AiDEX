//! Typed RPC adapters for native sessions, runs, and approvals.
use crate::methods::{attachments::attachment_error, native_view as view};
use crate::{
    errors::{ErrorCode, RpcError},
    router::{Router, RouterError},
    rpc::{expect_no_params, parse_params, to_result},
    services::Services,
};
use aidex_native_sessions::{MAX_ATTACHMENTS_PER_MESSAGE, Mode, SessionConfig};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdParams {
    pub id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionParams {
    pub session_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendParams {
    pub session_id: String,
    pub content: String,
    pub command_id: String,
    #[serde(default)]
    pub parent_run_id: Option<String>,
    /// Skill ids (`<scope>:<name>`) the user invoked for this message, at most
    /// [`aidex_harness::MAX_INVOKED_SKILLS`]. The stored message keeps `content`
    /// exactly as typed; the skills are injected for this run only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// Offer web search and fetch in Chat mode for this run (Agent and Plan
    /// modes follow the web settings). Never stored in the configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_search: Option<bool>,
    /// Staged attachment ids (`attachment.upload.*`, `attachment.from_workspace`)
    /// to send with this message, in order; at most
    /// [`MAX_ATTACHMENTS_PER_MESSAGE`]. `content` may then be blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<String>>,
}
/// Start refusals that are the caller's to fix, carried as `<reason>: <detail>`
/// by the harness: `invalid_params` with that `data.reason`.
const START_REASONS: [&str; 3] = [
    "skill_unavailable",
    aidex_harness::ATTACHMENTS_UNSUPPORTED,
    aidex_harness::CONTEXT_TOO_LARGE_FOR_RUNTIME,
];
/// Map a start failure: invoked skills that cannot be used, attachments the
/// model or runtime cannot take, and transcripts too large for a runtime are
/// `invalid_params` with `data.reason` (`skill_unavailable`,
/// `attachments_unsupported`, `context_too_large_for_runtime`, or an
/// attachment reason such as `attachment_unavailable`); a web switch that
/// cannot be honoured is `unavailable` with `"web_unavailable"`.
fn start_error(message: String) -> RpcError {
    for reason in START_REASONS {
        if let Some(detail) = message
            .strip_prefix(reason)
            .and_then(|rest| rest.strip_prefix(": "))
        {
            return RpcError::invalid_params(aidex_events::redact_text(detail).into_owned())
                .with_data(json!({ "reason": reason }));
        }
    }
    if let Some(detail) = message.strip_prefix("web_unavailable: ") {
        return RpcError::new(
            ErrorCode::Unavailable,
            aidex_events::redact_text(detail).into_owned(),
        )
        .with_data(json!({"reason": "web_unavailable"}));
    }
    attachment_error(message)
}
/// Refusals of `attachments` the daemon can decide from the parameters alone.
fn check_send_attachments(p: &SendParams) -> Result<Vec<String>, RpcError> {
    let attachments = p.attachments.clone().unwrap_or_default();
    let unavailable = |message: &str| {
        RpcError::invalid_params(message).with_data(json!({"reason": "attachment_unavailable"}))
    };
    if attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(unavailable(&format!(
            "at most {MAX_ATTACHMENTS_PER_MESSAGE} attachments can be sent with one message"
        )));
    }
    if !attachments.is_empty() && p.parent_run_id.is_some() {
        return Err(unavailable(
            "attachments can only be sent with a conversation message",
        ));
    }
    Ok(attachments)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinParams {
    pub id: String,
    pub pinned: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationParams {
    pub session_id: String,
    pub configuration: SessionConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalsParams {
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub status: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RespondParams {
    pub id: String,
    pub fingerprint: String,
    pub approved: bool,
    pub command_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointParams {
    pub session_id: String,
    pub id: String,
    #[serde(default)]
    pub expected_digest: Option<String>,
}
pub(crate) fn native_error(message: String) -> RpcError {
    let code = if message.contains("vault is locked") || message.contains("vault exists") {
        ErrorCode::VaultLocked
    } else if message.contains("policy denied") {
        ErrorCode::PolicyDenied
    } else if message.contains("not found") || message.contains("Query returned no rows") {
        ErrorCode::NotFound
    } else if message.contains("unavailable") {
        ErrorCode::Unavailable
    } else {
        ErrorCode::Conflict
    };
    RpcError::new(code, aidex_events::redact_text(&message).into_owned())
}
/// Load the runtime of the conversation `run_id` belongs to before the engine
/// continues that run (`run.resume`, `approval.respond`), as `message.send`
/// does: an unlock whose runtime sync was refused leaves the engine without it.
/// Best effort: the engine reports a runtime it still lacks.
fn ensure_run_runtime(services: &Services, run_id: &str) {
    let store = services.native().store();
    if let Ok(run) = store.run(run_id)
        && let Ok(detail) = store.get(&run.session_id)
    {
        super::runtime::ensure_session_runtime(services, &detail.configuration);
    }
}
/// The configuration of the conversation whose run waits on approval `id`.
/// Only conversations with a run waiting for an approval are decrypted (the
/// run index is plaintext), each at most once; an approval of any other run
/// cannot be answered, so its runtime needs no loading.
fn approval_configuration(services: &Services, id: &str) -> Option<SessionConfig> {
    let store = services.native().store();
    let mut sessions = Vec::new();
    for run in store.run_summaries().ok()? {
        if run.status == aidex_native_sessions::RunStatus::WaitingApproval
            && !sessions.contains(&run.session_id)
        {
            sessions.push(run.session_id);
        }
    }
    sessions.iter().find_map(|session| {
        store
            .get(session)
            .ok()
            .filter(|detail| detail.approvals.iter().any(|approval| approval.id == id))
            .map(|detail| detail.configuration)
    })
}
/// Whether a configuration change ends the conversation's external runtime
/// thread: a different runtime or project, or a different mode, permission mode
/// or instructions, which the vendor thread was started with. Its next message
/// then starts a new thread with the transcript instead of continuing the old
/// one (a Chat-mode thread is never continued as an Agent run).
fn ends_runtime_thread(old: &SessionConfig, new: &SessionConfig) -> bool {
    old.runtime_id != new.runtime_id
        || old.workspace_id != new.workspace_id
        || old.mode != new.mode
        || old.permission_mode != new.permission_mode
        || old.instructions != new.instructions
}
pub(crate) fn validate_workspace(
    services: &Services,
    config: &SessionConfig,
) -> Result<(), RpcError> {
    if let Some(reference) = &config.workspace_id {
        let id = aidex_core::EntityId::new(reference)
            .map_err(|e| RpcError::invalid_params(e.to_string()))?;
        services
            .workspaces()
            .handle(&id)
            .map_err(|e| native_error(e.to_string()))?;
    }
    Ok(())
}
pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError> {
    let s = services.clone();
    router.register("conversation.list", move |_, p| {
        let p:view::PageParams=if p.is_null(){Default::default()}else{parse_params(p)?};let sessions=s.native().store().list().map_err(native_error)?;let total=sessions.len();let offset=p.offset.unwrap_or(0).min(total);let end=(offset+view::page_limit(p.limit)?).min(total);view::bounded(json!({"sessions":&sessions[offset..end],"total_sessions":total,"offset":offset,"next_offset":(end<total).then_some(end),"truncated":end<total}),false)
    })?;
    let s = services.clone();
    router.register("conversation.create", move |_, p| {
        let mut config: SessionConfig = parse_params(p)?;
        config.validate().map_err(native_error)?;
        validate_workspace(&s, &config)?;
        super::runtime::ensure_session_runtime(&s, &config);
        if config.title.trim().is_empty() {
            config.title = "New conversation".into();
        }
        view::session(
            &s.native().store().create(config).map_err(native_error)?,
            None,
            None,
            &view::MessageExtras::default(),
        )
    })?;
    let s = services.clone();
    router.register("conversation.get", move |_, p| {
        let p: view::GetParams = parse_params(p)?;
        let detail = s.native().store().get(&p.id).map_err(native_error)?;
        let attachments = s
            .native()
            .store()
            .message_attachments(&p.id)
            .map_err(native_error)?;
        view::session(
            &detail,
            p.message_offset,
            p.message_limit,
            &view::MessageExtras::of(&detail, attachments),
        )
    })?;
    let s = services.clone();
    router.register("conversation.delete", move |_, p| {
        let p: IdParams = parse_params(p)?;
        to_result(&json!({"deleted":s.native().store().delete(&p.id).map_err(native_error)?}))
    })?;
    let s = services.clone();
    router.register("conversation.pin", move |_, p| {
        let p: PinParams = parse_params(p)?;
        s.native()
            .store()
            .update(&p.id, |detail| {
                detail.pinned = p.pinned;
                Ok(())
            })
            .map_err(native_error)?;
        to_result(
            &s.native()
                .store()
                .list()
                .map_err(native_error)?
                .into_iter()
                .find(|s| s.id == p.id),
        )
    })?;
    for method in ["message.send", "agent.start"] {
        let s = services.clone();
        router.register(method, move |_, p| {
            let p: SendParams = parse_params(p)?;
            let config = s
                .native()
                .store()
                .get(&p.session_id)
                .map_err(native_error)?
                .configuration;
            if method == "agent.start" && config.mode != Mode::Agent {
                return Err(RpcError::invalid_params("agent.start requires agent mode"));
            }
            let attachments = check_send_attachments(&p)?;
            let skills = p.skills.unwrap_or_default();
            if skills.len() > aidex_harness::MAX_INVOKED_SKILLS {
                return Err(RpcError::invalid_params(format!(
                    "at most {} skills can be invoked at once",
                    aidex_harness::MAX_INVOKED_SKILLS
                ))
                .with_data(json!({"reason": "skill_unavailable"})));
            }
            super::runtime::ensure_session_runtime(&s, &config);
            let options = aidex_harness::RunOptions {
                skills,
                web_search: p.web_search.unwrap_or(false),
                attachments,
            };
            to_result(&view::run(
                &s.native()
                    .start_with(
                        &p.session_id,
                        &p.content,
                        &p.command_id,
                        p.parent_run_id,
                        options,
                    )
                    .map_err(start_error)?,
            ))
        })?;
    }
    let s = services.clone();
    router.register("run.get", move |_, p| {
        let p: IdParams = parse_params(p)?;
        to_result(&view::run(
            &s.native().store().run(&p.id).map_err(native_error)?,
        ))
    })?;
    let s = services.clone();
    router.register("run.cancel", move |_, p| {
        let p: IdParams = parse_params(p)?;
        to_result(&view::run(&s.native().cancel(&p.id).map_err(native_error)?))
    })?;
    let s = services.clone();
    router.register("run.resume", move |_, p| {
        let p: IdParams = parse_params(p)?;
        ensure_run_runtime(&s, &p.id);
        to_result(&view::run(&s.native().resume(&p.id).map_err(native_error)?))
    })?;
    let s = services.clone();
    router.register("approval.list", move |_, p| {
        let p: ApprovalsParams = if p.is_null() {
            ApprovalsParams { run_id: None,offset:None,limit:None,status:None }
        } else {
            parse_params(p)?
        };
        let mut approvals=s.native().approvals(p.run_id.as_deref()).map_err(native_error)?;
        if let Some(status)=&p.status{if !matches!(status.as_str(),"pending"|"approved"|"denied"|"expired"){return Err(RpcError::invalid_params("unknown approval status"));}approvals.retain(|a|&a.status==status);}
        let total=approvals.len();let limit=view::page_limit(p.limit)?;let offset=p.offset.unwrap_or_else(||total.saturating_sub(limit)).min(total);let end=(offset+limit).min(total);
        view::bounded(json!({"approvals":approvals[offset..end].iter().map(view::approval).collect::<Vec<_>>(),"total_approvals":total,"offset":offset,"next_offset":(end<total).then_some(end),"truncated":offset>0||end<total}),true)
    })?;
    let s = services.clone();
    router.register("approval.respond", move |_, p| {
        let p: RespondParams = parse_params(p)?;
        // Only on a drive that has runtimes to load.
        if s.runtime()
            .is_ok_and(|manager| manager.has_saved_configuration())
            && let Some(configuration) = approval_configuration(&s, &p.id)
        {
            super::runtime::ensure_session_runtime(&s, &configuration);
        }
        to_result(&view::run(
            &s.native()
                .respond(&p.id, &p.fingerprint, p.approved, &p.command_id)
                .map_err(native_error)?,
        ))
    })?;
    let s = services.clone();
    router.register("agent.configuration.get", move |_, p| {
        let p: SessionParams = parse_params(p)?;
        to_result(
            &s.native()
                .store()
                .get(&p.session_id)
                .map_err(native_error)?
                .configuration,
        )
    })?;
    let s = services.clone();
    router.register("agent.configuration.set", move |_, p| {
        let p: ConfigurationParams = parse_params(p)?;
        p.configuration.validate().map_err(native_error)?;
        validate_workspace(&s, &p.configuration)?;
        s.native()
            .store()
            .update(&p.session_id, |detail| {
                if detail.runs.iter().any(|r| r.status.active()) {
                    return Err("cancel active runs before changing configuration".into());
                }
                if detail.configuration.workspace_id != p.configuration.workspace_id
                    && !detail.checkpoints.is_empty()
                {
                    return Err(
                        "checkpoint history binds this conversation to its original workspace"
                            .into(),
                    );
                }
                if ends_runtime_thread(&detail.configuration, &p.configuration) {
                    // No run is active here, so none of these references is
                    // in use: an interrupted run cannot resume the old thread
                    // either.
                    detail.runtime_ref = None;
                    for run in &mut detail.runs {
                        run.runtime_ref = None;
                    }
                }
                detail.configuration = p.configuration.clone();
                Ok(())
            })
            .map_err(native_error)?;
        to_result(&p.configuration)
    })?;
    let s = services.clone();
    router.register("execution.status",move|_,p|{expect_no_params(&p)?;let sessions=s.native().store().list().map_err(native_error)?;let runs=s.native().store().run_summaries().map_err(native_error)?;view::bounded(json!({"capabilities":s.native().capabilities(),"total_sessions":sessions.len(),"sessions":sessions.into_iter().take(100).collect::<Vec<_>>(),"total_runs":runs.len(),"runs":runs.into_iter().take(1000).collect::<Vec<_>>(),"runtime_descriptors":s.native().runtime_descriptors()}),false)})?;
    let s = services.clone();
    router.register("checkpoint.list",move|_,p|{#[derive(Deserialize)]#[serde(deny_unknown_fields)]struct Params{session_id:String,#[serde(default)]offset:Option<usize>,#[serde(default)]limit:Option<usize>}let p:Params=parse_params(p)?;let s=s.native().store().get(&p.session_id).map_err(native_error)?;let total=s.checkpoints.len();let offset=p.offset.unwrap_or(0).min(total);let end=(offset+view::page_limit(p.limit)?).min(total);view::bounded(json!({"checkpoints":s.checkpoints[offset..end].iter().map(view::checkpoint).collect::<Vec<_>>(),"offset":offset,"total_checkpoints":total,"next_offset":(end<total).then_some(end),"truncated":end<total}),false)})?;
    let s = services.clone();
    router.register("checkpoint.compare", move |_, p| {
        let p: CheckpointParams = parse_params(p)?;
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
        let mut result = s
            .native()
            .compare(&p.session_id, &p.id)
            .map_err(native_error)?;
        result["checkpoint"] = serde_json::to_value(view::checkpoint(checkpoint))
            .map_err(|e| native_error(e.to_string()))?;
        view::bounded(result, false)
    })?;
    let s = services.clone();
    router.register("checkpoint.restore", move |_, p| {
        let p: CheckpointParams = parse_params(p)?;
        let digest = p.expected_digest.ok_or_else(|| {
            RpcError::invalid_params("expected_digest from checkpoint.compare is required")
        })?;
        s.native()
            .restore(&p.session_id, &p.id, &digest)
            .map_err(native_error)?;
        to_result(&json!({"restored":p.id}))
    })?;
    crate::methods::workspace_live::register(router, services)?;
    crate::methods::native_history::register(router, services)?;
    crate::methods::native_fork::register(router, services)?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        lifecycle::ShutdownSignal,
        router::{CallContext, ClientInfo},
    };
    use aidex_vault::SecretString;
    use serde_json::{Value, json};
    #[test]
    fn project_free_chat_stays_project_free_and_unknown_projects_are_not_persisted() {
        let drive = crate::services::testing::temp_drive();
        let ws = tempfile::tempdir().unwrap();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        services
            .vault()
            .init(&SecretString::from("fixture-passphrase".to_owned()), false)
            .unwrap();
        let workspace = services.workspaces().add_host(ws.path(), None).unwrap();
        services.workspaces().select(&workspace.id).unwrap();
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let ctx = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        let config = json!({"title":"misc chat","mode":"chat","provider_id":"fixture","model_id":"small","workspace_id":null});
        let created = router
            .call(&ctx, "conversation.create", config.clone())
            .unwrap();
        assert!(created["workspace_id"].is_null());
        let id = created["id"].as_str().unwrap();
        services
            .native()
            .store()
            .update(id, |detail| {
                detail
                    .messages
                    .push(aidex_providers::ChatMessage::user("keep this chat"));
                Ok(())
            })
            .unwrap();
        let mut unknown = config.clone();
        unknown["workspace_id"] = json!("olympus");
        let before = services.native().store().list().unwrap().len();
        assert!(
            router
                .call(&ctx, "conversation.create", unknown.clone())
                .is_err()
        );
        assert_eq!(services.native().store().list().unwrap().len(), before);
        assert!(
            router
                .call(
                    &ctx,
                    "agent.configuration.set",
                    json!({"session_id":id,"configuration":unknown})
                )
                .is_err()
        );
        assert!(
            services
                .native()
                .store()
                .get(id)
                .unwrap()
                .configuration
                .workspace_id
                .is_none()
        );
        let mut valid = config;
        valid["workspace_id"] = json!(workspace.id);
        router
            .call(
                &ctx,
                "agent.configuration.set",
                json!({"session_id":id,"configuration":valid}),
            )
            .unwrap();
        let cleared = router.call(&ctx, "agent.configuration.set", json!({"session_id":id,"configuration":{"title":"misc chat","mode":"chat","provider_id":"fixture","model_id":"small","workspace_id":null}})).unwrap();
        assert!(cleared["workspace_id"].is_null());
        assert_eq!(
            services.native().store().get(id).unwrap().messages[0].content,
            "keep this chat"
        );
        ws.close().unwrap();
        let unavailable = json!({"title":"misc chat","mode":"chat","provider_id":"fixture","model_id":"small","workspace_id":workspace.id});
        assert!(
            router
                .call(&ctx, "conversation.create", unavailable.clone())
                .is_err()
        );
        assert!(
            router
                .call(
                    &ctx,
                    "agent.configuration.set",
                    json!({"session_id":id,"configuration":unavailable})
                )
                .is_err()
        );
        assert!(
            services
                .native()
                .store()
                .get(id)
                .unwrap()
                .configuration
                .workspace_id
                .is_none()
        );
    }
    #[test]
    fn native_contract_preserves_mode_history_and_workspace_reads() {
        let drive = crate::services::testing::temp_drive();
        let ws = tempfile::tempdir().unwrap();
        std::fs::write(ws.path().join("source.txt"), "workspace fixture").unwrap();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        services
            .vault()
            .init(&SecretString::from("fixture-passphrase".to_owned()), false)
            .unwrap();
        let workspace = services.workspaces().add_host(ws.path(), None).unwrap();
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let ctx = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        let created=router.call(&ctx,"conversation.create",json!({"title":"private fixture","mode":"chat","provider_id":"fixture","model_id":"small","workspace_id":workspace.id}));
        assert!(created.is_ok(), "{created:?}");
        let s = created.unwrap();
        let id = s["id"].as_str().unwrap();
        let listed = router.call(&ctx, "conversation.list", Value::Null).unwrap();
        assert_eq!(listed["sessions"][0]["title"], "private fixture");
        let config=router.call(&ctx,"agent.configuration.set",json!({"session_id":id,"configuration":{"title":"private fixture","mode":"agent","provider_id":"fixture","model_id":"small","workspace_id":workspace.id}})).unwrap();
        assert_eq!(config["mode"], "agent");
        let read = router
            .call(
                &ctx,
                "workspace.files.read",
                json!({"workspace_id":workspace.id,"path":"source.txt"}),
            )
            .unwrap();
        assert_eq!(read["content"], "workspace fixture");
        assert!(
            router
                .call(
                    &ctx,
                    "workspace.files.read",
                    json!({"workspace_id":workspace.id,"path":"../escape"})
                )
                .is_err()
        );
        services.vault().lock();
        let locked = router.call(&ctx, "conversation.list", Value::Null).unwrap();
        assert!(locked["sessions"][0].get("title").is_none());
        let e = router
            .call(&ctx, "conversation.get", json!({"id":id}))
            .unwrap_err();
        assert_eq!(e.code, -32020);
    }
    #[test]
    fn send_params_invoke_skills_and_gate_the_web_switch() {
        use aidex_providers::{
            FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
            ProviderId,
        };
        use std::time::{Duration, Instant};
        let drive = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        services
            .vault()
            .init(&SecretString::from("fixture-passphrase".to_owned()), false)
            .unwrap();
        let skill = services.paths().skills().join("review");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: review\ndescription: Reviews diffs\n---\nCheck every hunk.\n",
        )
        .unwrap();
        let fake = Arc::new(
            FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
                .with_model(ModelDescriptor::new(
                    ModelRef::new(
                        ProviderId::new("fixture").unwrap(),
                        ModelId::new("small").unwrap(),
                    ),
                    Locality::Local,
                    8192,
                ))
                .with_script(FakeScript::new().then(FakeTurn::text("reviewed"))),
        );
        services
            .models()
            .registry()
            .register_shared(fake.clone())
            .unwrap();
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        let ctx = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        let created = router
            .call(
                &ctx,
                "conversation.create",
                json!({"title":"skills","mode":"chat","provider_id":"fixture","model_id":"small"}),
            )
            .unwrap();
        let id = created["id"].as_str().unwrap().to_owned();
        let send = |params: Value| router.call(&ctx, "message.send", params);
        let base = json!({"session_id": id, "content": "$review go", "command_id": "c1"});
        let with = |key: &str, value: Value| {
            let mut params = base.clone();
            params[key] = value;
            params
        };

        let error = send(with("skills", json!(["drive:missing"]))).unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::InvalidParams));
        assert_eq!(error.data.as_ref().unwrap()["reason"], "skill_unavailable");
        let many: Vec<String> = (0..9).map(|n| format!("drive:s{n}")).collect();
        let error = send(with("skills", json!(many))).unwrap_err();
        assert_eq!(error.data.as_ref().unwrap()["reason"], "skill_unavailable");
        let error = send(with("web_search", json!(true))).unwrap_err();
        assert_eq!(error.kind(), Some(ErrorCode::Unavailable));
        assert_eq!(error.data.as_ref().unwrap()["reason"], "web_unavailable");
        assert!(send(with("skill", json!(["drive:review"]))).is_err());
        assert!(fake.requests().is_empty());

        let run = send(with("skills", json!(["drive:review"]))).unwrap();
        let run_id = run["id"].as_str().unwrap().to_owned();
        let until = Instant::now() + Duration::from_secs(10);
        while services
            .native()
            .store()
            .run(&run_id)
            .unwrap()
            .status
            .active()
        {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        let request = &fake.requests()[0];
        assert!(request.messages.iter().any(|m| {
            m.content
                .starts_with("The user invoked the skill \"review\".")
                && m.content.contains("Check every hunk.")
                && m.content.contains("trust=user>>>")
        }));
        let view = router
            .call(&ctx, "conversation.get", json!({"id": id}))
            .unwrap();
        assert_eq!(view["messages"][0]["content"], "$review go");
        assert_eq!(view["messages"][0]["skills"], json!(["drive:review"]));
        assert!(view["messages"][1].get("skills").is_none());
        let history = router
            .call(&ctx, "history.list", json!({"session_id": id}))
            .unwrap();
        assert_eq!(history["messages"][0]["skills"], json!(["drive:review"]));
        // The same command id with different skills is a different request.
        let error = send(base.clone()).unwrap_err();
        assert!(
            error.message.contains("command id reused"),
            "{}",
            error.message
        );
    }
    #[test]
    fn vault_lock_cancels_native_work_before_private_history_becomes_unavailable() {
        use aidex_providers::{
            FakeProvider, FakeScript, FakeTurn, Locality, ModelDescriptor, ModelId, ModelRef,
            ProviderId,
        };
        use std::time::{Duration, Instant};
        let drive = crate::services::testing::temp_drive();
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
        let passphrase = SecretString::from("fixture-passphrase".to_owned());
        services.vault().init(&passphrase, false).unwrap();
        let fake = Arc::new(
            FakeProvider::new(ProviderId::new("fixture").unwrap(), Locality::Local)
                .with_model(ModelDescriptor::new(
                    ModelRef::new(
                        ProviderId::new("fixture").unwrap(),
                        ModelId::new("small").unwrap(),
                    ),
                    Locality::Local,
                    2048,
                ))
                .with_script(
                    FakeScript::new()
                        .then(FakeTurn::text("late").with_delay(Duration::from_secs(3))),
                ),
        );
        services
            .models()
            .registry()
            .register_shared(fake.clone())
            .unwrap();
        let session = services
            .native()
            .store()
            .create(
                serde_json::from_value(json!({"provider_id":"fixture","model_id":"small"}))
                    .unwrap(),
            )
            .unwrap();
        let run = services
            .native()
            .start(&session.id, "hi", "once", None)
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while fake.requests().is_empty() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut router = Router::new();
        register(&mut router, &services).unwrap();
        crate::methods::vault::register(&mut router, &services).unwrap();
        let context = CallContext::detached(ClientInfo::new("test", "1"), shutdown);
        router.call(&context, "vault.lock", Value::Null).unwrap();
        let summary = services
            .native()
            .store()
            .run_summaries()
            .unwrap()
            .into_iter()
            .find(|r| r.id == run.id)
            .unwrap();
        assert_eq!(summary.status, aidex_native_sessions::RunStatus::Cancelled);
        assert!(services.native().store().get(&session.id).is_err());
        assert!(services.native().store().list().unwrap()[0].title.is_none());
        services.vault().unlock(&passphrase).unwrap();
        assert_eq!(
            services.native().store().run(&run.id).unwrap().status,
            aidex_native_sessions::RunStatus::Cancelled
        );
    }
}

#[cfg(test)]
#[test]
fn large_private_history_and_checkpoint_have_bounded_rpc_views_and_complete_restore() {
    let drive = crate::services::testing::temp_drive();
    let ws = tempfile::tempdir().unwrap();
    let shutdown = crate::lifecycle::ShutdownSignal::new();
    let services = Arc::new(Services::open(drive.path(), shutdown.clone()).unwrap());
    services
        .vault()
        .init(
            &aidex_vault::SecretString::from("fixture-passphrase".to_owned()),
            false,
        )
        .unwrap();
    let workspace = services.workspaces().add_host(ws.path(), None).unwrap();
    let session = services
        .native()
        .store()
        .create(
            serde_json::from_value(
                json!({"workspace_id":workspace.id,"provider_id":"fixture","model_id":"small"}),
            )
            .unwrap(),
        )
        .unwrap();
    let large = "complete-original\n".repeat(90000);
    std::fs::write(ws.path().join("file.txt"), "new").unwrap();
    let cp = aidex_native_sessions::Checkpoint {
        id: aidex_native_sessions::id("checkpoint").unwrap(),
        run_id: "fixture-run".into(),
        path: "file.txt".into(),
        before: Some(large.clone()),
        after_digest: aidex_native_sessions::digest(b"new"),
    };
    services
        .native()
        .store()
        .update(&session.id, |s| {
            s.messages
                .push(aidex_providers::ChatMessage::assistant(large.clone()));
            s.checkpoints.push(cp.clone());
            Ok(())
        })
        .unwrap();
    let mut router = Router::new();
    register(&mut router, &services).unwrap();
    let context =
        crate::router::CallContext::detached(crate::router::ClientInfo::new("test", "1"), shutdown);
    let view = router
        .call(&context, "conversation.get", json!({"id":session.id}))
        .unwrap();
    assert!(serde_json::to_vec(&view).unwrap().len() < 900 * 1024);
    assert_eq!(view["messages"][0]["content_truncated"], true);
    let compare = router
        .call(
            &context,
            "checkpoint.compare",
            json!({"session_id":session.id,"id":cp.id}),
        )
        .unwrap();
    assert!(serde_json::to_vec(&compare).unwrap().len() < 900 * 1024);
    assert_eq!(compare["checkpoint"]["before_bytes"], large.len());
    let chunk = router
        .call(
            &context,
            "checkpoint.content",
            json!({"session_id":session.id,"id":cp.id,"offset":0,"limit":65536}),
        )
        .unwrap();
    assert_eq!(chunk["total_bytes"], large.len());
    assert_eq!(chunk["truncated"], true);
    router
        .call(
            &context,
            "checkpoint.restore",
            json!({"session_id":session.id,"id":cp.id,"expected_digest":cp.after_digest}),
        )
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(ws.path().join("file.txt")).unwrap(),
        large
    );
    assert_eq!(
        services.native().store().get(&session.id).unwrap().messages[0]
            .content
            .len(),
        large.len()
    );
}
