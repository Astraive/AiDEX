//! Native Sign in with ChatGPT; OAuth secrets stay behind vault handles.
mod browser;
mod oauth;
mod service;
mod storage;
pub use browser::{open_in_browser, validate_browser_url};
pub use service::{AuthError, AuthService, AuthState, AuthStatus, StartResult};
