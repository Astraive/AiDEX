use crate::{
    Driver, RuntimeAuth, RuntimeConfig,
    binding::CredentialBinding,
    manager::Configs,
    profiles,
    protocol::{Protocol, prompt, uuid},
    snapshot::Capture,
    state::{Lease, ProfileLease, ProfileUse, State, error},
    transport::Transport,
};
use aidex_auth::AuthService;
use aidex_core::CancelToken;
use aidex_native_sessions::*;
use aidex_sandbox::{
    CommandSpec, ManagedProcess, Mount, NetworkMode, ResourceLimits, Sandbox, SandboxLevel,
    SandboxRequest,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, Weak},
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroizing;
pub(crate) type NetworkGate = Arc<dyn Fn() -> bool + Send + Sync>;
/// Launches paused for an approval decision, by run id.
type Parked = Mutex<BTreeMap<String, Live>>;
/// How long a launch of a managed account waits for another launch of it to
/// get past its credential refresh before it starts anyway.
const LEASE_WAIT: Duration = Duration::from_secs(30);
/// How often a running launch's watcher looks for a credential the vendor
/// refreshed.
const CREDENTIAL_POLL: Duration = Duration::from_millis(250);
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
/// The managed account credential restored into one launch's home, kept in
/// step with the refreshes the vendor makes there. The vendor rotates the
/// refresh token, so a refresh is saved as soon as it is seen, and only over
/// the credential this launch started from: a newer one that another launch
/// of the same account saved meanwhile is never overwritten.
struct CredentialSync {
    state: Arc<State>,
    profile: String,
    revision: Option<String>,
    home: PathBuf,
    progress: Mutex<Progress>,
    _use: ProfileUse,
}
struct Progress {
    /// Digest of the credential the stored profile held when this launch
    /// restored or last saved it.
    saved: String,
    /// Digest of the credential last read from the home.
    seen: String,
    /// Length and modification time of that read, to skip an unchanged file.
    stamp: Option<(u64, Option<SystemTime>)>,
    /// Another launch saved a newer credential, or the account changed or
    /// signed out: this launch saves nothing more.
    superseded: bool,
    /// A credential the transport does not scrub yet appeared in the home.
    fresh: bool,
    /// Held until this launch is past the point where the vendor refreshes a
    /// stale credential (see [`ProfileLease`]).
    lease: Option<ProfileLease>,
}
fn credential_stamp(home: &Path) -> Option<(u64, Option<SystemTime>)> {
    std::fs::symlink_metadata(home.join("auth.json"))
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}
impl CredentialSync {
    fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(PoisonError::into_inner)
    }
    /// Save a credential the vendor refreshed. Best effort: a locked vault or
    /// a failed write is retried at the next look. Unless `thorough`, an
    /// `auth.json` whose length and modification time did not change since
    /// the last look is not read again (the watcher's frequent polls).
    fn sync(&self, thorough: bool) {
        let mut progress = self.progress();
        if progress.superseded {
            return;
        }
        let stamp = credential_stamp(&self.home);
        if !thorough
            && stamp.is_some()
            && stamp == progress.stamp
            && progress.seen == progress.saved
        {
            return;
        }
        let Ok(current) = profiles::credential_digest(&self.home) else {
            // Missing or mid-write: looked at again later.
            return;
        };
        progress.stamp = stamp;
        if current != progress.seen {
            progress.seen = current.clone();
            progress.fresh = true;
        }
        if current == progress.saved {
            return;
        }
        let Ok(_guard) = self.state.credential_guard() else {
            return;
        };
        match self.state.stored_credential_digest(&self.profile) {
            Ok(Some(stored)) if stored == progress.saved => {}
            // Another launch saved a newer credential, or the account was
            // signed out.
            Ok(_) => {
                progress.superseded = true;
                progress.lease = None;
                return;
            }
            Err(_) => return,
        }
        match profiles::persist_native_credentials(
            &self.state,
            &self.profile,
            &self.home,
            &self.revision,
        ) {
            Ok(()) => {
                progress.saved = self
                    .state
                    .stored_credential_digest(&self.profile)
                    .ok()
                    .flatten()
                    .unwrap_or(current);
                // Other launches of the account may start from it now.
                progress.lease = None;
            }
            Err(e) if matches!(e.code.as_str(), "account_changed" | "auth_required") => {
                progress.superseded = true;
                progress.lease = None;
            }
            Err(_) => {}
        }
    }
    fn release_lease(&self) {
        let lease = self.progress().lease.take();
        drop(lease);
    }
    fn take_fresh(&self) -> bool {
        std::mem::take(&mut self.progress().fresh)
    }
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
    /// The managed account credential this launch restored; `None` without
    /// a managed profile.
    credential: Option<Arc<CredentialSync>>,
    /// Whether the vendor already reported turn output (so it authenticated).
    responded: bool,
    /// Scopes the vendor's per-connection approval request ids to this
    /// launch, so a later launch of the same run reusing a wire id never
    /// matches an earlier approval.
    launch: String,
    /// The launch's cancellation: a parked launch whose cancellation stopped
    /// is not waiting any more.
    cancel: CancelToken,
    pending: Option<Pending>,
    text: String,
    tools: BTreeSet<String>,
}
impl Live {
    fn home(&self) -> PathBuf {
        self.profile.path().join("home")
    }
    /// Save a credential the vendor refreshed and scrub it from later frames.
    fn keep_credential(&mut self) {
        let Some(sync) = &self.credential else {
            return;
        };
        sync.sync(true);
        if sync.take_fresh()
            && let Ok(secrets) = profiles::credential_secrets(&self.home())
        {
            self.transport.add_secrets(secrets);
        }
    }
    /// The vendor answered with turn output, so it authenticated: a stale
    /// credential it refreshed first is saved, and other launches of the
    /// account may start.
    fn responded(&mut self) {
        if std::mem::replace(&mut self.responded, true) {
            return;
        }
        self.keep_credential();
        if let Some(sync) = &self.credential {
            sync.release_lease();
        }
    }
}
/// End a launch that will not continue: stop its tree, keep a credential the
/// vendor refreshed before it ended and let other launches of the account
/// start. Dropping the launch afterwards removes its temporary home with the
/// restored snapshot and credential, and zeroizes the secrets it held.
fn abandon(live: &mut Live) {
    live.transport.stop();
    if let Some(sync) = &live.credential {
        sync.sync(true);
        sync.release_lease();
    }
}
/// Take the launch parked for `run_id` out of `parked` when `evict` says so.
fn take_parked(parked: &Parked, run_id: &str, evict: impl FnOnce(&Live) -> bool) -> Option<Live> {
    let mut map = parked.lock().unwrap_or_else(PoisonError::into_inner);
    if map.get(run_id).is_some_and(evict) {
        map.remove(run_id)
    } else {
        None
    }
}
/// Watch one launch until it is dropped. Its tree is killed once its
/// cancellation stops or the gate closes (a vault lock, emergency stop or
/// going offline), also while it waits for an approval; a credential the
/// vendor refreshes is saved as it appears; and a launch parked for an
/// approval whose tree ended (killed, exited or past its wall timeout) is
/// released here, since no thread drives it. Only the entry of this very
/// launch is released, never a newer launch of the same run.
fn watch(
    process: Weak<ManagedProcess>,
    parked: Weak<Parked>,
    run_id: String,
    cancel: CancelToken,
    network: NetworkGate,
    credential: Option<Weak<CredentialSync>>,
) {
    let mut killed = false;
    let mut polled = Instant::now();
    loop {
        let Some(tree) = process.upgrade() else {
            return;
        };
        let running = tree.is_running();
        if running && !killed && (cancel.is_stopped() || !network()) {
            let _ = tree.kill_tree();
            killed = true;
        }
        if running && !killed && polled.elapsed() >= CREDENTIAL_POLL {
            polled = Instant::now();
            if let Some(sync) = credential.as_ref().and_then(Weak::upgrade) {
                sync.sync(false);
            }
        }
        if (killed || !running)
            && let Some(parked) = parked.upgrade()
            && let Some(mut live) = take_parked(&parked, &run_id, |live| {
                Arc::ptr_eq(&live.transport.process, &tree)
            })
        {
            drop(tree);
            abandon(&mut live);
            return;
        }
        drop(tree);
        std::thread::sleep(Duration::from_millis(25));
    }
}
/// Remove the branch snapshot `reference` names when it is one of
/// `runtime`'s; anything else is left alone.
pub(crate) fn forget_snapshot(state: &State, runtime: &str, reference: &Value) -> bool {
    let Some(name) = reference.get("snapshot_id").and_then(Value::as_str) else {
        return false;
    };
    reference.get("runtime_id").and_then(Value::as_str) == Some(runtime)
        && State::is_branch_snapshot(runtime, name)
        && state.root.join(name).is_file()
        && state.remove(name).is_ok()
}
pub struct ExternalRuntime {
    pub(crate) config: RuntimeConfig,
    state: Arc<State>,
    /// The saved configuration list, to notice that this executor is stale.
    configs: Arc<Configs>,
    auth: Arc<AuthService>,
    sandbox: Sandbox,
    network: NetworkGate,
    live: Arc<Parked>,
}
/// Digest of what decides a runtime's credential: its driver, managed
/// profile and auth configuration (a reference's `config_digest`).
fn config_digest(config: &RuntimeConfig) -> String {
    digest(
        json!({"driver":config.driver,"profile":config.profile,"auth":config.auth})
            .to_string()
            .as_bytes(),
    )
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
        configs: Arc<Configs>,
        auth: Arc<AuthService>,
        sandbox: Sandbox,
        network: NetworkGate,
    ) -> Self {
        Self {
            config,
            state,
            configs,
            auth,
            sandbox,
            network,
            live: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    /// Release the launch of `run_id` parked for an approval decision (a
    /// cancelled, failed or abandoned run): stop its tree, keep a credential
    /// the vendor refreshed, and remove its temporary home with the restored
    /// snapshot and credential. Returns whether one was parked. A later
    /// `Resume` of the run launches normally.
    pub fn release_run(&self, run_id: &str) -> bool {
        let Some(mut live) = take_parked(&self.live, run_id, |_| true) else {
            return false;
        };
        abandon(&mut live);
        true
    }
    /// Release every launch parked for an approval decision (a vault lock,
    /// an emergency stop, going offline). Returns how many were released.
    pub fn release_paused(&self) -> usize {
        let drained =
            std::mem::take(&mut *self.live.lock().unwrap_or_else(PoisonError::into_inner));
        // Every tree is told to stop first, so the waits below overlap.
        for live in drained.values() {
            let _ = live.transport.process.kill_tree();
        }
        let released = drained.len();
        for (_, mut live) in drained {
            abandon(&mut live);
        }
        released
    }
    /// How many launches wait for an approval decision.
    pub fn parked_runs(&self) -> usize {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
    /// Remove the native session snapshot a saved reference of this runtime
    /// names: a refused, superseded or deleted native thread. Only this
    /// runtime's branch snapshots (`runtime-<digest(id)>-<branch>.sealed`)
    /// are removed. Best effort; returns whether one was named and is gone.
    pub fn forget_snapshot(&self, reference: &Value) -> bool {
        forget_snapshot(&self.state, &self.config.id, reference)
    }
    /// Whether a launch parked for an approval still waits for its decision:
    /// its tree runs, its cancellation did not stop and the gate is `open`
    /// (read before the live map is locked).
    fn waiting(live: &Live, open: bool) -> bool {
        open && live.transport.process.is_running() && !live.cancel.is_stopped()
    }
    /// Refuse a launch of an executor whose runtime's saved driver, managed
    /// profile or auth changed since it was made (`account_changed`), or
    /// whose runtime was removed (`not_found`): a login binds its account
    /// from a worker thread, and an engine that could not reload its
    /// runtimes then (another external run was active) still holds this
    /// executor with the previous credential. The caller reloads the
    /// runtimes ([`crate::RuntimeManager::executors`]) and tries again.
    fn current(&self) -> Result<(), RuntimeError> {
        match self.configs.get(&self.config.id)? {
            Some(saved) if config_digest(&saved) == config_digest(&self.config) => Ok(()),
            Some(_) => Err(error(
                "account_changed",
                "Runtime account or authentication changed since this runtime was loaded; reload runtimes and try again",
            )),
            None => Err(error("not_found", "Runtime configuration was removed")),
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
        self.current()?;
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
        let profile = self.state.temp_home("aidex-runtime-")?;
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
            config_digest: config_digest(&self.config),
            credential_binding: CredentialBinding::capture(
                &self.config.auth,
                &self.state,
                &self.auth,
            )?,
            native_branch_id,
        };
        let legacy_snapshot = State::legacy_snapshot(&self.config.id, &r.session_id);
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
            // Another launch of the account may be refreshing the rotating
            // refresh token the stored profile still holds: start from what
            // it saves once it is past that point.
            let lease = if self.config.driver == Driver::Codex {
                match self.state.lease_profile(selected, LEASE_WAIT, &|| {
                    cancel.is_stopped() || !(self.network)()
                }) {
                    Lease::Held(lease) => Some(lease),
                    Lease::TimedOut => None,
                    Lease::Stopped => {
                        return Err(error("cancelled", "Runtime launch cancelled"));
                    }
                }
            } else {
                None
            };
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
            if restored
                && self.config.driver == Driver::Codex
                && let Ok(restored) = profiles::credential_digest(&home)
            {
                credential = Some(Arc::new(CredentialSync {
                    state: self.state.clone(),
                    profile: selected.clone(),
                    revision: evidence.account_revision.clone(),
                    home: home.clone(),
                    progress: Mutex::new(Progress {
                        saved: restored.clone(),
                        seen: restored,
                        stamp: credential_stamp(&home),
                        superseded: false,
                        fresh: false,
                        lease,
                    }),
                    _use: self.state.use_profile(selected),
                }));
            }
        }
        let cwd = PathBuf::from(&r.workspace.as_ref().unwrap().root);
        let mut request = SandboxRequest::new(SandboxLevel::Brokered, &cwd, profile.path())
            .with_mount(Mount::read_write(&cwd))
            .with_network(NetworkMode::Normal)
            .with_limits(ResourceLimits {
                wall_timeout: Some(Duration::from_secs(300)),
                max_output_bytes: crate::transport::MAX_OUTPUT,
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
            responded: false,
            launch: uuid(),
            cancel: cancel.clone(),
            pending: None,
            text: String::new(),
            tools: BTreeSet::new(),
        };
        // The original cancellation token remains effective while execute has
        // returned WaitingApproval. The watcher owns only weak handles, and the
        // gate also closes when the vault locks or an emergency stop begins.
        let watched = (
            Arc::downgrade(&live.transport.process),
            Arc::downgrade(&self.live),
            r.run_id.clone(),
            cancel.clone(),
            self.network.clone(),
            live.credential.as_ref().map(Arc::downgrade),
        );
        std::thread::spawn(move || {
            let (process, parked, run_id, cancel, network, credential) = watched;
            watch(process, parked, run_id, cancel, network, credential);
        });
        if let Err(e) = self.start_turn(&mut live, r, cancel, begin, on_event) {
            abandon(&mut live);
            // A session that could not be reopened because the launch was
            // stopped is a stop, not a refusal to resume.
            if e.code == "resume_unavailable" {
                if cancel.is_stopped() {
                    return Err(error("cancelled", "Runtime cancelled"));
                }
                if !(self.network)() {
                    return Err(error(
                        "offline_refused",
                        "External runtimes were paused: offline mode, a locked vault or an emergency stop",
                    ));
                }
            }
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
        // Vendors number requests per connection, so a later launch of the
        // same run (a recovery resume) reuses wire ids: the id the harness
        // sees is scoped to this launch, and only the reply uses the wire id.
        let request_id = format!("{}:{}", live.launch, wire);
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
    /// is parked for the decision until it is answered, released
    /// ([`Self::release_run`], [`Self::release_paused`]), superseded by a
    /// `Resume` of its run, or its watcher sees its tree end; a failed one is
    /// stopped, and a credential the vendor refreshed before it failed is
    /// kept.
    fn drive(
        &self,
        mut live: Live,
        r: &RuntimeRequest,
        on_event: &mut dyn FnMut(&RuntimeEvent) -> RuntimeControl,
        cancel: &CancelToken,
    ) -> Result<RuntimeResponse, RuntimeError> {
        match self.turn(&mut live, r, on_event, cancel) {
            Ok(response) if response.status == RuntimeStatus::WaitingApproval => {
                let replaced = self
                    .live
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(r.run_id.clone(), live);
                if let Some(mut replaced) = replaced {
                    abandon(&mut replaced);
                }
                Ok(response)
            }
            Ok(response) => Ok(response),
            Err(e) => {
                abandon(&mut live);
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
            if live
                .credential
                .as_ref()
                .is_some_and(|sync| sync.take_fresh())
                && let Ok(secrets) = profiles::credential_secrets(&live.home())
            {
                // The watcher saw a refreshed credential: frames decoded from
                // now on are scrubbed of it too.
                live.transport.add_secrets(secrets);
            }
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
                live.responded();
                live.keep_credential();
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
            let events = live.protocol.events(&v);
            if !events.is_empty() {
                live.responded();
            }
            for event in events {
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
                live.responded();
                live.keep_credential();
                if let Some(sync) = &live.credential {
                    sync.release_lease();
                }
                let reference = self.reference(&live.protocol, &live.evidence);
                // Frames are scrubbed one by one; the assembled text is scrubbed
                // again for a credential split across streamed fragments.
                let text = live.transport.scrub_text(std::mem::take(&mut live.text));
                return Ok(RuntimeResponse {
                    status: RuntimeStatus::Completed,
                    runtime_ref: Some(reference),
                    message: Some(aidex_providers::ChatMessage::assistant(text)),
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
        abandon(&mut live);
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
        let open = (self.network)();
        let live = if r.operation == RuntimeOperation::Approval {
            let mut guard = self.live.lock().unwrap_or_else(PoisonError::into_inner);
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
            let waiting = Self::waiting(live, open);
            let mut live = guard.remove(&r.run_id).unwrap();
            // Release the live map first: stopping a launch waits for its
            // native tree to be reaped.
            drop(guard);
            if !waiting {
                abandon(&mut live);
                return Err(if open {
                    error("approval_expired", "Native approval process has ended")
                } else {
                    error(
                        "offline_refused",
                        "External runtimes were paused: offline mode, a locked vault or an emergency stop",
                    )
                });
            }
            // The decision continues under the credential the request was made with.
            if let Err(e) =
                live.evidence
                    .credential_binding
                    .verify(&self.config.auth, &self.state, &self.auth)
            {
                abandon(&mut live);
                return Err(e);
            }
            let pending = live.pending.take().unwrap();
            if let Err(e) = live.transport.send(json!({"jsonrpc":"2.0","id":pending.wire,"result":if decision.approved{pending.allow}else{pending.deny}})) {
                abandon(&mut live);
                return Err(e);
            }
            live
        } else {
            // A launch still parked for this run is released when it no longer
            // waits (its tree ended, its cancellation stopped or the gate
            // closed), and an explicit `Resume` of the run supersedes one that
            // still waits: it continues from the paused reference instead. A
            // `Start` never replaces a waiting launch.
            let resume = r.operation == RuntimeOperation::Resume;
            if let Some(mut stale) = take_parked(&self.live, &r.run_id, |live| {
                resume || !Self::waiting(live, open)
            }) {
                abandon(&mut stale);
            }
            if self
                .live
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
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
    fn release(&self, run_id: &str) {
        self.release_run(run_id);
    }
    fn forget(&self, runtime_ref: &Value) {
        self.forget_snapshot(runtime_ref);
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
        let config = RuntimeConfig {
            id: "probe".into(),
            driver: Driver::Codex,
            executable,
            argv: vec![],
            profile: None,
            enabled: true,
            auth: RuntimeAuth::None,
        };
        // A launch checks the runtime is still configured as it was made.
        let configs = Arc::new(Configs::new(state.clone()));
        configs.write(std::slice::from_ref(&config)).unwrap();
        let runtime = ExternalRuntime::new(
            config,
            state,
            configs,
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
