//! `private_config/skills.toml`: which skills the user enabled or disabled.
//!
//! A closed-schema file of its own (older daemons never read it). Only explicit
//! decisions are stored; a skill without an entry uses its scope's default
//! ([`SkillScope::enabled_by_default`]). Workspace (project and repo) decisions
//! are kept per workspace id.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use serde::{Deserialize, Serialize};

use crate::{
    SkillError, SkillScope,
    fsio::{self, Entry, ReadFailure},
    name::is_skill_name,
};

/// State file name inside `DrivePaths::config()`.
pub const SKILLS_STATE_FILE: &str = "skills.toml";
/// Schema version written by this build.
pub const STATE_VERSION: u32 = 1;
/// Most entries the state file may hold.
const MAX_STATE_ENTRIES: usize = 4096;
/// Largest state file accepted.
const MAX_STATE_BYTES: u64 = 1024 * 1024;

/// `skills.toml`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillStateFile {
    pub version: u32,
    #[serde(default, rename = "skill", skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillStateEntry>,
}

/// One explicit enable/disable decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillStateEntry {
    pub scope: SkillScope,
    pub name: String,
    /// Required for project and repo skills, absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<EntityId>,
    pub enabled: bool,
}

pub(crate) type StateKey = (SkillScope, String, Option<EntityId>);

/// The decisions in memory.
#[derive(Clone, Debug, Default)]
pub(crate) struct SkillState {
    entries: BTreeMap<StateKey, bool>,
}

pub(crate) fn state_path(paths: &DrivePaths) -> PathBuf {
    paths.config().join(SKILLS_STATE_FILE)
}

impl SkillState {
    /// Read the state file; a missing file means no decisions.
    pub fn load(path: &Path) -> Result<Self, SkillError> {
        let invalid = |message: String| SkillError::State {
            path: path.to_path_buf(),
            message,
        };
        match fsio::inspect(path).map_err(|error| SkillError::io(path, &error))? {
            Entry::Missing => return Ok(Self::default()),
            Entry::File(_) => {}
            _ => return Err(SkillError::UnsafePath(path.to_path_buf())),
        }
        let text = match fsio::read_text(path, MAX_STATE_BYTES) {
            Ok(text) => text,
            Err(ReadFailure::TooLarge) => return Err(invalid("file is larger than 1 MiB".into())),
            Err(ReadFailure::NotUtf8) => return Err(invalid("file is not UTF-8 text".into())),
            Err(ReadFailure::Io(error)) => return Err(SkillError::io(path, &error)),
        };
        // `toml`'s messages quote the offending line; report the position only.
        let file: SkillStateFile = toml::from_str(&text).map_err(|error| {
            let line = error
                .span()
                .map(|span| text[..span.start.min(text.len())].matches('\n').count() + 1);
            invalid(match line {
                Some(line) => format!("line {line} does not match the skills.toml schema"),
                None => "the file does not match the skills.toml schema".into(),
            })
        })?;
        if file.version != STATE_VERSION {
            return Err(SkillError::StateVersion {
                path: path.to_path_buf(),
                found: file.version,
            });
        }
        Self::from_file(file).map_err(invalid)
    }

    fn from_file(file: SkillStateFile) -> Result<Self, String> {
        if file.skills.len() > MAX_STATE_ENTRIES {
            return Err(format!("more than {MAX_STATE_ENTRIES} entries"));
        }
        let mut entries = BTreeMap::new();
        for (index, entry) in file.skills.into_iter().enumerate() {
            let position = index + 1;
            if !is_skill_name(&entry.name) {
                return Err(format!("entry {position} has an invalid skill name"));
            }
            if entry.scope.is_workspace() != entry.workspace_id.is_some() {
                return Err(format!(
                    "entry {position}: project and repo skills need a workspace_id; drive and plugin skills take none"
                ));
            }
            let key = (entry.scope, entry.name, entry.workspace_id);
            if entries.insert(key, entry.enabled).is_some() {
                return Err(format!("entry {position} repeats an earlier skill"));
            }
        }
        Ok(Self { entries })
    }

    /// Whether the skill is enabled: the stored decision or the scope default.
    pub fn is_enabled(&self, key: &StateKey) -> bool {
        self.entries
            .get(key)
            .copied()
            .unwrap_or_else(|| key.0.enabled_by_default())
    }

    pub fn set(&mut self, key: StateKey, enabled: bool) {
        self.entries.insert(key, enabled);
    }

    /// Forget the decision (the scope default applies again).
    pub fn remove(&mut self, key: &StateKey) -> bool {
        self.entries.remove(key).is_some()
    }

    fn to_file(&self) -> SkillStateFile {
        SkillStateFile {
            version: STATE_VERSION,
            skills: self
                .entries
                .iter()
                .map(|((scope, name, workspace_id), enabled)| SkillStateEntry {
                    scope: *scope,
                    name: name.clone(),
                    workspace_id: workspace_id.clone(),
                    enabled: *enabled,
                })
                .collect(),
        }
    }

    /// Write the state atomically, creating `private_config/` when needed.
    pub fn save(&self, paths: &DrivePaths) -> Result<(), SkillError> {
        if self.entries.len() > MAX_STATE_ENTRIES {
            return Err(SkillError::Conflict(format!(
                "skills.toml can hold at most {MAX_STATE_ENTRIES} decisions"
            )));
        }
        let config = paths.config();
        let dir = paths
            .ensure_dir(&config)
            .map_err(|_| SkillError::UnsafePath(config.clone()))?;
        let path = dir.join(SKILLS_STATE_FILE);
        let text = toml::to_string(&self.to_file()).map_err(|error| SkillError::Io {
            path: path.clone(),
            message: format!("cannot encode skills.toml: {error}"),
        })?;
        let text =
            format!("# AiDEX skill enablement. Change with `aidex skills enable|disable`.\n{text}");
        fsio::write_atomic(&path, text.as_bytes()).map_err(|error| SkillError::io(&path, &error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(text: &str) -> Result<SkillState, SkillError> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SKILLS_STATE_FILE);
        std::fs::write(&path, text).unwrap();
        SkillState::load(&path)
    }

    #[test]
    fn schema_is_closed_versioned_and_validated() {
        let state = load(concat!(
            "version = 1\n",
            "[[skill]]\nscope = \"drive\"\nname = \"review\"\nenabled = false\n",
            "[[skill]]\nscope = \"project\"\nname = \"deploy\"\nworkspace_id = \"ws_1\"\nenabled = true\n",
        ))
        .unwrap();
        assert!(!state.is_enabled(&(SkillScope::Drive, "review".into(), None)));
        let workspace = EntityId::new("ws_1").unwrap();
        assert!(state.is_enabled(&(SkillScope::Project, "deploy".into(), Some(workspace))));
        let other = EntityId::new("ws_2").unwrap();
        assert!(!state.is_enabled(&(SkillScope::Project, "deploy".into(), Some(other))));
        assert!(state.is_enabled(&(SkillScope::Plugin, "any".into(), None)));

        let secret = "sk-proj-PASTEDSECRET";
        for (text, reason) in [
            (format!("version = 1\nextra = \"{secret}\"\n"), "invalid_state_file"),
            (
                "version = 1\n[[skill]]\nscope = \"drive\"\nname = \"Bad\"\nenabled = true\n"
                    .to_owned(),
                "invalid_state_file",
            ),
            (
                "version = 1\n[[skill]]\nscope = \"repo\"\nname = \"a\"\nenabled = true\n"
                    .to_owned(),
                "invalid_state_file",
            ),
            (
                "version = 1\n[[skill]]\nscope = \"drive\"\nname = \"a\"\nworkspace_id = \"ws\"\nenabled = true\n"
                    .to_owned(),
                "invalid_state_file",
            ),
            (
                "version = 1\n[[skill]]\nscope = \"drive\"\nname = \"a\"\nenabled = true\n[[skill]]\nscope = \"drive\"\nname = \"a\"\nenabled = false\n"
                    .to_owned(),
                "invalid_state_file",
            ),
            ("version = 2\n".to_owned(), "unsupported_state_version"),
        ] {
            let error = load(&text).unwrap_err();
            assert_eq!(error.reason(), reason, "{text}: {error}");
            assert!(!error.to_string().contains(secret), "{error}");
        }
    }
}
