use crate::{
    model::{ServerRecord, SkillRecord},
    service::{hash_json, lock, validate_server},
    storage::{MAX_SKILL, atomic, digest, read, safe_path, valid_id, validate_grants},
    *,
};
use std::{collections::BTreeMap, fs};
impl ExtensionService {
    pub fn list_plugins(&self) -> Result<Vec<PluginSummary>> {
        Ok(lock(&self.state)?.plugins.values().cloned().collect())
    }
    pub(crate) fn verify_plugin(&self, plugin: &PluginSummary) -> Result<()> {
        let path = safe_path(
            &self.paths.plugins(),
            &format!("{}/manifest.json", plugin.manifest.id),
        )?;
        let original: PluginManifest = serde_json::from_str(&read(&path, MAX_SKILL)?)
            .map_err(|_| ExtensionError::Conflict("plugin manifest changed".into()))?;
        let mut files = BTreeMap::new();
        for skill in &original.skills {
            let path = safe_path(
                &self.paths.plugins(),
                &format!("{}/{}", plugin.manifest.id, skill.path),
            )?;
            files.insert(skill.path.clone(), read(&path, MAX_SKILL)?);
        }
        if hash_json(&(&original, &files))? != plugin.digest {
            return Err(ExtensionError::Conflict(
                "plugin content changed after trust".into(),
            ));
        }
        Ok(())
    }
    pub fn install_plugin(
        &self,
        mut manifest: PluginManifest,
        files: BTreeMap<String, String>,
    ) -> Result<PluginSummary> {
        for server in &mut manifest.mcp_servers {
            crate::portable::normalize(self.paths.root(), server)?;
        }
        valid_id(&manifest.id)?;
        validate_grants(&manifest.workspace_ids)?;
        validate_grants(&manifest.profiles)?;
        if manifest.name.is_empty()
            || manifest.name.len() > 128
            || manifest.version.is_empty()
            || manifest.version.len() > 64
            || manifest.description.len() > 4096
            || manifest.skills.len() > 64
            || manifest.mcp_servers.len() > 32
        {
            return Err(ExtensionError::Invalid("invalid plugin manifest".into()));
        }
        if files.len() != manifest.skills.len()
            || files.values().map(String::len).sum::<usize>() > 4 * 1024 * 1024
        {
            return Err(ExtensionError::Invalid(
                "plugin files must exactly match declared skills within size limits".into(),
            ));
        }
        let mut state = lock(&self.state)?;
        if state.plugins.contains_key(&manifest.id) {
            return Err(ExtensionError::Conflict(
                "plugin is installed; remove before replacing".into(),
            ));
        }
        let destination = safe_path(&self.paths.plugins(), &manifest.id)?;
        if destination.exists() {
            return Err(ExtensionError::Conflict(
                "plugin directory already exists".into(),
            ));
        }
        let mut next = state.clone();
        let package_digest = hash_json(&(&manifest, &files))?;
        let mut writes = Vec::new();
        for skill in &manifest.skills {
            if !skill.path.ends_with("/SKILL.md") {
                return Err(ExtensionError::Invalid(
                    "plugin skill paths must end in /SKILL.md".into(),
                ));
            }
            let content = files
                .get(&skill.path)
                .ok_or_else(|| ExtensionError::Invalid("declared skill file missing".into()))?;
            let (name, description, user, model) = skills::parse(content)?;
            let id = format!("{}--{name}", manifest.id);
            valid_id(&id)?;
            if next.skills.contains_key(&id) {
                return Err(ExtensionError::Conflict("duplicate plugin skill".into()));
            }
            let relative = format!("{}/{}", manifest.id, skill.path);
            let path = safe_path(&self.paths.plugins(), &relative)?;
            let summary = SkillSummary {
                id: id.clone(),
                name,
                description,
                enabled: true,
                trusted: manifest.trusted,
                user_invocable: user,
                model_invocable: model,
                workspace_ids: manifest.workspace_ids.clone(),
                profiles: manifest.profiles.clone(),
                digest: digest(content.as_bytes()),
                plugin_id: Some(manifest.id.clone()),
            };
            next.skills.insert(
                id,
                SkillRecord {
                    summary,
                    path: relative,
                },
            );
            writes.push((path, content.as_bytes().to_vec()));
        }
        for server in &manifest.mcp_servers {
            validate_server(server)?;
            if next.servers.contains_key(&server.id) {
                return Err(ExtensionError::Conflict("duplicate MCP id".into()));
            }
            next.servers.insert(
                server.id.clone(),
                ServerRecord {
                    config: server.clone(),
                    plugin_id: Some(manifest.id.clone()),
                },
            );
        }
        if next.skills.len() > 1024 || next.servers.len() > 128 || next.plugins.len() >= 128 {
            return Err(ExtensionError::TooLarge);
        }
        let manifest_bytes =
            serde_json::to_vec_pretty(&manifest).map_err(|_| ExtensionError::Io)?;
        let summary = PluginSummary {
            manifest: manifest.clone(),
            digest: package_digest,
            manifest_digest: digest(&manifest_bytes),
        };
        next.plugins.insert(manifest.id.clone(), summary.clone());
        // All validation precedes materialization. The registry commit makes the
        // complete package active; failed writes remain an inactive package.
        let result = (|| {
            for (path, bytes) in writes {
                atomic(&path, &bytes)?;
            }
            atomic(
                &safe_path(
                    &self.paths.plugins(),
                    &format!("{}/manifest.json", manifest.id),
                )?,
                &manifest_bytes,
            )?;
            self.persist(&next)
        })();
        if result.is_err() {
            let owned = next
                .skills
                .values()
                .filter(|s| s.summary.plugin_id.as_deref() == Some(&manifest.id))
                .map(|s| (s.path.clone(), s.summary.digest.clone()))
                .chain(std::iter::once((
                    format!("{}/manifest.json", manifest.id),
                    summary.manifest_digest.clone(),
                )))
                .collect::<Vec<_>>();
            let _ = self.cleanup_plugin(&manifest.id, &owned);
        }
        result?;
        *state = next;
        Ok(summary)
    }
    pub fn set_plugin_enabled(&self, id: &str, enabled: bool) -> Result<PluginSummary> {
        let mut state = lock(&self.state)?;
        let mut next = state.clone();
        let plugin = next
            .plugins
            .get_mut(id)
            .ok_or_else(|| ExtensionError::Unavailable("plugin is not installed".into()))?;
        plugin.manifest.enabled = enabled;
        let result = plugin.clone();
        self.persist(&next)?;
        *state = next;
        drop(state);
        self.disconnect_plugin(id)?;
        Ok(result)
    }
    pub fn remove_plugin(&self, id: &str) -> Result<bool> {
        let mut state = lock(&self.state)?;
        if !state.plugins.contains_key(id) {
            return Ok(false);
        }
        safe_path(&self.paths.plugins(), id)?;
        let plugin = state.plugins.get(id).unwrap();
        let owned = state
            .skills
            .values()
            .filter(|s| s.summary.plugin_id.as_deref() == Some(id))
            .map(|s| (s.path.clone(), s.summary.digest.clone()))
            .chain(std::iter::once((
                format!("{id}/manifest.json"),
                plugin.manifest_digest.clone(),
            )))
            .collect::<Vec<_>>();
        let servers = state
            .servers
            .iter()
            .filter(|(_, s)| s.plugin_id.as_deref() == Some(id))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        let mut next = state.clone();
        next.plugins.remove(id);
        next.skills
            .retain(|_, s| s.summary.plugin_id.as_deref() != Some(id));
        next.servers
            .retain(|_, s| s.plugin_id.as_deref() != Some(id));
        self.persist(&next)?;
        *state = next;
        drop(state);
        for server in servers {
            self.disconnect_mcp(&server)?;
        }
        self.cleanup_plugin(id, &owned)?;
        Ok(true)
    }
    fn cleanup_plugin(&self, id: &str, owned: &[(String, String)]) -> Result<()> {
        let package = safe_path(&self.paths.plugins(), id)?;
        for (relative, expected) in owned {
            let path = safe_path(&self.paths.plugins(), relative)?;
            if !path.starts_with(&package) {
                return Err(ExtensionError::Forbidden(
                    "plugin cleanup path mismatch".into(),
                ));
            }
            if path.exists() {
                let content = read(&path, MAX_SKILL)?;
                if digest(content.as_bytes()) != *expected {
                    continue;
                }
                fs::remove_file(&path)?;
            }
            let mut parent = path.parent();
            while let Some(directory) = parent {
                if directory == package || !directory.starts_with(&package) {
                    break;
                }
                if fs::remove_dir(directory).is_err() {
                    break;
                }
                parent = directory.parent();
            }
        }
        // Empty-directory removal preserves every unknown or user-modified file.
        let _ = fs::remove_dir(package);
        Ok(())
    }
    fn disconnect_plugin(&self, id: &str) -> Result<()> {
        let servers = lock(&self.state)?
            .servers
            .iter()
            .filter(|(_, s)| s.plugin_id.as_deref() == Some(id))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for server in servers {
            self.disconnect_mcp(&server)?;
        }
        Ok(())
    }
}
