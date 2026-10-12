//! Skill names, scopes, and `<scope>:<name>` identifiers.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::SkillError;

/// Longest skill name, in bytes.
pub const MAX_SKILL_NAME_BYTES: usize = 64;

/// `[a-z0-9][a-z0-9-]{0,63}`: the slug a skill is invoked by (`$name`).
pub fn is_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_SKILL_NAME_BYTES
        && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// A presentation name: words of `name` split on `-`, `_`, `:` or whitespace,
/// each capitalised (`code-review` becomes `Code Review`).
pub fn display_name(name: &str) -> String {
    name.split(|c: char| c == '-' || c == '_' || c == ':' || c.is_whitespace())
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect::<String>()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where a skill was found. Declaration order is precedence order: for two
/// skills with the same name, the earlier scope wins.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    /// The workspace's own `.aidex/skills/`.
    Project,
    /// Skills other agents use in the workspace: `.agents/skills/`,
    /// `.claude/skills/`, `.codex/skills/` (read-only).
    Repo,
    /// The user's portable skills in `DrivePaths::skills()`.
    Drive,
    /// `skills/` of an enabled, trusted plugin.
    Plugin,
}

impl SkillScope {
    pub const ALL: [Self; 4] = [Self::Project, Self::Repo, Self::Drive, Self::Plugin];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Repo => "repo",
            Self::Drive => "drive",
            Self::Plugin => "plugin",
        }
    }

    /// Whether skills of this scope belong to a workspace (their enabled state is
    /// kept per workspace).
    pub const fn is_workspace(self) -> bool {
        matches!(self, Self::Project | Self::Repo)
    }

    /// Drive and plugin skills are the user's own (a plugin is trusted before it
    /// is enabled); workspace skills come with a repository and stay disabled until
    /// the user enables them.
    pub const fn enabled_by_default(self) -> bool {
        !self.is_workspace()
    }
}

impl fmt::Display for SkillScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SkillScope {
    type Err = SkillError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|scope| scope.as_str() == text)
            .ok_or_else(|| {
                SkillError::InvalidId("the scope must be project, repo, drive, or plugin".into())
            })
    }
}

/// `<scope>:<name>`, e.g. `drive:code-review`. Unique within one catalog.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SkillId {
    scope: SkillScope,
    name: String,
}

impl SkillId {
    pub fn new(scope: SkillScope, name: &str) -> Result<Self, SkillError> {
        if !is_skill_name(name) {
            return Err(SkillError::InvalidId(
                "skill names are 1-64 lowercase letters, digits, or `-`, starting with a letter or digit"
                    .into(),
            ));
        }
        Ok(Self {
            scope,
            name: name.to_owned(),
        })
    }

    /// Parse `<scope>:<name>`.
    pub fn parse(text: &str) -> Result<Self, SkillError> {
        let (scope, name) = text.trim().split_once(':').ok_or_else(|| {
            SkillError::InvalidId(
                "skill ids look like `<scope>:<name>`, e.g. `drive:review`".into(),
            )
        })?;
        Self::new(scope.parse()?, name)
    }

    pub fn scope(&self) -> SkillScope {
        self.scope
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for SkillId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.scope, self.name)
    }
}

impl FromStr for SkillId {
    type Err = SkillError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

impl Serialize for SkillId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SkillId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_lowercase_slugs() {
        for good in ["a", "0", "code-review", "x-1-y", &"a".repeat(64)] {
            assert!(is_skill_name(good), "{good}");
        }
        for bad in [
            "",
            "-a",
            "Code",
            "code_review",
            "a b",
            "ünï",
            &"a".repeat(65),
            "a.b",
        ] {
            assert!(!is_skill_name(bad), "{bad}");
        }
    }

    #[test]
    fn ids_round_trip_and_reject_unknown_scopes() {
        let id = SkillId::parse(" drive:review ").unwrap();
        assert_eq!((id.scope(), id.name()), (SkillScope::Drive, "review"));
        assert_eq!(id.to_string(), "drive:review");
        assert_eq!(
            serde_json::to_value(&id).unwrap(),
            serde_json::json!("drive:review")
        );
        for bad in ["review", "global:review", "drive:", "drive:Bad", ":x"] {
            assert!(SkillId::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn display_names_are_title_case() {
        assert_eq!(display_name("code-review"), "Code Review");
        assert_eq!(display_name("pdf"), "Pdf");
        assert_eq!(display_name("a--b"), "A B");
    }

    #[test]
    fn scopes_order_by_precedence_and_default_enablement() {
        assert!(SkillScope::Project < SkillScope::Repo);
        assert!(SkillScope::Repo < SkillScope::Drive);
        assert!(SkillScope::Drive < SkillScope::Plugin);
        assert!(!SkillScope::Project.enabled_by_default());
        assert!(!SkillScope::Repo.enabled_by_default());
        assert!(SkillScope::Drive.enabled_by_default());
        assert!(SkillScope::Plugin.enabled_by_default());
    }
}
