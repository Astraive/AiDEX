//! First-class web outcomes.
//!
//! Messages name hosts and providers but never a full URL (a query string can
//! carry tokens), a request header, or a credential value.

use std::fmt;

use aidex_core::ServiceErrorCode;

/// Why a credential could not be released for a search provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialProblem {
    /// The vault is locked.
    Locked,
    /// No secret with the configured name exists.
    NotFound,
    /// The secret exists but its policy does not name the web consumer, or a
    /// policy rule denied its use.
    Denied,
    /// The secret is not usable as a header value (for example not UTF-8).
    Invalid,
    /// The vault or broker is unavailable.
    Unavailable,
}

impl CredentialProblem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Locked => "vault_locked",
            Self::NotFound => "secret_not_found",
            Self::Denied => "secret_denied",
            Self::Invalid => "secret_invalid",
            Self::Unavailable => "credential_unavailable",
        }
    }
}

/// Every way a search or fetch can fail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebError {
    /// The request itself is malformed (empty query, bad parameters).
    InvalidRequest(String),
    /// The URL is not acceptable: not absolute, not http(s), plain `http` to a
    /// non-loopback host, embedded credentials, or too long.
    InvalidUrl(String),
    /// The destination is refused before any connection: a private, loopback,
    /// link-local, or reserved address that is not explicitly local.
    TargetRefused(String),
    /// The offline switch is on; no connection was attempted.
    Offline,
    /// The egress policy refused the destination; no connection was attempted.
    EgressDenied(String),
    /// The settings file or a settings change is invalid.
    InvalidSettings(String),
    /// The configured provider is missing something it needs (an API key name
    /// or an endpoint).
    NotConfigured(String),
    /// The provider's credential could not be released.
    Credential {
        problem: CredentialProblem,
        message: String,
    },
    /// The provider rejected the credential (HTTP 401/403).
    Unauthorized(String),
    /// The provider is rate limiting (HTTP 429) or asked for a bot check.
    RateLimited(String),
    /// The server answered with an unexpected HTTP status.
    Http { status: u16, message: String },
    /// The response content type is not one this client reads.
    UnsupportedContentType(String),
    /// More than the allowed number of redirects.
    TooManyRedirects(usize),
    /// The response could not be understood.
    Protocol(String),
    /// A connect, response, or body deadline passed.
    Timeout,
    /// The caller cancelled the request.
    Cancelled,
    /// The host could not be resolved or reached.
    Unavailable(String),
    /// Reading or writing local state failed.
    Io(String),
}

impl WebError {
    /// The domain category of this error.
    pub const fn code(&self) -> ServiceErrorCode {
        match self {
            Self::InvalidRequest(_)
            | Self::InvalidUrl(_)
            | Self::InvalidSettings(_)
            | Self::NotConfigured(_)
            | Self::UnsupportedContentType(_) => ServiceErrorCode::InvalidArgument,
            Self::TargetRefused(_)
            | Self::Offline
            | Self::EgressDenied(_)
            | Self::Unauthorized(_) => ServiceErrorCode::PermissionDenied,
            Self::Credential { problem, .. } => match problem {
                CredentialProblem::NotFound => ServiceErrorCode::NotFound,
                CredentialProblem::Invalid => ServiceErrorCode::InvalidArgument,
                CredentialProblem::Unavailable => ServiceErrorCode::Unavailable,
                CredentialProblem::Locked | CredentialProblem::Denied => {
                    ServiceErrorCode::PermissionDenied
                }
            },
            Self::RateLimited(_)
            | Self::Http { .. }
            | Self::TooManyRedirects(_)
            | Self::Protocol(_)
            | Self::Timeout
            | Self::Cancelled
            | Self::Unavailable(_) => ServiceErrorCode::Unavailable,
            Self::Io(_) => ServiceErrorCode::Internal,
        }
    }

    /// A stable machine-readable reason (`data.reason` on the wire).
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidUrl(_) => "invalid_url",
            Self::TargetRefused(_) => "target_refused",
            Self::Offline => "offline_refused",
            Self::EgressDenied(_) => "policy_denied",
            Self::InvalidSettings(_) => "invalid_settings",
            Self::NotConfigured(_) => "not_configured",
            Self::Credential { problem, .. } => problem.as_str(),
            Self::Unauthorized(_) => "unauthorized",
            Self::RateLimited(_) => "rate_limited",
            Self::Http { .. } => "http_error",
            Self::UnsupportedContentType(_) => "unsupported_content_type",
            Self::TooManyRedirects(_) => "too_many_redirects",
            Self::Protocol(_) => "protocol",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Unavailable(_) => "unavailable",
            Self::Io(_) => "io",
        }
    }

    pub(crate) fn credential(problem: CredentialProblem, message: impl Into<String>) -> Self {
        Self::Credential {
            problem,
            message: message.into(),
        }
    }
}

impl fmt::Display for WebError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest(message) => write!(f, "invalid web request: {message}"),
            Self::InvalidUrl(message) => write!(f, "URL refused: {message}"),
            Self::TargetRefused(message) => write!(f, "destination refused: {message}"),
            Self::Offline => f.write_str("offline mode is on, so web requests are refused"),
            Self::EgressDenied(message) => {
                write!(f, "network policy refused the request: {message}")
            }
            Self::InvalidSettings(message) => write!(f, "invalid web settings: {message}"),
            Self::NotConfigured(message) => write!(f, "web search is not configured: {message}"),
            Self::Credential { message, .. } => {
                write!(f, "search provider key unavailable: {message}")
            }
            Self::Unauthorized(message) => {
                write!(f, "the provider rejected the request: {message}")
            }
            Self::RateLimited(message) => write!(f, "rate limited: {message}"),
            Self::Http { status, message } => write!(f, "HTTP {status}: {message}"),
            Self::UnsupportedContentType(kind) => {
                write!(f, "content type `{kind}` is not readable as text")
            }
            Self::TooManyRedirects(count) => write!(f, "more than {count} redirects"),
            Self::Protocol(message) => write!(f, "unexpected response: {message}"),
            Self::Timeout => f.write_str("the web request timed out"),
            Self::Cancelled => f.write_str("the web request was cancelled"),
            Self::Unavailable(message) => write!(f, "web request failed: {message}"),
            Self::Io(message) => write!(f, "web settings I/O failed: {message}"),
        }
    }
}

impl std::error::Error for WebError {}
