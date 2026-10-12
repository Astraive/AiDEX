//! Changing skills: enablement (with events) and drive skill authoring.

use std::{
    fs,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use aidex_core::EntityId;
use aidex_drive::DrivePaths;
use aidex_events::{EventKind, EventScope, EventSink, ExtensionKind};

use crate::{
    ExtraSkillRoot, SkillCatalog, SkillDraft, SkillError, SkillId, SkillScope, SkillSummary,
    SkillWorkspace,
    authoring::{render_replacement, render_skill_file},
    fsio::{self, Entry, ReadFailure},
    manifest::MAX_SKILL_FILE_BYTES,
    name::is_skill_name,
    state::{SkillState, state_path},
};

/// The daemon's skill service. Writes are serialised so concurrent requests
/// never lose an update to `skills.toml` or a drive skill.
pub struct SkillService {
    paths: DrivePaths,
    events: Arc<dyn EventSink>,
    writes: Mutex<()>,
}

impl std::fmt::Debug for SkillService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillService")
            .field("skills", &self.paths.skills())
            .finish_non_exhaustive()
    }
}

impl SkillService {
    pub fn new(paths: DrivePaths, events: Arc<dyn EventSink>) -> Self {
        Self {
            paths,
            events,
            writes: Mutex::new(()),
        }
    }

    pub fn paths(&self) -> &DrivePaths {
        &self.paths
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.writes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A fresh snapshot (skills are rescanned on every call; nothing is cached).
    pub fn catalog(
        &self,
        workspace: Option<&SkillWorkspace>,
        extra_roots: &[ExtraSkillRoot],
    ) -> Result<SkillCatalog, SkillError> {
        SkillCatalog::discover(&self.paths, workspace, extra_roots)
    }

    /// Record the user's decision for `id` and emit `extension.enabled` or
    /// `extension.disabled` when the effective state changes. A skill with an
    /// error can be disabled but not enabled.
    pub fn set_enabled(
        &self,
        workspace: Option<&SkillWorkspace>,
        extra_roots: &[ExtraSkillRoot],
        id: &SkillId,
        enabled: bool,
    ) -> Result<SkillSummary, SkillError> {
        let _guard = self.lock();
        let catalog = self.catalog(workspace, extra_roots)?;
        let mut summary = catalog.summary(id)?;
        if enabled && let Some(error) = &summary.error {
            return Err(SkillError::Invalid(format!(
                "skill `{id}` cannot be enabled until it is fixed: {error}"
            )));
        }
        let key = catalog.state_key(id);
        let mut state = SkillState::load(&state_path(&self.paths))?;
        let before = state.is_enabled(&key);
        state.set(key, enabled);
        state.save(&self.paths)?;
        summary.enabled = enabled;
        if before != enabled {
            self.emit(id, catalog.workspace_id(), enabled)?;
        }
        Ok(summary)
    }

    fn emit(
        &self,
        id: &SkillId,
        workspace_id: Option<&EntityId>,
        enabled: bool,
    ) -> Result<(), SkillError> {
        let extension_id = EntityId::new(id.to_string())
            .map_err(|error| SkillError::EventNotRecorded(error.to_string()))?;
        let kind = ExtensionKind::Skill;
        let event = if enabled {
            EventKind::ExtensionEnabled { extension_id, kind }
        } else {
            EventKind::ExtensionDisabled { extension_id, kind }
        };
        let scope = match workspace_id {
            Some(workspace) if id.scope().is_workspace() => {
                EventScope::workspace(workspace.clone())
            }
            _ => EventScope::global(),
        };
        self.events
            .emit(scope, event)
            .map(|_| ())
            .map_err(|error| SkillError::EventNotRecorded(error.to_string()))
    }

    /// Create or replace the drive skill `draft.name` (`skills/<name>/SKILL.md`,
    /// written atomically). A drive skill declaring that name in another
    /// directory is replaced in place. A replaced skill keeps every front matter
    /// key the draft does not set (`name`, `description`, `version`), such as who
    /// may invoke it and its `allowed-tools`.
    pub fn save_drive_skill(&self, draft: &SkillDraft) -> Result<SkillSummary, SkillError> {
        let mut text = render_skill_file(draft)?;
        let _guard = self.lock();
        let skills = self.paths.skills();
        let root = self
            .paths
            .ensure_dir(&skills)
            .map_err(|_| SkillError::UnsafePath(skills.clone()))?;
        let catalog = self.catalog(None, &[])?;
        let id = SkillId::new(SkillScope::Drive, &draft.name)?;
        let (dir, file) = match catalog.find(&id) {
            Ok(existing) => {
                if existing.linked {
                    return Err(SkillError::UnsafePath(existing.dir.clone()));
                }
                let file = match &existing.file {
                    Some(file) => {
                        match fsio::read_text(file, MAX_SKILL_FILE_BYTES) {
                            Ok(current) => text = render_replacement(draft, &current)?,
                            // Too large or not text: nothing to keep, it is replaced whole.
                            Err(ReadFailure::TooLarge | ReadFailure::NotUtf8) => {}
                            Err(ReadFailure::Io(error)) => {
                                return Err(SkillError::io(file, &error));
                            }
                        }
                        file.clone()
                    }
                    None => existing.dir.join("SKILL.md"),
                };
                (existing.dir.clone(), file)
            }
            Err(SkillError::NotFound(_)) => {
                let dir = root.join(&draft.name);
                if let Some(other) = catalog.skills().iter().find(|skill| skill.dir == dir) {
                    return Err(SkillError::Conflict(format!(
                        "`{}` already holds the skill `{}`",
                        other.summary.location, other.summary.name
                    )));
                }
                let file = dir.join("SKILL.md");
                (dir, file)
            }
            Err(error) => return Err(error),
        };
        let dir = self
            .paths
            .ensure_dir(&dir)
            .map_err(|_| SkillError::UnsafePath(dir.clone()))?;
        if dir.parent() != Some(root.as_path()) {
            return Err(SkillError::UnsafePath(dir));
        }
        fsio::write_atomic(&file, text.as_bytes())
            .map_err(|error| SkillError::io(&file, &error))?;
        self.catalog(None, &[])?.summary(&id)
    }

    /// Delete the drive skill `name` (its whole directory) and forget its
    /// enablement decision.
    pub fn delete_drive_skill(&self, name: &str) -> Result<SkillSummary, SkillError> {
        if !is_skill_name(name) {
            return Err(SkillError::InvalidId(
                "skill names are 1-64 lowercase letters, digits, or `-`".into(),
            ));
        }
        let id = SkillId::new(SkillScope::Drive, name)?;
        let _guard = self.lock();
        let catalog = self.catalog(None, &[])?;
        let skill = catalog.find(&id)?;
        let root = self.paths.skills();
        let dir = skill.dir.clone();
        if skill.linked || dir.parent() != Some(root.as_path()) {
            return Err(SkillError::UnsafePath(dir));
        }
        match fsio::inspect(&dir) {
            Ok(Entry::Dir) => {}
            Ok(_) => return Err(SkillError::UnsafePath(dir)),
            Err(error) => return Err(SkillError::io(&dir, &error)),
        }
        fs::remove_dir_all(&dir).map_err(|error| SkillError::io(&dir, &error))?;
        let mut state = SkillState::load(&state_path(&self.paths))?;
        if state.remove(&catalog.state_key(&id)) {
            state.save(&self.paths)?;
        }
        Ok(skill.summary.clone())
    }
}
