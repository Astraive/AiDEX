//! Explicit external process adapters; vendor tool loops remain vendor owned.
mod binding;
mod catalog;
mod diagnostics;
mod install;
mod manager;
mod profiles;
mod protocol;
mod runtime;
mod snapshot;
mod state;
mod transport;
pub use aidex_native_sessions::RuntimeError;
pub use catalog::*;
pub use manager::RuntimeManager;
pub use profiles::{RuntimeAccountStatus, RuntimeLoginStatus};
pub use runtime::ExternalRuntime;
pub use state::RUNTIME_STATE_KEY_SECRET;
