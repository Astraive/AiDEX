//! Secret metadata: names, kinds, consumer policies, and value-free descriptions.

use crate::{ConsumerScope, VaultError};
use aidex_core::{SecretRef, Timestamp};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};

/// Maximum secret-name length in bytes.
pub const MAX_NAME_LEN: usize = 128;
/// Maximum number of consumers in one policy.
pub const MAX_CONSUMERS: usize = 32;

/// A validated secret name: 1-128 characters of `A-Z a-z 0-9 . _ -`, starting with a
/// letter or digit. Names are case-sensitive and are stored only inside the encrypted index.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct SecretName(String);

impl SecretName {
    pub fn new(value: impl Into<String>) -> Result<Self, VaultError> {
        let value = value.into();
        let bytes = value.as_bytes();
        if bytes.is_empty() || bytes.len() > MAX_NAME_LEN {
            return Err(VaultError::InvalidName(format!(
                "must be 1 to {MAX_NAME_LEN} characters"
            )));
        }
        if !bytes[0].is_ascii_alphanumeric()
            || !bytes
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(VaultError::InvalidName(
                "use letters, digits, `.`, `_`, or `-`, starting with a letter or digit".into(),
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for SecretName {
    type Err = VaultError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl Serialize for SecretName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SecretName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

/// What a secret is, for display and consumer-side handling. Kinds do not change
/// how values are protected.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    ApiKey,
    Token,
    Password,
    SshKey,
    PrivateKey,
    #[default]
    Generic,
}

impl SecretKind {
    pub const ALL: [Self; 6] = [
        Self::ApiKey,
        Self::Token,
        Self::Password,
        Self::SshKey,
        Self::PrivateKey,
        Self::Generic,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::Token => "token",
            Self::Password => "password",
            Self::SshKey => "ssh_key",
            Self::PrivateKey => "private_key",
            Self::Generic => "generic",
        }
    }

    pub fn parse(text: &str) -> Result<Self, VaultError> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| VaultError::InvalidKind(text.to_owned()))
    }
}

impl fmt::Display for SecretKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SecretKind {
    type Err = VaultError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// Which consumers may be issued handles for a secret. An empty list means no
/// consumer can ever receive the value.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretPolicy {
    pub allowed_consumers: Vec<ConsumerScope>,
}

impl SecretPolicy {
    /// Build a policy, dropping duplicate consumers while preserving order.
    pub fn new(consumers: impl IntoIterator<Item = ConsumerScope>) -> Result<Self, VaultError> {
        let mut allowed_consumers: Vec<ConsumerScope> = Vec::new();
        for consumer in consumers {
            if !allowed_consumers.contains(&consumer) {
                allowed_consumers.push(consumer);
            }
        }
        let policy = Self { allowed_consumers };
        policy.validate()?;
        Ok(policy)
    }

    /// Exact-match membership; there are no wildcards.
    pub fn allows(&self, consumer: &ConsumerScope) -> bool {
        self.allowed_consumers.contains(consumer)
    }

    pub(crate) fn validate(&self) -> Result<(), VaultError> {
        if self.allowed_consumers.len() > MAX_CONSUMERS {
            return Err(VaultError::LimitExceeded("at most 32 consumers per secret"));
        }
        for (position, consumer) in self.allowed_consumers.iter().enumerate() {
            if self.allowed_consumers[..position].contains(consumer) {
                return Err(VaultError::InvalidConsumer(format!(
                    "`{consumer}` is listed twice"
                )));
            }
        }
        Ok(())
    }
}

/// A value-free description of a stored secret.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretMeta {
    pub secret_ref: SecretRef,
    pub name: SecretName,
    pub kind: SecretKind,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub policy: SecretPolicy,
}

/// Which stored revision of a secret is current: its record and how many times
/// its value was written. Replacing the value raises `revision`; deleting and
/// re-creating a secret gives it a new `secret_ref`. Contains no name or value.
/// Callers bind an approval or a resume to it and resolve with
/// [`VaultService::resolve_versioned`](crate::VaultService::resolve_versioned);
/// the vault never stores it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretVersion {
    pub secret_ref: SecretRef,
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VaultErrorKind;

    #[test]
    fn secret_names_are_validated() {
        for valid in ["openai", "GITHUB_TOKEN", "a", "prod.db-password", "x9"] {
            assert_eq!(SecretName::new(valid).unwrap().as_str(), valid);
        }
        for invalid in ["", ".hidden", "-x", "a b", "a/b", "ü", &"a".repeat(129)] {
            let error = SecretName::new(invalid).unwrap_err();
            assert_eq!(error.kind(), VaultErrorKind::InvalidArgument, "{invalid}");
        }
        assert!(serde_json::from_str::<SecretName>("\"bad name\"").is_err());
    }

    #[test]
    fn kinds_parse_their_wire_names() {
        for kind in SecretKind::ALL {
            assert_eq!(SecretKind::parse(kind.as_str()).unwrap(), kind);
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::json!(kind.as_str())
            );
        }
        assert!(SecretKind::parse("cookie").is_err());
    }

    #[test]
    fn policies_deduplicate_and_match_exactly() {
        let openai = ConsumerScope::provider("openai").unwrap();
        let github = ConsumerScope::mcp("github").unwrap();
        let policy = SecretPolicy::new([openai.clone(), github.clone(), openai.clone()]).unwrap();
        assert_eq!(policy.allowed_consumers, [openai.clone(), github.clone()]);
        assert!(policy.allows(&openai));
        assert!(!policy.allows(&ConsumerScope::provider("openai2").unwrap()));
        assert!(!SecretPolicy::default().allows(&openai));
        let many = (0..=MAX_CONSUMERS).map(|i| ConsumerScope::provider(format!("p{i}")).unwrap());
        assert_eq!(
            SecretPolicy::new(many).unwrap_err().kind(),
            VaultErrorKind::LimitExceeded
        );
        let duplicated = SecretPolicy {
            allowed_consumers: vec![github.clone(), github],
        };
        assert!(duplicated.validate().is_err());
    }
}
