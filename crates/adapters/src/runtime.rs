use crate::{
    Driver, RuntimeAuth, RuntimeConfig,
    protocol::{Protocol, prompt},
    state::{State, error},
    transport::Transport,
};
use aidex_auth::AuthService;
use aidex_core::CancelToken;
use aidex_native_sessions::*;
use aidex_sandbox::{
    CommandSpec, Mount, NetworkMode, ResourceLimits, Sandbox, SandboxLevel, SandboxRequest,
};
use aidex_vault::{ConsumerScope, SecretName};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use zeroize::Zeroizing;
pub(crate) type NetworkGate = Arc<dyn Fn() -> bool + Send + Sync>;
struct Pending {
    wire: Value,
    request_id: String,
    reference: Value,
    fingerprint: String,
    allow: Value,
    deny: Value,
}
struct Live {
    transport: Transport,
    protocol: Protocol,
    profile: tempfile::TempDir,
    snapshot: String,
    pending: Option<Pending>,
    text: String,
    tools: BTreeSet<String>,
    run_id: String,
    session_id: String,
}
pub struct ExternalRuntime {
    pub(crate) config: RuntimeConfig,
    state: Arc<State>,
    auth: Arc<AuthService>,
    sandbox: Sandbox,
    network: NetworkGate,
    live: Mutex<BTreeMap<String, Live>>,
}
impl ExternalRuntime {
    pub(crate) fn new(
        config: RuntimeConfig,
        state: Arc<State>,
        auth: Arc<AuthService>,
        sandbox: Sandbox,
        network: NetworkGate,
    ) -> Self {
        Self {
            config,
            state,
            auth,
            sandbox,
            network,
            live: Mutex::new(BTreeMap::new()),
        }
    }
    fn launch(
        &self,
        r: &RuntimeRequest,
        cancel: &CancelToken,
        begin: bool,
    ) -> Result<Live, RuntimeError> {
        if !(self.network)() {
            return Err(error(
                "offline_refused",
                "External runtime network use is disabled",
            ));
        }
        if cancel.is_stopped() {
            return Err(error("cancelled", "Runtime cancelled"));
        }
        let profile = tempfile::Builder::new()
            .prefix("aidex-runtime-")
            .tempdir()
            .map_err(|_| error("state_unavailable", "Runtime temporary profile unavailable"))?;
        let snapshot = format!(
            "runtime-{}.sealed",
            digest(format!("{}:{}", self.config.id, r.session_id).as_bytes())
        );
        let home = profile.path().join("home");
        std::fs::create_dir_all(&home)
            .map_err(|_| error("state_unavailable", "Runtime home unavailable"))?;
        // A selected managed profile is encrypted input, never a raw host login home.
        if let Some(selected) = &self.config.profile {
            self.state.restore(
                &format!("profile-{}.sealed", digest(selected.as_bytes())),
                &home,
            )?;
        }
        if r.operation == RuntimeOperation::Resume {
            self.state.restore(&snapshot, &home)?;
        }
        let cwd = PathBuf::from(&r.workspace.as_ref().unwrap().root);
        let mut request = SandboxRequest::new(SandboxLevel::Brokered, &cwd, profile.path())
            .with_mount(Mount::read_write(&cwd))
            .with_network(NetworkMode::Normal)
            .with_limits(ResourceLimits {
                wall_timeout: Some(Duration::from_secs(300)),
                max_output_bytes: crate::transport::MAX_FRAME,
                ..Default::default()
            });
        let mut args = self.config.argv.clone();
        args.extend(self.config.driver.info().command);
        let mut secrets: Vec<Zeroizing<String>> = vec![];
        match &self.config.auth {
            RuntimeAuth::None => {}
            RuntimeAuth::CodexOauth { provider_id } => {
                if self.config.driver != Driver::Codex {
                    return Err(error(
                        "invalid_config",
                        "Codex OAuth can only authenticate Codex",
                    ));
                }
                let secret = self
                    .auth
                    .resolve(provider_id, &format!("oauth.{provider_id}.access"))
                    .map_err(|_| {
                        error(
                            "auth_required",
                            "Connect the shared ChatGPT account before using Codex",
                        )
                    })?;
                let token = Zeroizing::new(
                    String::from_utf8(secret.expose().to_vec())
                        .map_err(|_| error("auth_required", "Invalid credential encoding"))?,
                );
                request.env.push(("ACCESS_TOKEN".into(), token.to_string()));
                secrets.push(token);
                args.extend(["-c".into(),"model_provider=\"aidex_chatgpt\"".into(),"-c".into(),"model_providers.aidex_chatgpt.name=\"AiDEX\"".into(),"-c".into(),"model_providers.aidex_chatgpt.base_url=\"https://api.openai.com/v1\"".into(),"-c".into(),"model_providers.aidex_chatgpt.wire_api=\"responses\"".into(),"-c".into(),"model_providers.aidex_chatgpt.env_key=\"ACCESS_TOKEN\"".into(),"-c".into(),"model_providers.aidex_chatgpt.requires_openai_auth=false".into(),"-c".into(),"shell_environment_policy.exclude=[\"ACCESS_TOKEN\",\"OPENAI_API_KEY\",\"CODEX_API_KEY\"]".into()]);
            }
            RuntimeAuth::Vault {
                secret_name,
                env_name,
            } => {
                let consumer = ConsumerScope::process(format!("runtime-{}", self.config.id))
                    .map_err(|_| error("invalid_config", "Invalid runtime secret consumer"))?;
                let name = SecretName::new(secret_name)
                    .map_err(|_| error("invalid_config", "Invalid secret reference"))?;
                let handle = self
                    .state
                    .vault
                    .issue_handle(&name, &consumer, Duration::from_secs(5))
                    .map_err(|_| {
                        error(
                            "auth_required",
                            "Runtime credential is unavailable or consumer denied",
                        )
                    })?;
                let secret = self.state.vault.resolve(&handle, &consumer);
                self.state.vault.revoke_handle(&handle.handle_id);
                let secret =
                    secret.map_err(|_| error("auth_required", "Runtime credential unavailable"))?;
                let token = Zeroizing::new(
                    String::from_utf8(secret.expose().to_vec())
                        .map_err(|_| error("auth_required", "Credential must be UTF-8"))?,
                );
                request.env.push((env_name.clone(), token.to_string()));
                secrets.push(token);
                if self.config.driver == Driver::Codex {
                    args.extend([
                        "-c".into(),
                        format!(
                            "shell_environment_policy.exclude=[{}]",
                            serde_json::to_string(env_name).unwrap()
                        ),
                    ]);
                }
            }
        }
        let home_string = home.to_string_lossy().into_owned();
        for name in [
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "PI_CODING_AGENT_DIR",
            "OMP_CODING_AGENT_DIR",
            "MUSE_HOME",
            "GROK_HOME",
            "GEMINI_CLI_HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            request.env.push((name.into(), home_string.clone()));
        }
        request.env.push(("MUSE_NO_AUTO_UPDATE".into(), "1".into()));
        if self.config.driver == Driver::ClaudeCode {
            args.push("--dangerously-skip-permissions".into());
            args.push("--max-turns".into());
            args.push(r.max_steps.to_string());
        }
        if matches!(self.config.driver, Driver::Pi | Driver::Omp) {
            args.extend([
                "--session-dir".into(),
                home.join("sessions").to_string_lossy().into_owned(),
            ]);
        }
        if matches!(self.config.driver, Driver::ClaudeCode | Driver::Cursor) {
            if let Some(model) = &r.model {
                args.extend(["--model".into(), model.clone()]);
            }
            if r.operation == RuntimeOperation::Resume
                && let Some(id) = r
                    .runtime_ref
                    .as_ref()
                    .and_then(|v| v["native_session_id"].as_str())
            {
                args.extend(["--resume".into(), id.into()]);
            }
            args.push(prompt(
                r,
                r.operation == RuntimeOperation::Resume && r.runtime_ref.is_none(),
            ));
        }
        let plan = self.sandbox.negotiate(&request).map_err(|_| {
            error(
                "scope_unsupported",
                "Requested runtime process controls are unavailable on this host",
            )
        })?;
        let executable = if PathBuf::from(&self.config.executable).is_absolute() {
            PathBuf::from(&self.config.executable)
        } else {
            self.state
                .root
                .parent()
                .unwrap()
                .join(&self.config.executable)
        };
        let process = self
            .sandbox
            .spawn_interactive(&plan, CommandSpec::new(executable).args(args))
            .map_err(|_| {
                error(
                    "launch_failed",
                    "Configured runtime executable could not be launched",
                )
            })?;
        let mut transport = Transport::new(process, secrets);
        // The original cancellation token remains effective while execute has returned
        // WaitingApproval. The watcher owns only a weak process handle.
        let weak = Arc::downgrade(&transport.process);
        let watched = cancel.clone();
        let network = self.network.clone();
        std::thread::spawn(move || {
            while let Some(process) = weak.upgrade() {
                if !process.is_running() {
                    break;
                }
                if watched.is_stopped() || !network() {
                    let _ = process.kill_tree();
                    break;
                }
                drop(process);
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let mut protocol = Protocol::new(self.config.driver);
        protocol.initialize(&mut transport, cancel)?;
        if begin {
            protocol.begin(&mut transport, r, cancel)?;
        }
        Ok(Live {
            transport,
            protocol,
            profile,
            snapshot,
            pending: None,
            text: String::new(),
            tools: BTreeSet::new(),
            run_id: r.run_id.clone(),
            session_id: r.session_id.clone(),
        })
    }
    fn reference(&self, live: &Live) -> Value {
        json!({"runtime_id":self.config.id,"native_session_id":live.protocol.session,"native_turn_id":live.protocol.turn,"snapshot_id":live.snapshot,"run_id":live.run_id,"session_id":live.session_id})
    }
    fn approval(
        &self,
        live: &Live,
        r: &RuntimeRequest,
        v: &Value,
    ) -> Result<Option<Pending>, RuntimeError> {
        let method = v["method"].as_str().unwrap_or("");
        if v.get("id").is_none()
            || !matches!(
                method,
                "item/commandExecution/requestApproval"
                    | "item/fileChange/requestApproval"
                    | "session/request_permission"
                    | "approval/request"
            )
        {
            return Ok(None);
        }
        let params = &v["params"];
        if !live.protocol.owns(params) {
            return Err(error(
                "approval_mismatch",
                "Unowned native approval request",
            ));
        }
        let wire = v["id"].clone();
        if !wire.is_string() && !wire.is_i64() {
            return Err(error(
                "protocol_error",
                "Invalid native approval request ID",
            ));
        }
        let request_id = wire.to_string();
        let tool = if method == "session/request_permission" {
            params
                .pointer("/toolCall/title")
                .and_then(Value::as_str)
                .unwrap_or("external_tool")
        } else {
            method
        };
        let tool = aidex_events::redact_text(tool).into_owned();
        let mut reference = self.reference(live);
        reference["native_request_id"] = wire.clone();
        reference["approval_digest"] = json!(digest(params.to_string().as_bytes()));
        let arguments = sanitize_json(params.clone());
        let fingerprint=digest(json!({"runtime":self.config.id,"session":r.session_id,"run":r.run_id,"workspace":r.workspace,"permission":r.permission_mode,"request":request_id,"tool":tool,"arguments":arguments,"reference":reference}).to_string().as_bytes());
        let (allow, deny) = if method == "session/request_permission" {
            let options = params["options"]
                .as_array()
                .ok_or_else(|| error("protocol_error", "Approval options missing"))?;
            let allow = options
                .iter()
                .find(|v| v["kind"] == "allow_once")
                .and_then(|v| v["optionId"].as_str());
            let deny = options
                .iter()
                .find(|v| v["kind"] == "reject_once")
                .and_then(|v| v["optionId"].as_str());
            (
                allow
                    .map(|id| json!({"outcome":{"outcome":"selected","optionId":id}}))
                    .unwrap_or(json!({"outcome":{"outcome":"cancelled"}})),
                deny.map(|id| json!({"outcome":{"outcome":"selected","optionId":id}}))
                    .unwrap_or(json!({"outcome":{"outcome":"cancelled"}})),
            )
        } else if method == "approval/request" {
            return Err(error(
                "unsupported",
                "MSP approval choice protocol needs a supported offered choice; full access mode should not issue approvals",
            ));
        } else {
            (json!({"decision":"accept"}), json!({"decision":"decline"}))
        };
        Ok(Some(Pending {
            wire,
            request_id,
            reference,
            fingerprint,
            allow,
            deny,
        }))
    }
    fn drive(
        &self,
        mut live: Live,
        r: &RuntimeRequest,
        on_event: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        loop {
            if !(self.network)() {
                live.transport.stop();
                return Err(error(
                    "offline_refused",
                    "External runtime network use disabled",
                ));
            }
            let v = live.protocol.next(&mut live.transport, cancel)?;
            if matches!(self.config.driver, Driver::ClaudeCode | Driver::Cursor) {
                if let Some(id) = v["session_id"].as_str() {
                    live.protocol.session = id.into();
                }
                if v["type"] == "assistant"
                    && v["parent_tool_use_id"].is_null()
                    && let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array)
                    && live.text.is_empty()
                {
                    for block in blocks {
                        if let Some(text) = block["text"].as_str() {
                            let text = aidex_events::redact_text(text).into_owned();
                            live.text.push_str(&text);
                            on_event(&RuntimeEvent::TextDelta { text });
                        }
                    }
                }
            }
            if let Some(pending) = self.approval(&live, r, &v)? {
                let method = v["method"].as_str().unwrap();
                let tool_name = if method == "session/request_permission" {
                    v.pointer("/params/toolCall/title")
                        .and_then(Value::as_str)
                        .unwrap_or("external_tool")
                } else {
                    method
                };
                let event = RuntimeEvent::ApprovalRequest {
                    request_id: pending.request_id.clone(),
                    tool_name: aidex_events::redact_text(tool_name).into_owned(),
                    arguments: sanitize_json(v["params"].clone()),
                    summary: "Review vendor tool request before execution".into(),
                    runtime_ref: pending.reference.clone(),
                };
                let control = on_event(&event);
                if control == RuntimeControl::Pause {
                    let reference = self.reference(&live);
                    live.pending = Some(pending);
                    self.live
                        .lock()
                        .map_err(|_| error("state_unavailable", "Runtime lock unavailable"))?
                        .insert(r.run_id.clone(), live);
                    return Ok(RuntimeResponse {
                        status: RuntimeStatus::WaitingApproval,
                        runtime_ref: Some(reference),
                        message: None,
                    });
                }
                // A Continue callback is observation, never an implicit approval grant.
                live.transport
                    .send(json!({"jsonrpc":"2.0","id":pending.wire,"result":pending.deny}))?;
                continue;
            }
            if v.get("id").is_some() && v.get("method").is_some() {
                live.transport.send(json!({"jsonrpc":"2.0","id":v["id"],"error":{"code":-32601,"message":"AiDEX does not implement this client method"}}))?;
                continue;
            }
            for event in live.protocol.events(&v) {
                match &event {
                    RuntimeEvent::TextDelta { text } => {
                        live.text.push_str(text);
                        if live.text.len() > r.max_tokens as usize * 16 {
                            live.transport.stop();
                            return Err(error(
                                "output_limit",
                                "Runtime response exceeded AiDEX output budget",
                            ));
                        }
                    }
                    RuntimeEvent::ToolProgress {
                        call_id,
                        status: RuntimeToolStatus::Started | RuntimeToolStatus::Requested,
                        ..
                    } => {
                        live.tools.insert(call_id.clone());
                        if live.tools.len() > r.max_steps as usize {
                            live.transport.stop();
                            return Err(error(
                                "step_limit",
                                "Runtime exceeded tool observation budget",
                            ));
                        }
                    }
                    _ => {}
                }
                if on_event(&event) == RuntimeControl::Pause {
                    live.transport.stop();
                    return Err(error("interrupted", "Runtime stopped by event consumer"));
                }
            }
            if live.protocol.completed(&v)? {
                if live.text.is_empty()
                    && let Some(text) = v["result"].as_str()
                {
                    live.text = aidex_events::redact_text(text).into_owned();
                }
                let reference = self.reference(&live);
                live.transport.stop();
                self.state
                    .snapshot(&live.snapshot, &live.profile.path().join("home"))?;
                return Ok(RuntimeResponse {
                    status: RuntimeStatus::Completed,
                    runtime_ref: Some(reference),
                    message: Some(aidex_providers::ChatMessage::assistant(live.text)),
                });
            }
        }
    }
    pub(crate) fn discover_models(
        &self,
        r: &RuntimeRequest,
    ) -> Result<Vec<crate::RuntimeModel>, RuntimeError> {
        // Start no turn: protocol initialization and model/list cannot invoke model tools.
        let cancel = CancelToken::new().with_timeout(Duration::from_secs(15));
        let mut probe = r.clone();
        probe.messages.clear();
        let mut live = self.launch_probe(&probe, &cancel)?;
        let result = live.protocol.models(&mut live.transport, &cancel);
        live.transport.stop();
        result
    }
    fn launch_probe(&self, r: &RuntimeRequest, cancel: &CancelToken) -> Result<Live, RuntimeError> {
        self.launch(r, cancel, false)
    }
}
impl RuntimeExecutor for ExternalRuntime {
    fn descriptor(&self) -> RuntimeDescriptor {
        let info = self.config.driver.info();
        RuntimeDescriptor {
            id: self.config.id.clone(),
            name: info.name,
            modes: info.modes,
            permission_modes: info.permission_modes,
            supports_resume: !matches!(self.config.driver, Driver::Pi | Driver::Omp),
            supports_approvals: !matches!(
                self.config.driver,
                Driver::Pi | Driver::Omp | Driver::ClaudeCode | Driver::Cursor | Driver::Muse
            ),
        }
    }
    fn validate(&self, r: &RuntimeRequest) -> Result<(), RuntimeError> {
        if !self.config.enabled {
            return Err(error("runtime_disabled", "Runtime is disabled"));
        }
        if r.mode != Mode::Agent {
            return Err(error(
                "mode_unsupported",
                "This runtime currently supports agent mode only",
            ));
        }
        if r.permission_mode != PermissionMode::FullAccess {
            return Err(error(
                "scope_unsupported",
                "Verified confinement is unavailable; select explicit full access",
            ));
        }
        if r.workspace
            .as_ref()
            .is_none_or(|w| !PathBuf::from(&w.root).is_dir())
        {
            return Err(error(
                "workspace_required",
                "A selected existing workspace is required",
            ));
        }
        if r.max_steps == 0
            || r.max_steps > 128
            || r.max_tokens == 0
            || r.max_tokens > 32768
            || r.instructions.len() > 32768
            || r.messages.iter().map(|m| m.content.len()).sum::<usize>() > 1024 * 1024
        {
            return Err(error("invalid_request", "Runtime request bounds exceeded"));
        }
        if r.operation == RuntimeOperation::Resume
            && matches!(self.config.driver, Driver::Pi | Driver::Omp)
        {
            return Err(error(
                "resume_unsupported",
                "Pi and OMP native session restore is not implemented",
            ));
        }
        Ok(())
    }
    fn execute(
        &self,
        r: &RuntimeRequest,
        on_event: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        self.validate(r)?;
        let live = if r.operation == RuntimeOperation::Approval {
            let mut guard = self
                .live
                .lock()
                .map_err(|_| error("state_unavailable", "Runtime lock unavailable"))?;
            let live = guard.get(&r.run_id).ok_or_else(|| {
                error(
                    "approval_expired",
                    "Native approval process is no longer live; resume explicitly",
                )
            })?;
            let pending = live.pending.as_ref().ok_or_else(|| {
                error("approval_expired", "No live native request awaits approval")
            })?;
            let decision = r
                .approvals
                .iter()
                .find(|d| d.request_id == pending.request_id)
                .ok_or_else(|| {
                    error(
                        "approval_mismatch",
                        "Approval must match exact native request ID",
                    )
                })?;
            if decision.runtime_ref != pending.reference
                || decision.fingerprint != pending.fingerprint
                || r.runtime_ref != Some(self.reference(live))
                || live.session_id != r.session_id
            {
                return Err(error(
                    "approval_mismatch",
                    "Approval fingerprint or native reference does not match live request",
                ));
            }
            if !live.transport.process.is_running() {
                guard.remove(&r.run_id);
                return Err(error(
                    "approval_expired",
                    "Native approval process has ended",
                ));
            }
            let mut live = guard.remove(&r.run_id).unwrap();
            let pending = live.pending.take().unwrap();
            live.transport.send(json!({"jsonrpc":"2.0","id":pending.wire,"result":if decision.approved{pending.allow}else{pending.deny}}))?;
            live
        } else {
            if self
                .live
                .lock()
                .map_err(|_| error("state_unavailable", "Runtime lock unavailable"))?
                .contains_key(&r.run_id)
            {
                return Err(error(
                    "runtime_busy",
                    "Native run already waits for approval",
                ));
            }
            self.launch(r, cancel, true)?
        };
        self.drive(live, r, on_event, cancel)
    }
}
fn sanitize_json(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(aidex_events::redact_text(&s).into_owned()),
        Value::Array(a) => Value::Array(a.into_iter().map(sanitize_json).collect()),
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, v)| (k, sanitize_json(v))).collect())
        }
        v => v,
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    #[test]
    #[ignore = "requires explicit installed Codex path; isolated auth-free command/exec probe, no inference"]
    fn installed_codex_control_plane_and_vendor_scope_probe() {
        let executable = std::env::var("AIDEX_TEST_CODEX").expect("explicit Codex path required");
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let outside = root.path().join("outside.txt");
        let vault = Arc::new(
            aidex_vault::VaultService::open(
                root.path().join("vault"),
                aidex_vault::VaultServiceConfig {
                    kdf: aidex_vault::KdfParams::new(8, 1, 1).unwrap(),
                    ..Default::default()
                },
            )
            .unwrap(),
        );
        vault
            .init(
                &aidex_vault::SecretString::from("strong protocol probe passphrase".to_owned()),
                false,
            )
            .unwrap();
        let state = Arc::new(State::new(root.path().join("private"), vault.clone()).unwrap());
        let runtime = ExternalRuntime::new(
            RuntimeConfig {
                id: "probe".into(),
                driver: Driver::Codex,
                executable,
                argv: vec![],
                profile: None,
                enabled: true,
                auth: RuntimeAuth::None,
            },
            state,
            Arc::new(AuthService::new(vault).unwrap()),
            Sandbox::locate(),
            Arc::new(|| true),
        );
        let r = RuntimeRequest {
            session_id: "probe".into(),
            run_id: "probe".into(),
            parent_run_id: None,
            operation: RuntimeOperation::Start,
            mode: Mode::Agent,
            workspace: Some(RuntimeWorkspace {
                workspace_id: "probe".into(),
                root: work.to_string_lossy().into_owned(),
            }),
            permission_mode: PermissionMode::FullAccess,
            instructions: String::new(),
            model: None,
            messages: vec![],
            max_steps: 1,
            max_tokens: 1,
            runtime_ref: None,
            approvals: vec![],
        };
        let mut confined = r.clone();
        confined.permission_mode = PermissionMode::Supervised;
        assert_eq!(
            runtime.validate(&confined).unwrap_err().code,
            "scope_unsupported"
        );
        assert!(
            !runtime
                .descriptor()
                .permission_modes
                .contains(&PermissionMode::Supervised)
        );
        let cancel = CancelToken::new().with_timeout(Duration::from_secs(30));
        let mut live = runtime.launch_probe(&r, &cancel).unwrap();
        let catalog = live.protocol.models(&mut live.transport, &cancel).unwrap();
        assert!(!catalog.is_empty());
        let script = format!(
            "[System.IO.File]::WriteAllText('{}','probe')",
            outside.to_string_lossy().replace('\'', "''")
        );
        let command = if cfg!(windows) {
            vec![
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".to_owned(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script,
            ]
        } else {
            vec![
                "/bin/sh".into(),
                "-c".into(),
                format!("printf probe > '{}'", outside.display()),
            ]
        };
        let result=live.protocol.call(&mut live.transport,"command/exec",json!({"command":command,"cwd":work,"sandboxPolicy":{"type":"workspaceWrite","writableRoots":[work],"networkAccess":false},"timeoutMs":5000}),&cancel);
        println!(
            "Vendor command sandbox result: {}. Outside file created: {}",
            if result.is_ok() {
                "response"
            } else {
                "refused"
            },
            outside.exists()
        );
        // Record vendor behavior, while the production adapter fails validation
        // before launching any request that relies on this unverified boundary.
        assert_eq!(
            runtime.validate(&confined).unwrap_err().code,
            "scope_unsupported"
        );
        live.transport.stop();
        assert!(runtime.sandbox.registry().is_empty());
    }
}
