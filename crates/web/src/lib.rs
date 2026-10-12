//! AiDEX web search and fetch.
//!
//! - [`WebSettings`]: `private_config/web.toml` (closed schema): the search
//!   provider, a SearXNG endpoint, the vault secret *name* of the provider key,
//!   result count, and the fetch switch.
//! - [`WebClient`]: searches the configured provider (Brave, Tavily, Exa,
//!   SearXNG, or DuckDuckGo's HTML page) and fetches pages as readable text.
//!   It holds no policy: the embedder passes a credential resolver (vault
//!   handles for the consumer `process:web`) and an egress check (offline
//!   switch, network policy) that runs before every connection, redirects
//!   included.
//! - Fetches accept `https` (plain `http` only to loopback), refuse embedded
//!   credentials, follow at most five redirects with every hop re-validated,
//!   resolve DNS before connecting and refuse private, loopback, and
//!   link-local addresses unless the URL names them explicitly and the client
//!   allows it, and pin the connection to the checked addresses.
//! - [`html`]: a dependency-free tokenizer and readable-text extraction.
//!
//! Page text and search results are untrusted data; callers wrap them before
//! they reach a model.

mod catalog;
mod client;
mod error;
mod fetch;
pub mod html;
mod http;
mod open;
mod search;
mod settings;
mod target;

pub use catalog::{ProviderInfo, SearchProvider, catalog};
pub use client::{
    CredentialRequest, CredentialResolver, EgressCheck, EgressPurpose, EgressRequest, FetchedPage,
    MAX_RAW_BYTES, MAX_REDIRECTS, MAX_TEXT_BYTES, SearchResult, Secret, WebClient, WebOptions,
};
pub use error::{CredentialProblem, WebError};
pub use open::validate_open_url;
pub use search::MAX_QUERY_CHARS;
pub use settings::{
    DEFAULT_MAX_RESULTS, MAX_RESULTS, SETTINGS_FILE, SETTINGS_VERSION, WebSettings,
    WebSettingsPatch, validate_endpoint, validate_secret_name,
};
pub use target::{AddressClass, DnsResolver, LocalTargets, MAX_URL_BYTES, classify_ip, system_dns};

/// The vault consumer kind and tool id under which provider keys are stored
/// (`process:web`).
pub const CREDENTIAL_TOOL_ID: &str = "web";
