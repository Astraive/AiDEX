//! Opens trusted HTTPS pages in the user's browser.
//!
//! The command text is fixed: the URL travels to the OS as environment data and
//! never as part of the command, so it cannot inject shell syntax. Callers pass
//! only URLs they control (a pending sign-in flow, a built-in preset link); this
//! module never accepts a URL from a client without that check.
use crate::{AuthError, oauth};

/// Longest URL handed to the OS.
const MAX_URL_BYTES: usize = 4096;

/// Accept only absolute `https://` URLs with a host, without embedded
/// credentials, whitespace, or control characters.
pub fn validate_browser_url(url: &str) -> Result<(), AuthError> {
    if url.is_empty()
        || url.len() > MAX_URL_BYTES
        || url.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AuthError::Invalid);
    }
    let parsed = url::Url::parse(url).map_err(|_| AuthError::Invalid)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(str::is_empty)
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(AuthError::Invalid);
    }
    Ok(())
}

/// Open `url` in the default browser after the network gate admits it and the
/// URL passes [`validate_browser_url`]. Windows only; other platforms report
/// [`AuthError::Unavailable`].
pub fn open_in_browser(url: &str, network_allowed: &dyn Fn() -> bool) -> Result<(), AuthError> {
    oauth::network(network_allowed)?;
    validate_browser_url(url)?;
    launch(url)
}

#[cfg(windows)]
fn launch(url: &str) -> Result<(), AuthError> {
    use std::os::windows::process::CommandExt;
    /// CREATE_NO_WINDOW: no console flashes while PowerShell starts.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system = std::env::var_os("SystemRoot").ok_or(AuthError::Unavailable)?;
    let program =
        std::path::PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let status = std::process::Command::new(program)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Process -FilePath $env:AIDEX_OPEN_URL",
        ])
        .env("AIDEX_OPEN_URL", url)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|_| AuthError::Unavailable)?;
    if status.success() {
        Ok(())
    } else {
        Err(AuthError::Unavailable)
    }
}

#[cfg(not(windows))]
fn launch(url: &str) -> Result<(), AuthError> {
    let _ = url;
    Err(AuthError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_https_urls_are_accepted() {
        for good in [
            "https://platform.openai.com/api-keys",
            "https://auth.openai.com/oauth/authorize?client_id=x&state=y",
        ] {
            assert_eq!(validate_browser_url(good), Ok(()), "{good}");
        }
        for bad in [
            "",
            "http://platform.openai.com/api-keys",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "https://user:pass@example.com/",
            "https://example.com/a b",
            "https://example.com/\n",
            "https://",
            "//example.com/relative",
            "ms-settings:privacy",
        ] {
            assert_eq!(validate_browser_url(bad), Err(AuthError::Invalid), "{bad}");
        }
        assert_eq!(
            validate_browser_url(&format!(
                "https://example.com/{}",
                "a".repeat(MAX_URL_BYTES)
            )),
            Err(AuthError::Invalid)
        );
    }

    #[test]
    fn the_network_gate_is_checked_before_anything_is_launched() {
        assert_eq!(
            open_in_browser("https://platform.openai.com/api-keys", &|| false),
            Err(AuthError::NetworkOffline)
        );
        // The gate also wins over an invalid URL, so offline mode never leaks
        // whether a URL would have been accepted.
        assert_eq!(
            open_in_browser("http://example.com/", &|| false),
            Err(AuthError::NetworkOffline)
        );
        assert_eq!(
            open_in_browser("http://example.com/", &|| true),
            Err(AuthError::Invalid)
        );
    }
}
