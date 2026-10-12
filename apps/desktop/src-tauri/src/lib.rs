//! AiDEX desktop backend: the bridge between the webview and `aidexd`.

mod app;
mod bridge;
mod commands;
mod error;
mod lifecycle;
mod navigation;
mod state;

pub use app::run;
