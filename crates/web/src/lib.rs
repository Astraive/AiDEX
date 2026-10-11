//! Vault-authenticated native search and public-page fetch through sealed tools.
mod model;
mod network;
mod service;
mod tools;
pub use model::*;
pub use network::{
    HttpReply, HttpRequest, HttpTransport, PublicHttpsTransport, public_address, validate_url,
};
pub use service::WebService;
