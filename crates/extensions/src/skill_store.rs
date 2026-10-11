use crate::{
    model::SkillRecord,
    service::lock,
    storage::{MAX_SKILL, atomic, digest, read, safe_path, validate_grants},
    *,
};
use std::fs;
impl ExtensionService {
    pub fn list_skills(&self) -> Result<Vec<SkillSummary>> {
        let state = lock(&self.state)?;
        state
            .skills
            .values()
            .map(|record| {
                let content = read(&self.skill_path(record)?, MAX_SKILL)?;
                skills::refresh(record.summary.clone(), &content)
            })
            .collect()
    }
    pub fn read_skill(&self, id: &str) -> Result<SkillDocument> {
        let state = lock(&self.state)?;
        let record = state
            .skills
            .get(id)
            .ok_or_else(|| ExtensionError::Unavailable("skill is not installed".into()))?;
        let content = read(&self.skill_path(record)?, MAX_SKILL)?;
        Ok(SkillDocument {
            summary: skills::refresh(record.summary.clone(), &content)?,
            content,
        })
    }
    pub fn import_skill(
        &self,
        content: &str,
        trusted: bool,
        workspace_ids: Vec<String>,
        profiles: Vec<String>,
    ) -> Result<SkillSummary> {
        let (name, description, user, model) = skills::parse(content)?;
        validate_grants(&workspace_ids)?;
        validate_grants(&profiles)?;
        let mut state = lock(&self.state)?;
        if state.skills.contains_key(&name) {
            return Err(ExtensionError::Conflict("skill id is installed".into()));
        }
        if state.skills.len() >= 1024 {
            return Err(ExtensionError::TooLarge);
        }
        let summary = SkillSummary {
            id: name.clone(),
            name: name.clone(),
            description,
            enabled: true,
            trusted,
            user_invocable: user,
            model_invocable: model,
            workspace_ids,
            profiles,
            digest: digest(content.as_bytes()),
            plugin_id: None,
        };
        let path = format!("{name}/SKILL.md");
        let file = safe_path(&self.paths.skills(), &path)?;
        if file.exists() {
            return Err(ExtensionError::Conflict(
                "skill directory already exists".into(),
            ));
        }
        atomic(&file, content.as_bytes())?;
        let mut next = state.clone();
        next.skills.insert(
            name,
            SkillRecord {
                summary: summary.clone(),
                path,
            },
        );
        if let Err(error) = self.persist(&next) {
            let _ = fs::remove_file(file);
            return Err(error);
        }
        *state = next;
        Ok(summary)
    }
    pub fn save_skill(
        &self,
        id: &str,
        content: &str,
        expected_digest: Option<&str>,
    ) -> Result<SkillSummary> {
        let mut state = lock(&self.state)?;
        let record = state
            .skills
            .get(id)
            .ok_or_else(|| ExtensionError::Unavailable("skill is not installed".into()))?;
        if record.summary.plugin_id.is_some() {
            return Err(ExtensionError::Forbidden(
                "plugin skills are updated by reinstalling their plugin".into(),
            ));
        }
        let path = self.skill_path(record)?;
        let before = read(&path, MAX_SKILL)?;
        if expected_digest.is_some_and(|expected| expected != digest(before.as_bytes())) {
            return Err(ExtensionError::Conflict("skill content changed".into()));
        }
        let summary = skills::refresh(record.summary.clone(), content)?;
        if summary.name != record.summary.name {
            return Err(ExtensionError::Invalid("skill name is immutable".into()));
        }
        let mut next = state.clone();
        next.skills.get_mut(id).unwrap().summary = summary.clone();
        atomic(&path, content.as_bytes())?;
        if let Err(error) = self.persist(&next) {
            let _ = atomic(&path, before.as_bytes());
            return Err(error);
        }
        *state = next;
        Ok(summary)
    }
    pub fn set_skill_enabled(&self, id: &str, enabled: bool) -> Result<SkillSummary> {
        let mut state = lock(&self.state)?;
        let mut next = state.clone();
        let record = next
            .skills
            .get_mut(id)
            .ok_or_else(|| ExtensionError::Unavailable("skill is not installed".into()))?;
        record.summary.enabled = enabled;
        let summary = record.summary.clone();
        self.persist(&next)?;
        *state = next;
        Ok(summary)
    }
    pub fn remove_skill(&self, id: &str) -> Result<bool> {
        let mut state = lock(&self.state)?;
        let Some(record) = state.skills.get(id) else {
            return Ok(false);
        };
        if record.summary.plugin_id.is_some() {
            return Err(ExtensionError::Forbidden(
                "remove the contributing plugin".into(),
            ));
        }
        let path = self.skill_path(record)?;
        let mut next = state.clone();
        next.skills.remove(id);
        self.persist(&next)?;
        *state = next;
        fs::remove_file(path)?;
        Ok(true)
    }
}
