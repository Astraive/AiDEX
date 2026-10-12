//! The skills visible to one workspace (or none), with their enabled state.

use std::path::PathBuf;

use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use aidex_events::redact_text;
use serde::{Deserialize, Serialize};

use crate::{
    ExtraSkillRoot, FrontMatterValue, SkillError, SkillId, SkillIssue, SkillScope, SkillWorkspace,
    discover::{self, Found},
    fsio::{self, Entry},
    name::display_name,
    state::{SkillState, StateKey, state_path},
};

/// One skill as `skill.list` reports it. Text fields are credential-redacted.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillSummary {
    /// `<scope>:<name>`.
    pub id: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub scope: SkillScope,
    /// The user's decision, or the scope default. A skill with an `error` is
    /// never invoked, whatever this says.
    pub enabled: bool,
    pub user_invocable: bool,
    pub model_invocable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Drive- or workspace-relative path of the skill file (or directory).
    pub location: String,
    pub size_bytes: u64,
    /// The plugin that contributes a plugin-scope skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    /// Why the skill cannot be used (too large, malformed, a link, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One front matter key and its value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FrontMatterEntry {
    pub key: String,
    pub value: FrontMatterValue,
}

/// `skill.show`: the summary, the redacted instructions, and the front matter.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillDetail {
    #[serde(flatten)]
    pub summary: SkillSummary,
    /// Absent when the skill has an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub front_matter: Vec<FrontMatterEntry>,
    /// Credential-like text was hidden in the `body` (or in the `description` or
    /// `version`) returned here. Saving these values back with `skill.save` would
    /// replace the hidden text with the redaction marker: edit the file instead.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub body_redacted: bool,
}

/// The instructions of an enabled, valid skill, for injection. `body` is
/// credential-redacted but still untrusted: wrap it as untrusted data.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillBody {
    pub id: SkillId,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub body: String,
    pub user_invocable: bool,
    pub model_invocable: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Skill {
    pub id: SkillId,
    pub summary: SkillSummary,
    pub dir: PathBuf,
    pub file: Option<PathBuf>,
    pub linked: bool,
}

/// A snapshot of discovered skills and their enabled state.
#[derive(Clone, Debug)]
pub struct SkillCatalog {
    workspace_id: Option<EntityId>,
    skills: Vec<Skill>,
    issues: Vec<SkillIssue>,
}

fn redact(text: &str) -> String {
    redact_text(text).into_owned()
}

impl SkillCatalog {
    /// Discover skills from the workspace (when given), the drive, and
    /// `extra_roots` (enabled plugins), and apply `private_config/skills.toml`.
    pub fn discover(
        paths: &DrivePaths,
        workspace: Option<&SkillWorkspace>,
        extra_roots: &[ExtraSkillRoot],
    ) -> Result<Self, SkillError> {
        let discovery = discover::discover(paths, workspace, extra_roots)?;
        let state = SkillState::load(&state_path(paths))?;
        let workspace_id = workspace.map(|workspace| workspace.id.clone());
        let mut skills: Vec<Skill> = discovery
            .found
            .into_iter()
            .map(|found| Self::skill(found, workspace_id.as_ref(), &state))
            .collect::<Result<_, _>>()?;
        skills.sort_by(|a, b| a.summary.name.cmp(&b.summary.name));
        Ok(Self {
            workspace_id,
            skills,
            issues: discovery.issues,
        })
    }

    fn skill(
        found: Found,
        workspace_id: Option<&EntityId>,
        state: &SkillState,
    ) -> Result<Skill, SkillError> {
        let id = SkillId::new(found.scope, &found.name)?;
        let enabled = state.is_enabled(&state_key(&id, workspace_id));
        let summary = match &found.manifest {
            Ok(manifest) => SkillSummary {
                id: id.to_string(),
                name: found.name.clone(),
                display_name: redact(&manifest.display_name),
                description: redact(&manifest.description),
                scope: found.scope,
                enabled,
                user_invocable: manifest.user_invocable,
                model_invocable: manifest.model_invocable,
                version: manifest.version.as_deref().map(redact),
                location: found.location.clone(),
                size_bytes: found.size_bytes,
                plugin: found.owner.clone(),
                error: None,
            },
            Err(error) => SkillSummary {
                id: id.to_string(),
                name: found.name.clone(),
                display_name: display_name(&found.name),
                description: String::new(),
                scope: found.scope,
                enabled,
                user_invocable: false,
                model_invocable: false,
                version: None,
                location: found.location.clone(),
                size_bytes: found.size_bytes,
                plugin: found.owner.clone(),
                error: Some(redact(error)),
            },
        };
        Ok(Skill {
            id,
            summary,
            dir: found.dir,
            file: found.file,
            linked: found.linked,
        })
    }

    /// The workspace this catalog includes, if any.
    pub fn workspace_id(&self) -> Option<&EntityId> {
        self.workspace_id.as_ref()
    }

    /// Every visible skill, sorted by name.
    pub fn summaries(&self) -> Vec<SkillSummary> {
        self.skills
            .iter()
            .map(|skill| skill.summary.clone())
            .collect()
    }

    /// Enabled, valid skills (candidates for invocation), sorted by name.
    pub fn enabled(&self) -> impl Iterator<Item = &SkillSummary> {
        self.skills
            .iter()
            .map(|skill| &skill.summary)
            .filter(|summary| summary.enabled && summary.error.is_none())
    }

    /// Roots and entries that could not be considered.
    pub fn issues(&self) -> &[SkillIssue] {
        &self.issues
    }

    /// Every visible skill with its on-disk location, sorted by name.
    pub(crate) fn skills(&self) -> &[Skill] {
        &self.skills
    }

    pub(crate) fn find(&self, id: &SkillId) -> Result<&Skill, SkillError> {
        if id.scope().is_workspace() && self.workspace_id.is_none() {
            return Err(SkillError::WorkspaceRequired);
        }
        self.skills
            .iter()
            .find(|skill| &skill.id == id)
            .ok_or_else(|| SkillError::NotFound(id.to_string()))
    }

    pub(crate) fn state_key(&self, id: &SkillId) -> StateKey {
        state_key(id, self.workspace_id.as_ref())
    }

    pub fn summary(&self, id: &SkillId) -> Result<SkillSummary, SkillError> {
        self.find(id).map(|skill| skill.summary.clone())
    }

    /// The summary plus the instructions and front matter, read again from disk.
    pub fn inspect(&self, id: &SkillId) -> Result<SkillDetail, SkillError> {
        let skill = self.find(id)?;
        let mut summary = skill.summary.clone();
        if summary.error.is_some() {
            return Ok(SkillDetail {
                summary,
                body: None,
                front_matter: Vec::new(),
                body_redacted: false,
            });
        }
        match Self::read(skill) {
            Ok((front_matter, body)) => {
                let shown = redact(&body);
                // What an editor would save back: the body, description, and version.
                let body_redacted = shown != body
                    || front_matter.iter().any(|(key, value)| {
                        matches!(key.as_str(), "description" | "version")
                            && redact_value(value.clone()) != *value
                    });
                Ok(SkillDetail {
                    summary,
                    body: Some(shown),
                    front_matter: front_matter
                        .into_iter()
                        .map(|(key, value)| FrontMatterEntry {
                            key,
                            value: redact_value(value),
                        })
                        .collect(),
                    body_redacted,
                })
            }
            Err(error) => {
                summary.error = Some(redact(&error));
                Ok(SkillDetail {
                    summary,
                    body: None,
                    front_matter: Vec::new(),
                    body_redacted: false,
                })
            }
        }
    }

    /// The redacted instructions of an enabled, valid skill, read again from
    /// disk. Unknown, disabled, and invalid skills are [`SkillError::Unavailable`].
    pub fn load_body(&self, id: &SkillId) -> Result<SkillBody, SkillError> {
        let unavailable = |reason: &str| SkillError::Unavailable {
            id: id.to_string(),
            reason: reason.to_owned(),
        };
        let skill = match self.find(id) {
            Ok(skill) => skill,
            Err(SkillError::NotFound(_) | SkillError::WorkspaceRequired) => {
                return Err(unavailable("no such skill is visible here"));
            }
            Err(error) => return Err(error),
        };
        if !skill.summary.enabled {
            return Err(unavailable("it is disabled"));
        }
        if skill.summary.error.is_some() {
            return Err(unavailable("it has an error; see `skill.show`"));
        }
        let (_, body) = Self::read(skill).map_err(|_| unavailable("it changed on disk"))?;
        Ok(SkillBody {
            id: id.clone(),
            name: skill.summary.name.clone(),
            display_name: skill.summary.display_name.clone(),
            description: skill.summary.description.clone(),
            body: redact(&body),
            user_invocable: skill.summary.user_invocable,
            model_invocable: skill.summary.model_invocable,
        })
    }

    /// Re-read and re-validate the skill file; the name must still match.
    fn read(skill: &Skill) -> Result<(Vec<(String, FrontMatterValue)>, String), String> {
        let file = skill
            .file
            .as_ref()
            .ok_or_else(|| "the skill file is missing".to_owned())?;
        let size = match fsio::inspect(file) {
            Ok(Entry::File(size)) => size,
            _ => return Err("the skill file changed on disk; list skills again".into()),
        };
        let directory = skill.dir.file_name().and_then(|name| name.to_str());
        let (manifest, document) =
            discover::read_document(file, size, directory).map_err(|(_, error)| error)?;
        if manifest.name != skill.summary.name {
            return Err("the skill changed on disk; list skills again".into());
        }
        let front_matter = document
            .front_matter
            .map(|front_matter| front_matter.entries().to_vec())
            .unwrap_or_default();
        Ok((front_matter, document.body))
    }
}

fn redact_value(value: FrontMatterValue) -> FrontMatterValue {
    match value {
        FrontMatterValue::Text(text) => FrontMatterValue::Text(redact(&text)),
        FrontMatterValue::List(items) => {
            FrontMatterValue::List(items.iter().map(|item| redact(item)).collect())
        }
        FrontMatterValue::Bool(value) => FrontMatterValue::Bool(value),
    }
}

/// Workspace skills are keyed by workspace; drive and plugin skills are not.
pub(crate) fn state_key(id: &SkillId, workspace_id: Option<&EntityId>) -> StateKey {
    let workspace = if id.scope().is_workspace() {
        workspace_id.cloned()
    } else {
        None
    };
    (id.scope(), id.name().to_owned(), workspace)
}
