//! Process diagnostics disclose categories and counts, never vendor output.
pub(crate) fn stderr_category(bytes: &[u8]) -> &'static str {
    let text = String::from_utf8_lossy(bytes);
    if text.contains("AssertionError") {
        "assertion_failed"
    } else if text.contains("ModuleNotFoundError") || text.contains("ImportError") {
        "dependency_missing"
    } else if text.contains("PermissionError") || text.contains("Access is denied") {
        "access_denied"
    } else if text.contains("SyntaxError") {
        "syntax_error"
    } else if text.contains("AIDEX_SANDBOX_ERROR") {
        "sandbox_helper_failed"
    } else if bytes.is_empty() {
        "none"
    } else {
        "vendor_output_withheld"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostic_is_a_closed_category_even_for_credential_bearing_output() {
        assert_eq!(
            stderr_category(b"secret-value-without-token-pattern AssertionError"),
            "assertion_failed"
        );
        assert_eq!(
            stderr_category(b"{\"access_token\":\"opaque-secret\"}"),
            "vendor_output_withheld"
        );
    }
}
