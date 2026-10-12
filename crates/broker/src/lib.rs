//! The AiDEX execution broker: the security boundary between reasoning and action
//! (spec §4.4, §13-§15, §48, §81, §92, §94).
//!
//! Agents (and the harness, skills, adapters, remote devices, and the local user)
//! submit [`ToolInvocation`]s. The [`Broker`] plans each call with `aidex-tools` (no
//! side effects), turns the plan's computed facts into `aidex-policy` intents,
//! evaluates them, and then executes under a permit bound to the plan's fingerprint,
//! asks for a bound [`approvals::ApprovalCard`] decision, or denies. Processes run
//! only through [`SandboxLauncher`] (over `aidex-sandbox`, failing closed when the
//! required level is unavailable), secrets reach only the child that needs them
//! through short-lived vault handles, and every decision and outcome is an audit
//! event. [`Broker::emergency_stop`] freezes everything.

pub mod approvals;
mod audit;
mod broker;
mod digest;
mod egress;
mod emergency;
mod error;
mod git_remote;
mod intent;
mod launcher;
mod origin;
mod outcome;
mod rate;
mod secrets;
mod settings;
mod workspaces;

pub use broker::{
    Broker, BrokerBuilder, BrokerConfig, DEFAULT_SESSION_GRANT_TTL, HardRuleInfo, PolicyStatus,
    SessionCancelReport,
};
pub use egress::ModelEgress;
pub use emergency::EmergencyReport;
pub use error::{ApprovalRefusal, BrokerError};
pub use intent::{CombinedDecision, IntentDecision};
pub use launcher::SandboxLauncher;
pub use origin::{Clock, ManualClock, Origin, Principal, SystemClock};
pub use outcome::{BrokerOutcome, Explanation, PermitInfo, ToolFailure, ToolInvocation};
pub use settings::{PolicyChange, PolicySettings, SETTINGS_FILE, SETTINGS_VERSION};
pub use workspaces::{ResolvedWorkspace, StaticWorkspaces, WorkspaceResolver};
