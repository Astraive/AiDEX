use crate::{
    Driver, RuntimeAuth, RuntimeConfig,
    binding::CredentialBinding,
    profiles,
    protocol::{Protocol, prompt},
    snapshot::Capture,
    state::{State, error},
    transport::Transport,
};
use aidex_auth::AuthService;
use aidex_core::CancelToken;
use aidex_native_sessions::*;
use aidex_sandbox::{
    CommandSpec, Mount, NetworkMode, ResourceLimits, Sandbox, SandboxLevel, SandboxRequest,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
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
/// What a resume reference binds: the native branch and its encrypted snapshot,
/// the account and auth configuration, and the credential the launch used.
/// Every field is value-free resume metadata that `aidex_events::redact_text`
/// leaves unchanged; the harness refuses a reference it would alter.
struct Evidence {
    snapshot: String,
    /// Digest of the last snapshot this launch saved; `None` while none of its
    /// snapshots can be resumed (the reference is then not resumable).
    snapshot_digest: Option<String>,
    /// Closed code of the last snapshot failure.
    snapshot_error: Option<&'static str>,
    run_id: String,
    session_id: String,
    account_revision: Option<String>,
    /// Digest of the driver, profile and auth configuration.
    config_digest: String,
    credential_binding: CredentialBinding,
    native_branch_id: String,
}
struct Live {
    transport: Transport,
    protocol: Protocol,
    profile: tempfile::TempDir,
    evidence: Evidence,
    /// Snapshot name daemons before branch-scoped snapshots used for this
    /// session; removed after the first new snapshot.
    legacy_snapshot: String,
    /// Whether this launch already tried its first checkpoint.
    checkpointed: bool,
    /// Digest of the managed account credential this launch restored, to save
    /// a credential the vendor refreshed; `None` without a managed profile.
    credential: Option<String>,
    pending: Option<Pending>,
    text: String,
    tools: BTreeSet<String>,
}
impl Live {
    fn home(&self) -> PathBuf {
        self.profile.path().join("home")
    }
}
pub struct ExternalRuntime {
    pub(crate) config: RuntimeConfig,
    state: Arc<State>,
    auth: Arc<AuthService>,
    sandbox: Sandbox,
    network: NetworkGate,
    live: Mutex<BTreeMap<String, Live>>,
}
fn resume_unavailable(message: &str) -> RuntimeError {
    error("resume_unavailable", message)
}
/// Recovery failures are refusals to resume (the harness then starts a new
/// native session with the transcript); a locked vault stays a locked vault.
fn as_resume_refusal(e: RuntimeError) -> RuntimeError {
    if e.code == "vault_locked" {
        e
    } else {
        resume_unavailable("Encrypted native snapshot evidence is unavailable")
    }
}
/// The closed code a reference records when its snapshot could not be saved.
fn snapshot_failure(e: &RuntimeError) -> &'static str {
    match e.code.as_str() {
        "state_limit" => "state_limit",
        "state_busy" => "state_busy",
        "state_corrupt" => "state_corrupt",
        "vault_locked" => "vault_locked",
        _ => "state_unavailable",
    }
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
    /// Pi and OMP have no native restore: their homes are never snapshotted
    /// and their references are never resumable.
    fn resumable(&self) -> bool {
        !matches!(self.config.driver, Driver::Pi | Driver::Omp)
    }
    /// Codex profile mode: the encrypted account profile is the only credential.
    fn managed_account(&self) -> bool {
        self.config.driver == Driver::Codex
            && self.config.profile.is_some()
            && matches!(self.config.auth, RuntimeAuth::None)
    }
    fn capture(&self) -> Capture<'static> {
        match self.config.driver {
            // Codex regenerates `.tmp` (argv0 tool copies, Windows byte-range
            // locked files) every launch; the account profile record is the one
            // authoritative copy of its managed credential.
            Driver::Codex if self.config.profile.is_some() => {
                Capture::skipping(&[".tmp", "auth.json"])
            }
            Driver::Codex => Capture::skipping(&[".tmp"]),
            _ => Capture::skipping(&[]),
        }
    }
    /// Encrypt `home` as the branch snapshot. A failure never fails the turn:
    /// the caller continues with a reference that is not resumable.
    fn save_snapshot(&self, name: &str, legacy: &str, home: &Path) -> Result<String, &'static str> {
        let saved = self
            .state
            .snapshot_native(name, home, self.capture())
            .and_then(|()| self.state.snapshot_digest(name));
        match saved {
            Ok(evidence) => {
                // Nothing resumes from the pre-branch name any more.
                let _ = self.state.remove(legacy);
                Ok(evidence)
            }
            Err(e) => {
                // A snapshot that cannot be refreshed is not left resumable.
                let _ = self.state.remove(name);
                Err(snapshot_failure(&e))
            }
        }
    }
    fn refresh_snapshot(&self, live: &mut Live) {
        if !self.resumable() {
            // Nothing resumes this driver's sessions, so no snapshot is kept;
            // one an older daemon saved is never restored either.
            let _ = self.state.remove(&live.legacy_snapshot);
            return;
        }
        match self.save_snapshot(&live.evidence.snapshot, &live.legacy_snapshot, &live.home()) {
            Ok(evidence) => {
                live.evidence.snapshot_digest = Some(evidence);
                live.evidence.snapshot_error = None;
            }
            Err(code) => {
                live.evidence.snapshot_digest = None;
                live.evidence.snapshot_error = Some(code);
            }
        }
    }
    /// Keep a credential the vendor refreshed during the turn. Best effort: a
    /// failure leaves the stored profile as it was, and a changed account is
    /// never overwritten.
    fn persist_credentials(&self, live: &mut Live) {
        let (Some(profile), Some(restored)) = (&self.config.profile, &live.credential) else {
            return;
        };
        let home = live.home();
        let Ok(current) = profiles::credential_digest(&home) else {
            return;
        };
        if current == *restored {
            return;
        }
        if profiles::persist_native_credentials(
            &self.state,
            profile,
            &home,
            &live.evidence.account_revision,
        )
        .is_ok()
        {
            live.credential = Some(current);
        }
    }
    fn reference(&self, protocol: &Protocol, e: &Evidence) -> Value {
        let resumable = e.snapshot_digest.is_some();
        let mut reference = json!({"runtime_id":self.config.id,"native_session_id":protocol.session,"native_turn_id":protocol.turn,"snapshot_id":e.snapshot,"snapshot_digest":e.snapshot_digest,"run_id":e.run_id,"session_id":e.session_id,"account_revision":e.account_revision,"config_digest":e.config_digest,"credential_binding":e.credential_binding.evidence(),"native_branch_id":if resumable {json!(e.native_branch_id)} else {Value::Null}});
        if let Some(code) = e.snapshot_error {
            reference["snapshot_error"] = json!(code);
        }
        reference
    }
    /// The native branch a Resume continues, from a reference bound to one.
    fn resume_branch(&self, r: &RuntimeRequest) -> Result<String, RuntimeError> {
        r.runtime_ref
            .as_ref()
            .filter(|reference| is_resumable_reference(reference))
            .and_then(|reference| reference["native_branch_id"].as_str())
            .filter(|branch| branch.len() <= 128)
            .map(str::to_owned)
            .ok_or_else(|| {
                resume_unavailable(
                    "No bound native branch to resume; continue with a transcript handoff",
                )
            })
    }
    /// Every check runs before a process starts: a refusal leaves nothing to
    /// clean up, so the harness can start a new native session instead.
    fn verify_resume(
        &self,
        r: &RuntimeRequest,
        evidence: &Evidence,
        home: &Path,
    ) -> Result<(), RuntimeError> {
        let reference = r
            .runtime_ref
            .as_ref()
            .ok_or_else(|| resume_unavailable("A native session reference is required"))?;
        if reference["runtime_id"] != self.config.id
            || reference["session_id"] != r.session_id
            || reference["run_id"].as_str().is_none_or(str::is_empty)
            || reference["snapshot_id"] != evidence.snapshot
            || reference["account_revision"] != json!(evidence.account_revision)
            || reference["config_digest"] != evidence.config_digest
            || reference["credential_binding"] != evidence.credential_binding.evidence()
            || reference["native_session_id"]
                .as_str()
                .is_none_or(str::is_empty)
        {
            return Err(resume_unavailable(
                "Native reference does not match this runtime, session, account or credential",
            ));
        }
        let saved = self
            .state
            .snapshot_digest(&evidence.snapshot)
            .map_err(as_resume_refusal)?;
        if reference["snapshot_digest"].as_str() != Some(saved.as_str()) {
            return Err(resume_unavailable(
                "Encrypted native snapshot evidence changed",
            ));
        }
        // A snapshot removed since its digest was read is missing evidence too.
        if !self
            .state
            .restore(&evidence.snapshot, home)
            .map_err(as_resume_refusal)?
        {
            return Err(resume_unavailable(
                "Encrypted native snapshot evidence is missing",
            ));
        }
        Ok(())
    }
    fn launch(
        &self,
        r: &RuntimeRequest,
        cancel: &CancelToken,
        begin: bool,
        on_event: Option<&mut dyn FnMut(&RuntimeEvent) -> RuntimeControl>,
    ) -> Result<Live, RuntimeError> {
        if !(self.network)() {
            return Err(error(
                "offline_refused",
                "External runtimes are paused: offline mode, a locked vault or an emergency stop",
            ));
        }
        if cancel.is_stopped() {
            return Err(error("cancelled", "Runtime cancelled"));
        }
        let resume = r.operation == RuntimeOperation::Resume;
        // A binding that changed is a refusal to resume; for a new launch it is
        // an ordinary account change.
        let binding_refusal = |e: RuntimeError| {
            if resume && e.code == "account_changed" {
                resume_unavailable("Runtime credential changed since the native session started")
            } else {
                e
            }
        };
        let native_branch_id = if resume {
            self.resume_branch(r)?
        } else {
            r.run_id.clone()
        };
        let profile = tempfile::Builder::new()
            .prefix("aidex-runtime-")
            .tempdir()
            .map_err(|_| error("state_unavailable", "Runtime temporary profile unavailable"))?;
        let home = profile.path().join("home");
        std::fs::create_dir_all(&home)
            .map_err(|_| error("state_unavailable", "Runtime home unavailable"))?;
        let account_revision = self
            .config
            .profile
            .as_ref()
            .map(|profile| self.state.account_revision(profile))
            .transpose()?
            .flatten();
        let mut evidence = Evidence {
            snapshot: format!(
                "runtime-{}-{}.sealed",
                digest(self.config.id.as_bytes()),
                digest(format!("{}:{}", r.session_id, native_branch_id).as_bytes())
            ),
            snapshot_digest: None,
            snapshot_error: None,
            run_id: r.run_id.clone(),
            session_id: r.session_id.clone(),
            account_revision,
            config_digest: digest(
                json!({"driver":self.config.driver,"profile":self.config.profile,"auth":self.config.auth})
                    .to_string()
                    .as_bytes(),
            ),
            credential_binding: CredentialBinding::capture(
                &self.config.auth,
                &self.state,
                &self.auth,
            )?,
            native_branch_id,
        };
        let legacy_snapshot = format!(
            "runtime-{}.sealed",
            digest(format!("{}:{}", self.config.id, r.session_id).as_bytes())
        );
        if resume {
            self.verify_resume(r, &evidence, &home)?;
            evidence.snapshot_digest = r
                .runtime_ref
                .as_ref()
                .and_then(|reference| reference["snapshot_digest"].as_str().map(str::to_owned));
        }
        // A selected managed profile is encrypted input, never a raw host login
        // home, and it outranks a credential inside an older session snapshot.
        let mut credential = None;
        if let Some(selected) = &self.config.profile {
            if self.config.driver == Driver::Codex {
                let _ = std::fs::remove_file(home.join("auth.json"));
            }
            let restored = self
                .state
                .restore(&profiles::profile_record(selected), &home)?;
            if !restored && self.managed_account() {
                return Err(error(
                    "auth_required",
                    "Managed account is signed out; connect it before using this runtime",
                ));
            }
            if restored && self.config.driver == Driver::Codex {
                credential = profiles::credential_digest(&home).ok();
            }
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
        if self.config.driver == Driver::Codex {
            // Codex never reaches the host keyring: credentials stay in the
            // app-owned home that AiDEX encrypts.
            args.extend(["-c".into(), "cli_auth_credentials_store=\"file\"".into()]);
            if credential.is_some() {
                secrets.extend(profiles::credential_secrets(&home)?);
            }
        }
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
                let secret = evidence
                    .credential_binding
                    .vault_secret(&self.state, &self.config.id, secret_name)
                    .map_err(binding_refusal)?;
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
            if resume
                && let Some(id) = r
                    .runtime_ref
                    .as_ref()
                    .and_then(|v| v["native_session_id"].as_str())
            {
                args.extend(["--resume".into(), id.into()]);
            }
            args.push(prompt(r, resume && r.runtime_ref.is_none()));
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
        // The credential the process receives is the one the reference binds.
        evidence
            .credential_binding
            .verify(&self.config.auth, &self.state, &self.auth)
            .map_err(binding_refusal)?;
        if cancel.is_stopped() || !(self.network)() {
            return Err(error("cancelled", "Runtime launch cancelled"));
        }
        let process = self
            .sandbox
            .spawn_interactive(&plan, CommandSpec::new(executable).args(args))
            .map_err(|_| {
                error(
                    "launch_failed",
                    "Configured runtime executable could not be launched",
                )
            })?;
        let mut live = Live {
            transport: Transport::new(process, secrets),
            protocol: Protocol::new(self.config.driver),
            profile,
            evidence,
            legacy_snapshot,
            checkpointed: false,
            credential,
            pending: None,
            text: String::new(),
            tools: BTreeSet::new(),
        };
        // The original cancellation token remains effective while execute has returned
        // WaitingApproval. The watcher owns only a weak process handle, and the
        // gate also closes when the vault locks or an emergency stop begins.
        let weak = Arc::downgrade(&live.transport.process);
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
        if let Err(e) = self.start_turn(&mut live, r, cancel, begin, on_event) {
            self.abandon(&mut live);
            return Err(e);
        }
        Ok(live)
    }
    /// Initialize the protocol and, for a turn, open the native session: its
    /// checkpoint is saved and reported before the turn is sent.
    fn start_turn(
        &self,
        live: &mut Live,
        r: &RuntimeRequest,
        cancel: &CancelToken,
        begin: bool,
        mut on_event: Option<&mut dyn FnMut(&RuntimeEvent) -> RuntimeControl>,
    ) -> Result<(), RuntimeError> {
        live.protocol.initialize(&mut live.transport, cancel)?;
        if !begin {
            return Ok(());
        }
        let home = live.home();
        let Live {
            transport,
            protocol,
            evidence,
            legacy_snapshot,
            checkpointed,
            ..
        } = live;
        protocol.begin(transport, r, cancel, &mut |protocol| {
            *checkpointed = true;
            if !self.resumable() {
                return Ok(());
            }
            match self.save_snapshot(&evidence.snapshot, legacy_snapshot, &home) {
                Ok(saved) => {
                    evidence.snapshot_digest = Some(saved);
                    evidence.snapshot_error = None;
                    let reference = self.reference(protocol, evidence);
                    if on_event.as_mut().is_some_and(|callback| {
                        callback(&RuntimeEvent::SessionStarted {
                            runtime_ref: reference,
                        }) == RuntimeControl::Pause
                    }) {
                        return Err(error(
                            "interrupted",
                            "Native session checkpoint was refused before turn execution",
                        ));
                    }
                }
                // Continue without a checkpoint; the reference is not resumable.
                Err(code) => {
                    evidence.snapshot_digest = None;
                    evidence.snapshot_error = Some(code);
                }
            }
            Ok(())
        })
    }
    /// End a launch that will not continue: stop its tree, then keep a
    /// credential the vendor refreshed before it ended.
    fn abandon(&self, live: &mut Live) {
        live.transport.stop();
        self.persist_credentials(live);
    }
    fn approval(
        &self,
        live: &Live,
        r: &RuntimeRequest,
        v: &Value,
    ) -> Result<Option<Pending>, RuntimeError> {
        if !is_approval_request(v) {
            return Ok(None);
        }
        let method = v["method"].as_str().unwrap_or("");
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
        let mut reference = self.reference(&live.protocol, &live.evidence);
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
    /// Run the turn to completion or to an approval pause. A paused launch
    /// stays live for the decision; a failed one is stopped, and a credential
    /// the vendor refreshed before it failed is kept.
    fn drive(
        &self,
        mut live: Live,
        r: &RuntimeRequest,
        on_event: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        match self.turn(&mut live, r, on_event, cancel) {
            Ok(response) if response.status == RuntimeStatus::WaitingApproval => {
                self.live
                    .lock()
                    .map_err(|_| error("state_unavailable", "Runtime lock unavailable"))?
                    .insert(r.run_id.clone(), live);
                Ok(response)
            }
            Ok(response) => Ok(response),
            Err(e) => {
                self.abandon(&mut live);
                Err(e)
            }
        }
    }
    fn turn(
        &self,
        live: &mut Live,
        r: &RuntimeRequest,
        on_event: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        loop {
            if !(self.network)() {
                live.transport.stop();
                return Err(error(
                    "offline_refused",
                    "External runtimes were paused: offline mode, a locked vault or an emergency stop",
                ));
            }
            let v = live.protocol.next(&mut live.transport, cancel)?;
            if matches!(self.config.driver, Driver::ClaudeCode | Driver::Cursor) {
                // Only the root session identity is the native session; it is
                // fixed by its first frame, and subagent frames never replace it.
                if v["parent_tool_use_id"].is_null() && v["session_id"].is_string() {
                    let id = crate::protocol::required(&v, "/session_id")?;
                    if !live.protocol.session.is_empty() && live.protocol.session != id {
                        live.transport.stop();
                        return Err(error(
                            "protocol_error",
                            "Runtime changed the root native session identity",
                        ));
                    }
                    live.protocol.session = id;
                    if !live.checkpointed {
                        live.checkpointed = true;
                        self.refresh_snapshot(live);
                        if live.evidence.snapshot_digest.is_some()
                            && on_event(&RuntimeEvent::SessionStarted {
                                runtime_ref: self.reference(&live.protocol, &live.evidence),
                            }) == RuntimeControl::Pause
                        {
                            live.transport.stop();
                            return Err(error(
                                "interrupted",
                                "Native session checkpoint was refused",
                            ));
                        }
                    }
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
            if is_approval_request(&v) {
                // The paused reference covers the native state the decision continues.
                self.refresh_snapshot(live);
                self.persist_credentials(live);
            }
            if let Some(pending) = self.approval(live, r, &v)? {
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
                    let reference = self.reference(&live.protocol, &live.evidence);
                    live.pending = Some(pending);
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
                live.transport.stop();
                // The vendor's work already ran: a snapshot that cannot be saved
                // leaves the turn completed with a reference that is not resumable.
                self.refresh_snapshot(live);
                self.persist_credentials(live);
                let reference = self.reference(&live.protocol, &live.evidence);
                return Ok(RuntimeResponse {
                    status: RuntimeStatus::Completed,
                    runtime_ref: Some(reference),
                    message: Some(aidex_providers::ChatMessage::assistant(std::mem::take(
                        &mut live.text,
                    ))),
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
        // The CLI may refresh its credential while it starts.
        self.abandon(&mut live);
        result
    }
    fn launch_probe(&self, r: &RuntimeRequest, cancel: &CancelToken) -> Result<Live, RuntimeError> {
        self.launch(r, cancel, false, None)
    }
}
fn is_approval_request(v: &Value) -> bool {
    v.get("id").is_some()
        && matches!(
            v["method"].as_str().unwrap_or(""),
            "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "session/request_permission"
                | "approval/request"
        )
}
impl RuntimeExecutor for ExternalRuntime {
    fn descriptor(&self) -> RuntimeDescriptor {
        let info = self.config.driver.info();
        RuntimeDescriptor {
            id: self.config.id.clone(),
            name: info.name,
            modes: info.modes,
            permission_modes: info.permission_modes,
            supports_resume: self.resumable(),
            supports_approvals: !matches!(
                self.config.driver,
                Driver::Pi | Driver::Omp | Driver::ClaudeCode | Driver::Cursor | Driver::Muse
            ),
        }
    }
    fn validate(&self, r: &RuntimeRequest) -> Result<(), RuntimeError> {
        if self.state.login_pending(&self.config.id)? {
            return Err(error(
                "auth_pending",
                "Complete or cancel the pending official login before starting a native turn",
            ));
        }
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
                || r.runtime_ref != Some(self.reference(&live.protocol, &live.evidence))
                || live.evidence.session_id != r.session_id
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
            // Release the live map first: a failed send drops `live`, whose
            // transport waits for the native tree to be reaped.
            drop(guard);
            // The decision continues under the credential the request was made with.
            if let Err(e) =
                live.evidence
                    .credential_binding
                    .verify(&self.config.auth, &self.state, &self.auth)
            {
                self.abandon(&mut live);
                return Err(e);
            }
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
            self.launch(r, cancel, true, Some(on_event))?
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
        // An actual encrypted native-home snapshot of the installed CLI's home
        // (its regenerated `.tmp` excluded) stays within the recovery bounds.
        runtime
            .state
            .snapshot_native("probe.sealed", &live.home(), runtime.capture())
            .unwrap();
        assert!(runtime.sandbox.registry().is_empty());
    }
}
