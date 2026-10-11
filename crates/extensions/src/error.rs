use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    Invalid(String),
    Forbidden(String),
    Conflict(String),
    Unavailable(String),
    Protocol(String),
    TooLarge,
    Cancelled,
    Timeout,
    Io,
}
impl fmt::Display for ExtensionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid extension configuration: {s}"),
            Self::Forbidden(s) => write!(f, "extension access denied: {s}"),
            Self::Conflict(s) => write!(f, "extension changed: {s}"),
            Self::Unavailable(s) => write!(f, "extension unavailable: {s}"),
            Self::Protocol(s) => write!(f, "MCP protocol error: {s}"),
            Self::TooLarge => f.write_str("extension limit exceeded"),
            Self::Cancelled => f.write_str("extension operation cancelled"),
            Self::Timeout => f.write_str("extension operation timed out"),
            Self::Io => f.write_str("portable extension storage unavailable"),
        }
    }
}
impl std::error::Error for ExtensionError {}
impl From<std::io::Error> for ExtensionError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
pub type Result<T> = std::result::Result<T, ExtensionError>;
