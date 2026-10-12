//! Explicit external process adapters; vendor tool loops remain vendor owned.
mod catalog;
mod manager;
mod protocol;
mod runtime;
mod state;
mod transport;
pub use aidex_native_sessions::RuntimeError;
pub use catalog::*;
pub use manager::RuntimeManager;
pub use runtime::ExternalRuntime;
