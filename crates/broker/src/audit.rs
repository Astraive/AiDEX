//! Audit events the broker emits, and conversions to the event catalog's enums.
//!
//! Every free-text field is redacted and bounded again by the event sink; the broker
//! never puts secret values, raw command lines, or tool input into an event.

use std::sync::Arc;

use aidex_events::{EventKind, EventScope, EventSink};

use crate::BrokerError;

/// Where broker events go.
#[derive(Clone)]
pub(crate) struct Audit {
    sink: Arc<dyn EventSink>,
}

impl Audit {
    pub fn new(sink: Arc<dyn EventSink>) -> Self {
        Self { sink }
    }

    pub fn sink(&self) -> &Arc<dyn EventSink> {
        &self.sink
    }

    /// Record an event that must exist before anything happens (fail closed).
    pub fn before(&self, scope: &EventScope, event: EventKind) -> Result<(), BrokerError> {
        let name = event.name();
        self.sink
            .emit(scope.clone(), event)
            .map(|_| ())
            .map_err(|error| BrokerError::Audit {
                committed: false,
                message: format!("{name}: {error}"),
            })
    }

    /// Record an event describing a change that already happened.
    pub fn after(&self, scope: &EventScope, event: EventKind) -> Result<(), BrokerError> {
        let name = event.name();
        self.sink
            .emit(scope.clone(), event)
            .map(|_| ())
            .map_err(|error| BrokerError::Audit {
                committed: true,
                message: format!("{name}: {error}"),
            })
    }

    /// Record a best-effort event whose loss cannot change the outcome.
    pub fn note(&self, scope: &EventScope, event: EventKind) {
        let _ = self.sink.emit(scope.clone(), event);
    }
}

pub(crate) fn events_level(level: aidex_sandbox::SandboxLevel) -> aidex_events::SandboxLevel {
    match level {
        aidex_sandbox::SandboxLevel::Direct => aidex_events::SandboxLevel::Direct,
        aidex_sandbox::SandboxLevel::Brokered => aidex_events::SandboxLevel::Brokered,
        aidex_sandbox::SandboxLevel::Restricted => aidex_events::SandboxLevel::Restricted,
        aidex_sandbox::SandboxLevel::Ephemeral => aidex_events::SandboxLevel::Ephemeral,
    }
}

pub(crate) fn sandbox_level(level: aidex_policy::SandboxLevel) -> aidex_sandbox::SandboxLevel {
    match level {
        aidex_policy::SandboxLevel::Direct => aidex_sandbox::SandboxLevel::Direct,
        aidex_policy::SandboxLevel::Brokered => aidex_sandbox::SandboxLevel::Brokered,
        aidex_policy::SandboxLevel::Restricted => aidex_sandbox::SandboxLevel::Restricted,
        aidex_policy::SandboxLevel::Ephemeral => aidex_sandbox::SandboxLevel::Ephemeral,
    }
}

pub(crate) fn events_verdict(verdict: &aidex_policy::Verdict) -> aidex_events::Verdict {
    match verdict {
        aidex_policy::Verdict::Allow => aidex_events::Verdict::Allow,
        aidex_policy::Verdict::Deny => aidex_events::Verdict::Deny,
        aidex_policy::Verdict::Sandbox { .. } => aidex_events::Verdict::Sandbox,
        aidex_policy::Verdict::RequireApproval { .. } => aidex_events::Verdict::RequireApproval,
        aidex_policy::Verdict::RateLimit { .. } => aidex_events::Verdict::RateLimit,
    }
}

pub(crate) fn events_scope(scope: aidex_policy::ApprovalScope) -> aidex_events::ApprovalScope {
    match scope {
        aidex_policy::ApprovalScope::Once => aidex_events::ApprovalScope::Once,
        aidex_policy::ApprovalScope::Session => aidex_events::ApprovalScope::Session,
    }
}
