//! `aidex-events`: the typed, versioned AiDEX event contract.
//!
//! - [`EventKind`]: the closed catalog (`{"type": "<dotted.name>", "data": {...}}`).
//! - [`EventEnvelope`]: version, global [`Cursor`], [`EventScope`], time, and event.
//! - [`EventLog`]: durable SQLite log with one strictly increasing cursor per drive,
//!   retention without silent gaps, filtered replay, and bounded subscriptions.
//! - [`EventSink`]: the emit interface for domain crates ([`MemoryEventSink`] and
//!   [`NullEventSink`] for tests).
//! - [`redact_text`]: credential redaction applied to every text field on append.

mod catalog;
mod envelope;
mod error;
mod filter;
mod log;
pub mod redact;
mod sanitize;
mod sink;
pub mod subscription;

pub use catalog::{
    ApprovalScope, DaemonStopReason, EVENT_NAMES, EventKind, ExtensionKind, FileChange,
    LegacyApprovalDecision, LegacyRecord, MessageRole, Outcome, ProcessStream, RiskLevel,
    SandboxLevel, Verdict, WorkspaceKind,
};
pub use envelope::{Cursor, ENVELOPE_VERSION, EventEnvelope, EventScope};
pub use error::EventError;
pub use filter::{EventFilter, MAX_FILTER_KINDS, kind_matches};
pub use log::{
    DEFAULT_MAX_EVENTS, EventLog, LegacyImport, MAX_PAGE_SIZE, ReplayPage, RetentionPolicy,
};
pub use redact::{redact_text, sanitize_text};
pub use sanitize::MAX_EVENT_BYTES;
pub use sink::{EventSink, MemoryEventSink, NullEventSink};
pub use subscription::{Subscription, SubscriptionItem};
