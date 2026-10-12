//! Approvals (spec §14): bound, expiring, single-use approval requests with
//! human-readable cards, durable compare-and-set state, and session grants. This is
//! the implementation of the reserved `approvals` domain.

pub(crate) mod card;
mod model;
mod store;

pub use model::{
    ApprovalCard, ApprovalFilter, ApprovalRecord, ApprovalResolution, ApprovalState,
    ApprovalStatus, CardDiff, CardDiffFile, GrantInfo, Requester, SecretNotice,
};
pub(crate) use store::NewApproval;
pub use store::{APPROVALS_FILE, ApprovalStore};
