//! JSON-RPC adapters, one module per domain.
//!
//! Each `methods/<domain>.rs` exposes
//! `pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError>`,
//! registers `<domain>.<verb>` methods (and `<domain>.subscribe` streams that
//! emit `<domain>.event` notifications), and keeps domain logic in its crate.
//! Add one `pub mod` line and one `register` line below per domain.

pub mod auth;
pub mod context;
pub mod daemon;
pub mod events;
pub mod extensions;
pub mod inference;
pub mod memory;
pub mod models;
pub mod native;
pub mod native_history;
pub mod native_view;
pub mod runtime;
pub mod runtime_profiles;
pub mod sessions;
pub mod settings;
pub mod update;
pub mod vault;
pub mod web;
pub mod workspace;
pub mod workspace_live;

use crate::{
    errors::RpcError,
    router::{Router, RouterError},
    services::Services,
};
use aidex_core::ServiceErrorCode;
use aidex_events::{EventKind, EventScope};
use serde_json::json;
use std::sync::Arc;

/// Record the audit event of a change that has already committed.
///
/// The change is not undone if the append fails, so the failure is reported
/// explicitly instead of being swallowed: an `internal` error whose
/// `data` is `{"reason": "event_not_recorded", "committed": true}`.
pub(crate) fn record_event(
    services: &Services,
    scope: EventScope,
    event: EventKind,
) -> Result<(), RpcError> {
    let name = event.name();
    services
        .events()
        .append(scope, event)
        .map(|_| ())
        .map_err(|error| {
            RpcError::service(
                ServiceErrorCode::Internal,
                format!(
                    "the change was applied, but its `{name}` event could not be recorded: {error}"
                ),
            )
            .with_data(json!({"reason": "event_not_recorded", "committed": true}))
        })
}

/// Register every built-in and domain method.
pub(crate) fn register_all(
    router: &mut Router,
    services: &Arc<Services>,
) -> Result<(), RouterError> {
    context::register(router, services)?;
    auth::register(router, services)?;
    extensions::register(router, services)?;
    web::register(router, services)?;
    runtime::register(router, services)?;
    runtime_profiles::register(router, services)?;
    inference::register(router, services)?;
    native::register(router, services)?;
    settings::register(router, services)?;
    daemon::register(router, services)?;
    events::register(router, services)?;
    memory::register(router, services)?;
    models::register(router, services)?;
    sessions::register(router, services)?;
    update::register(router, services)?;
    vault::register(router, services)?;
    workspace::register(router, services)?;
    Ok(())
}
