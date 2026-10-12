//! The execution broker: composition, configuration, and policy state.
//!
//! Behaviour lives in `submit.rs` (submit and continue), `prepare.rs` (planning,
//! policy evaluation, dry runs), `execute.rs` (permits, sandbox checks, denials),
//! `approvals.rs` (resolve/revoke/cancel/expire), and `control.rs` (policy changes,
//! session cancellation, emergency stop).

mod approvals;
mod control;
mod execute;
mod prepare;
mod submit;

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard},
    time::Duration,
};

use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use aidex_events::{EventKind, EventScope, EventSink};
use aidex_policy::{
    Decision, HARD_RULES, HardVerdict, Intent, PolicySet, RuleOrigin, RuleRef, SandboxLevel,
    Verdict,
};
use aidex_sandbox::Sandbox;
use aidex_tools::{CancelToken, ToolDescriptor, ToolLimits, ToolRegistry};
use aidex_vault::VaultService;
use serde::Serialize;

pub use control::SessionCancelReport;

use crate::{
    BrokerError, Clock, PolicySettings, SystemClock, ToolInvocation,
    approvals::{APPROVALS_FILE, ApprovalStore},
    audit::Audit,
    emergency::EmergencyState,
    launcher::SandboxLauncher,
    rate::RateLimiter,
    secrets::Secrets,
    settings::{self, SETTINGS_FILE},
    workspaces::WorkspaceResolver,
};

/// Default lifetime of a session grant created by `approve_session`.
pub const DEFAULT_SESSION_GRANT_TTL: Duration = Duration::from_secs(8 * 60 * 60);

/// Where the broker keeps its state and what it treats specially.
#[derive(Clone, Debug)]
pub struct BrokerConfig {
    /// SQLite approval database.
    pub approvals_path: PathBuf,
    /// Persisted policy settings; `None` keeps them in memory only.
    pub settings_path: Option<PathBuf>,
    /// Policy documents (`DrivePaths::policies()`); `None` loads none.
    pub policies_dir: Option<PathBuf>,
    /// Root for per-launch sandbox scratch directories (must exist).
    pub scratch_dir: PathBuf,
    /// Filesystem targets beneath these are `protected` (hard-denied for `fs.*`).
    pub protected_paths: Vec<PathBuf>,
    /// Writes beneath these imply `policy.change`.
    pub policy_paths: Vec<PathBuf>,
    /// Settings used when no settings file exists yet.
    pub default_settings: PolicySettings,
    pub tool_limits: ToolLimits,
    pub session_grant_ttl: Duration,
}

impl BrokerConfig {
    /// Everything inside `state_dir` (embedders and tests): approvals and scratch
    /// there, settings in memory, no policy documents.
    pub fn in_dir(state_dir: &Path) -> Self {
        Self {
            approvals_path: state_dir.join(APPROVALS_FILE),
            settings_path: None,
            policies_dir: None,
            scratch_dir: state_dir.to_path_buf(),
            protected_paths: Vec::new(),
            policy_paths: Vec::new(),
            default_settings: PolicySettings::default(),
            tool_limits: ToolLimits::default(),
            session_grant_ttl: DEFAULT_SESSION_GRANT_TTL,
        }
    }

    /// The drive layout: approvals in `private_sessions/`, settings in
    /// `private_config/policy.toml`, documents in `policies/`, scratch in
    /// `tmp/broker/`. The vault, identity, sessions, config, memory, index, and daemon
    /// runtime directories are protected; `policies/` writes are policy changes.
    pub fn for_drive(paths: &DrivePaths) -> Result<Self, BrokerError> {
        let ensure = |dir: PathBuf| {
            paths
                .ensure_dir(&dir)
                .map_err(|error| BrokerError::Settings(error.to_string()))
        };
        let sessions = ensure(paths.sessions())?;
        let config = ensure(paths.config())?;
        let policies = ensure(paths.policies())?;
        let scratch = ensure(paths.tmp().join("broker"))?;
        Ok(Self {
            approvals_path: sessions.join(APPROVALS_FILE),
            settings_path: Some(config.join(SETTINGS_FILE)),
            policies_dir: Some(policies.clone()),
            scratch_dir: scratch,
            protected_paths: vec![
                paths.vault(),
                paths.identity(),
                paths.sessions(),
                paths.config(),
                paths.memory(),
                paths.index(),
                paths.runtime_state(),
            ],
            policy_paths: vec![policies],
            default_settings: PolicySettings::default(),
            tool_limits: ToolLimits::default(),
            session_grant_ttl: DEFAULT_SESSION_GRANT_TTL,
        })
    }
}

/// The loaded policy documents and active settings.
struct PolicyState {
    documents: Result<Arc<PolicySet>, String>,
    settings: PolicySettings,
}

/// An invocation waiting for a human decision (memory only: raw tool input is never
/// persisted, so approvals pending across a restart are cancelled).
struct PendingCall {
    invocation: ToolInvocation,
    call_id: EntityId,
}

/// A call currently executing.
struct ActiveCall {
    session_id: EntityId,
    cancel: CancelToken,
}

/// Builds a [`Broker`].
pub struct BrokerBuilder {
    config: BrokerConfig,
    workspaces: Arc<dyn WorkspaceResolver>,
    events: Arc<dyn EventSink>,
    registry: Option<ToolRegistry>,
    sandbox: Option<Sandbox>,
    vault: Option<Arc<VaultService>>,
    clock: Arc<dyn Clock>,
}

impl BrokerBuilder {
    /// Tools to mediate (default: every built-in tool).
    pub fn registry(mut self, registry: ToolRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// The sandbox processes launch through (default: `Sandbox::locate()`).
    pub fn sandbox(mut self, sandbox: Sandbox) -> Self {
        self.sandbox = Some(sandbox);
        self
    }

    /// The vault credentials come from (default: none; credential-bearing calls run
    /// without credentials).
    pub fn vault(mut self, vault: Arc<VaultService>) -> Self {
        self.vault = Some(vault);
        self
    }

    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Open the approval store, load settings and policy documents, and cancel
    /// approvals left pending by a previous run (their invocations were in memory).
    pub fn open(self) -> Result<Broker, BrokerError> {
        let store = ApprovalStore::open(&self.config.approvals_path)?;
        let settings = match &self.config.settings_path {
            Some(path) => settings::load(path)?.unwrap_or(self.config.default_settings),
            None => self.config.default_settings,
        };
        let documents = load_documents(self.config.policies_dir.as_deref());
        let secrets = Secrets::new(self.vault);
        let audit = Audit::new(self.events);
        let launcher = Arc::new(SandboxLauncher::new(
            self.sandbox.unwrap_or_else(Sandbox::locate),
            self.config.scratch_dir.clone(),
            Arc::clone(audit.sink()),
            secrets.clone(),
        ));
        let broker = Broker {
            registry: self
                .registry
                .unwrap_or_else(ToolRegistry::with_builtin_tools),
            workspaces: self.workspaces,
            audit,
            store,
            launcher,
            secrets,
            clock: self.clock,
            policy: RwLock::new(PolicyState {
                documents,
                settings,
            }),
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new(HashMap::new()),
            rate: RateLimiter::default(),
            emergency: EmergencyState::default(),
            config: self.config,
        };
        let now = broker.clock.now();
        for record in broker.store.cancel_pending(
            None,
            &now,
            "the broker restarted before a decision was made",
        )? {
            broker.audit.note(
                &EventScope::session(record.workspace_id.clone(), record.session_id.clone()),
                EventKind::ApprovalCancelled {
                    approval_id: record.approval_id.clone(),
                },
            );
        }
        Ok(broker)
    }
}

fn load_documents(dir: Option<&Path>) -> Result<Arc<PolicySet>, String> {
    match dir {
        Some(dir) => PolicySet::load(dir)
            .map(Arc::new)
            .map_err(|error| error.to_string()),
        None => Ok(Arc::new(PolicySet::new())),
    }
}

/// A hard rule as reported by `policy.show`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HardRuleInfo {
    pub id: &'static str,
    /// `deny` or `require_approval`.
    pub verdict: &'static str,
    pub description: &'static str,
}

/// The broker's policy configuration, for `policy.show`.
#[derive(Clone, Debug, Serialize)]
pub struct PolicyStatus {
    pub settings: PolicySettings,
    /// The profile after host-trust tightening.
    pub effective_profile: aidex_policy::Profile,
    pub effective_privacy: aidex_policy::PrivacyMode,
    pub effective_network_mode: aidex_policy::NetworkMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policies_dir: Option<PathBuf>,
    /// Rules loaded from policy documents (0 when loading failed).
    pub document_rules: usize,
    /// Why the policy documents failed to load; every call is denied until fixed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_error: Option<String>,
    pub hard_rules: Vec<HardRuleInfo>,
    /// The reason of an emergency stop in effect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emergency_stop: Option<String>,
}

/// Mediates every tool execution: plans the call, evaluates policy on the plan's
/// computed facts, asks for bound approvals, launches processes only through the
/// sandbox, injects secrets only into the processes that need them, and records
/// every decision and outcome as an audit event. It owns its [`ToolRegistry`];
/// callers hold the broker, never the registry.
pub struct Broker {
    config: BrokerConfig,
    registry: ToolRegistry,
    workspaces: Arc<dyn WorkspaceResolver>,
    audit: Audit,
    store: ApprovalStore,
    launcher: Arc<SandboxLauncher>,
    secrets: Secrets,
    clock: Arc<dyn Clock>,
    policy: RwLock<PolicyState>,
    pending: Mutex<HashMap<EntityId, PendingCall>>,
    active: Mutex<HashMap<EntityId, ActiveCall>>,
    rate: RateLimiter,
    emergency: EmergencyState,
}

impl fmt::Debug for Broker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Broker")
            .field("approvals", &self.store.path())
            .field("tools", &self.registry.descriptors().len())
            .field("emergency_stop", &self.emergency.reason())
            .finish_non_exhaustive()
    }
}

impl Broker {
    pub fn builder(
        config: BrokerConfig,
        workspaces: Arc<dyn WorkspaceResolver>,
        events: Arc<dyn EventSink>,
    ) -> BrokerBuilder {
        BrokerBuilder {
            config,
            workspaces,
            events,
            registry: None,
            sandbox: None,
            vault: None,
            clock: Arc::new(SystemClock),
        }
    }

    pub fn config(&self) -> &BrokerConfig {
        &self.config
    }

    /// Read access to the approval database.
    pub fn approvals(&self) -> &ApprovalStore {
        &self.store
    }

    /// Every registered tool descriptor (with risk metadata), ordered by id.
    pub fn tools(&self) -> Vec<&ToolDescriptor> {
        self.registry.descriptors()
    }

    /// The registry's model-facing tool specs.
    pub fn tool_specs(&self) -> Vec<aidex_tools::ToolSpec> {
        self.registry.specs()
    }

    /// The sandbox (and its process registry) tool processes run in.
    pub fn sandbox(&self) -> &Sandbox {
        self.launcher.sandbox()
    }

    /// The broker's process launcher. It refuses every launch whose plan the broker
    /// did not permit, so handing it out grants nothing.
    pub fn launcher(&self) -> Arc<dyn aidex_tools::ProcessLauncher> {
        self.launcher.clone()
    }

    pub fn settings(&self) -> PolicySettings {
        self.policy_state().settings
    }

    /// Whether an emergency stop is in effect, and why.
    pub fn emergency_reason(&self) -> Option<String> {
        self.emergency.reason()
    }

    pub fn policy_status(&self) -> PolicyStatus {
        let state = self.policy_state();
        let context = state.settings.context();
        let (document_rules, document_error) = match &state.documents {
            Ok(set) => (set.rule_count(), None),
            Err(error) => (0, Some(error.clone())),
        };
        PolicyStatus {
            settings: state.settings,
            effective_profile: context.effective_profile(),
            effective_privacy: context.effective_privacy(),
            effective_network_mode: context.effective_network_mode(),
            policies_dir: self.config.policies_dir.clone(),
            document_rules,
            document_error,
            hard_rules: HARD_RULES
                .iter()
                .map(|rule| HardRuleInfo {
                    id: rule.id,
                    verdict: match rule.verdict {
                        HardVerdict::Deny => "deny",
                        HardVerdict::RequireApproval => "require_approval",
                    },
                    description: rule.description,
                })
                .collect(),
            emergency_stop: self.emergency.reason(),
        }
    }

    /// Evaluate one standalone intent (no tool plan) under the current settings and
    /// documents; a document load failure denies.
    pub(crate) fn evaluate_intent(&self, intent: &Intent) -> Decision {
        let state = self.policy_state();
        match &state.documents {
            Ok(set) => aidex_policy::evaluate(set, &state.settings.context(), intent),
            Err(error) => Decision {
                verdict: Verdict::Deny,
                reason: format!(
                    "policy documents failed to load, so every call is denied: {error}"
                ),
                rule: RuleRef {
                    origin: RuleOrigin::Default,
                    scope: "broker".into(),
                    id: "broker.policy-documents".into(),
                    source: None,
                },
                effect_summary: intent.summary.clone(),
                sandbox: SandboxLevel::Brokered,
                rate_limit: None,
            },
        }
    }

    pub(crate) fn audit(&self) -> &Audit {
        &self.audit
    }

    pub(crate) fn emergency(&self) -> &EmergencyState {
        &self.emergency
    }

    pub(crate) fn secrets(&self) -> &Secrets {
        &self.secrets
    }

    pub(crate) fn rate(&self) -> &RateLimiter {
        &self.rate
    }

    fn policy_state(&self) -> RwLockReadGuard<'_, PolicyState> {
        self.policy.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn pending(&self) -> MutexGuard<'_, HashMap<EntityId, PendingCall>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn active(&self) -> MutexGuard<'_, HashMap<EntityId, ActiveCall>> {
        self.active.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
