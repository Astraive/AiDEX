//! Who asked for an action (origins) and who decided an approval (principals).

use std::{
    fmt,
    sync::atomic::{AtomicI64, Ordering},
};

use aidex_core::{EntityId, Timestamp};
use serde::{Deserialize, Serialize};

/// Where a tool invocation came from (spec §81 provenance). Every origin except
/// [`Origin::User`] is untrusted agent-side input: such invocations can never change
/// policy, disable the broker, or modify the vault, whatever the active profile says.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Origin {
    /// The native AiDEX agent (the invocation's `agent_id` names it).
    Agent,
    /// The local user acting directly through the CLI or desktop.
    User,
    /// A paired remote device (spec §53). Adds an implied `remote.action` intent.
    Remote { device_id: EntityId },
    /// A portable skill running inside an agent session.
    Skill { skill_id: EntityId },
    /// An external agent adapter (OMP, Codex, Claude, ...).
    Adapter { adapter_id: EntityId },
}

impl Origin {
    /// Whether the request comes from the local user rather than an agent, skill,
    /// adapter, or remote device.
    pub fn is_local_user(&self) -> bool {
        matches!(self, Self::User)
    }

    /// Short label for approval cards (`agent`, `user`, `remote`, `skill`, `adapter`).
    pub fn label(&self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::User => "user",
            Self::Remote { .. } => "remote",
            Self::Skill { .. } => "skill",
            Self::Adapter { .. } => "adapter",
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent => f.write_str("agent"),
            Self::User => f.write_str("local user"),
            Self::Remote { device_id } => write!(f, "remote device {device_id}"),
            Self::Skill { skill_id } => write!(f, "skill {skill_id}"),
            Self::Adapter { adapter_id } => write!(f, "adapter {adapter_id}"),
        }
    }
}

/// Who resolved or revoked an approval.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Principal {
    /// The user at this machine (authenticated local IPC).
    LocalUser,
    /// A paired remote device (authorization of remote commands is issue #29).
    RemoteDevice { device_id: EntityId },
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LocalUser => f.write_str("local user"),
            Self::RemoteDevice { device_id } => write!(f, "remote device {device_id}"),
        }
    }
}

/// The broker's source of wall-clock time (approval expiry, grant lifetimes).
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Timestamp::now()
    }
}

/// A clock that only moves when told to (tests, deterministic simulations).
#[derive(Debug)]
pub struct ManualClock {
    millis: AtomicI64,
}

impl ManualClock {
    pub fn new(start: &Timestamp) -> Self {
        Self {
            millis: AtomicI64::new(start.unix_millis()),
        }
    }

    /// Move the clock forward by `millis` milliseconds.
    pub fn advance_millis(&self, millis: i64) {
        self.millis.fetch_add(millis, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        let millis = self.millis.load(Ordering::SeqCst);
        Timestamp::from_unix_millis(millis).unwrap_or_else(|_| Timestamp::now())
    }
}
