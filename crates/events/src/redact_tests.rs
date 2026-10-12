use super::*;

fn redacted(text: &str) -> String {
    redact_text(text).into_owned()
}

/// A credential-shaped fixture assembled at runtime, so no secret-shaped literal
/// appears in the source (fixtures are fake, but scanners cannot know that).
fn fake(prefix: &str, body: &str) -> String {
    format!("{prefix}{body}")
}

#[test]
fn provider_tokens_are_redacted_by_prefix() {
    let cases = [
        fake("sk-", "proj-FAKE0fake1FAKE2fake3"),
        fake("sk-", "ant-api03-FAKE0fake1FAKE2fake3_test-x"),
        fake("sk_", "live_FAKE0fake1FAKE2fake3"),
        fake("ghp_", "FAKE0fake1FAKE2fake3FAKE4fake5"),
        fake("gho_", "FAKE0fake1FAKE2fake3FAKE4fake5"),
        fake("github_", "pat_FAKE0fake1_FAKE2fake3FAKE4"),
        fake("glpat", "-FAKE0fake1FAKE2fake3"),
        fake("AKIA", "FAKEAWSKEY012345"),
        fake("ASIA", "FAKEAWSKEY012345"),
        fake("xoxb", "-FAKE0-fake1-FAKE2"),
        fake("xoxp", "-FAKE0fake1F"),
        fake("AIza", "FAKE0fake1FAKE2fake3FAKE4fake5_x"),
        fake("hf_", "FAKE0fake1FAKE2fake3FAKE4fake56"),
        fake(
            "eyJ",
            "hbGciOiJIUzI1NiJ9.eyJzdWIiOiJmYWtlIn0.FAKEsignatureFAKEsig",
        ),
    ];
    for token in cases {
        let text = format!("using {token} now");
        assert_eq!(redacted(&text), "using [REDACTED] now", "{token}");
    }
}

#[test]
fn bearer_and_authorization_credentials_are_redacted() {
    assert_eq!(
        redacted("curl -H 'Authorization: Bearer abc.DEF-123_456~xyz'"),
        "curl -H 'Authorization: Bearer [REDACTED]'"
    );
    assert_eq!(
        redacted("bearer   opaque-session-credential"),
        "bearer   [REDACTED]"
    );
    // The quoted header value is also a high-entropy assignment, so it goes entirely.
    assert_eq!(
        redacted(r#"{"Authorization":"Basic dXNlcjpwYXNzd29yZA=="}"#),
        r#"{"Authorization":"[REDACTED]"}"#
    );
    assert_eq!(
        redacted("Authorization: Basic dXNlcjpwYXNzd29yZA=="),
        "Authorization: Basic [REDACTED]"
    );
    assert_eq!(
        redacted("authorization = token 0123456789abcdef"),
        "authorization = token [REDACTED]"
    );
}

#[test]
fn authorization_headers_accept_horizontal_whitespace_and_http_token_schemes() {
    for (header, expected) in [
        (
            "Authorization:\tBasic dXNlcjpwYXNzd29yZA==",
            "Authorization:\tBasic [REDACTED]",
        ),
        (
            "Authorization: Api-Key opaque-credential",
            "Authorization: Api-Key [REDACTED]",
        ),
        (
            "authorization\t:\tCustom_2\t\tlowercase-credential",
            "authorization\t:\tCustom_2\t\t[REDACTED]",
        ),
        (
            "Authorization: !#$%&'*+-.^_`|~09Az opaque-credential",
            "Authorization: !#$%&'*+-.^_`|~09Az [REDACTED]",
        ),
    ] {
        assert_eq!(redacted(header), expected);
        assert_eq!(redacted(expected), expected);
    }
    assert_eq!(
        redacted("Authorization:\nBasic opaque-credential"),
        "Authorization:\nBasic opaque-credential"
    );
}

#[test]
fn sensitive_assignments_are_redacted_in_common_syntaxes() {
    assert_eq!(redacted("PASSWORD=hunter2"), "PASSWORD=[REDACTED]");
    assert_eq!(redacted("db_passwd: s3cr3t!"), "db_passwd: [REDACTED]");
    assert_eq!(
        redacted(r#"{"password": "correct horse battery"}"#),
        r#"{"password": "[REDACTED]"}"#
    );
    assert_eq!(
        redacted("export OPENAI_API_KEY=Abc123Def456Ghi789"),
        "export OPENAI_API_KEY=[REDACTED]"
    );
    assert_eq!(
        redacted("client_secret: 'x9Y8z7W6v5U4t3S2r1'"),
        "client_secret: '[REDACTED]'"
    );
    assert_eq!(
        redacted("x-auth-token:0123456789abcdef0123"),
        "x-auth-token:[REDACTED]"
    );
    assert_eq!(
        redacted("login --password hunter22 --user bob"),
        "login --password [REDACTED] --user bob"
    );
    assert_eq!(
        redacted("app --api-key Zz9Yy8Xx7Ww6Vv5Uu4"),
        "app --api-key [REDACTED]"
    );
}

#[test]
fn low_entropy_values_and_ordinary_text_are_kept() {
    for text in [
        "max_tokens=4096",
        "keyboard: logitech",
        "token: expired",
        "the secret: patience",
        "at 2026-10-10T07:51:01.042Z the build passed",
        "std::fs::read_to_string",
        "if a == b && c >= d",
        "https://example.com/path?q=1",
        "commit 3f2a1b0c4d5e6f708192a3b4c5d6e7f8091a2b3c",
        "-----BEGIN PUBLIC KEY-----\nMIIBIjANBg\n-----END PUBLIC KEY-----",
        "skeleton sk-short",
    ] {
        assert!(
            matches!(redact_text(text), Cow::Borrowed(_)),
            "{text:?} became {:?}",
            redact_text(text)
        );
    }
}

#[test]
fn pem_private_key_blocks_are_removed_entirely() {
    let label = fake("OPENSSH PRIVATE", " KEY");
    let text =
        format!("key:\n-----BEGIN {label}-----\nFAKEbody0\nFAKE\n-----END {label}-----\ndone");
    assert_eq!(redacted(&text), "key:\n[REDACTED]\ndone");
    let truncated = format!("-----BEGIN {}-----\nFAKEbody0", fake("RSA PRIVATE", " KEY"));
    assert_eq!(redacted(&truncated), "[REDACTED]");
}

#[test]
fn url_passwords_are_redacted() {
    assert_eq!(
        redacted("git clone https://x-access-token:abc123@github.com/o/r.git"),
        "git clone https://x-access-token:[REDACTED]@github.com/o/r.git"
    );
    assert_eq!(
        redacted("postgres://admin:pa55@db:5432/app"),
        "postgres://admin:[REDACTED]@db:5432/app"
    );
}

#[test]
fn redaction_is_idempotent_and_utf8_safe() {
    let text = format!(
        "naïve ✓ PASSWORD=pässwörd → {} ✓",
        fake("sk-", "proj-FAKE0fake1FAKE2fake3")
    );
    let once = redacted(&text);
    assert_eq!(once, "naïve ✓ PASSWORD=[REDACTED] → [REDACTED] ✓");
    assert_eq!(redacted(&once), once);
}

#[test]
fn long_text_is_truncated_with_an_explicit_marker() {
    let long = "é".repeat(MAX_TEXT_BYTES);
    let bounded = sanitize_text(&long);
    assert!(bounded.len() <= MAX_TEXT_BYTES, "{}", bounded.len());
    assert!(bounded.contains(TRUNCATION_MARKER));
    assert!(bounded.ends_with("bytes omitted]"));
    let short = "short";
    assert!(matches!(sanitize_text(short), Cow::Borrowed("short")));
    // Redaction happens before truncation, so a secret is never split by the cut.
    let mut secret_tail = "x".repeat(MAX_TEXT_BYTES - 30);
    secret_tail.push(' ');
    secret_tail.push_str(&fake("sk-", "proj-FAKE0fake1FAKE2fake3"));
    let bounded = sanitize_text(&secret_tail);
    assert!(!bounded.contains("FAKE0fake1"));
}
