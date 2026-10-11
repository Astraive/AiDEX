use crate::{
    model::ServerRecord,
    service::{Connected, hash_json, lock, validate_server},
    storage::{MAX_SKILL, digest, grants, read},
    *,
};
use std::sync::{Arc, atomic::Ordering};
impl ExtensionService {
    pub fn upsert_mcp(&self, mut config: McpServerConfig) -> Result<McpServerSummary> {
        crate::portable::normalize(self.paths.root(), &mut config)?;
        validate_server(&config)?;
        let mut state = lock(&self.state)?;
        if state
            .servers
            .get(&config.id)
            .is_some_and(|s| s.plugin_id.is_some())
        {
            return Err(ExtensionError::Forbidden(
                "plugin MCP definitions are changed through the plugin".into(),
            ));
        }
        if state.servers.len() >= 128 && !state.servers.contains_key(&config.id) {
            return Err(ExtensionError::TooLarge);
        }
        let mut next = state.clone();
        next.servers.insert(
            config.id.clone(),
            ServerRecord {
                config: config.clone(),
                plugin_id: None,
            },
        );
        self.persist(&next)?;
        *state = next;
        drop(state);
        self.disconnect_mcp(&config.id)?;
        self.server_summary(&config.id)
    }
    pub fn remove_mcp(&self, id: &str) -> Result<bool> {
        let mut state = lock(&self.state)?;
        if state.servers.get(id).is_some_and(|s| s.plugin_id.is_some()) {
            return Err(ExtensionError::Forbidden(
                "remove the contributing plugin".into(),
            ));
        }
        let mut next = state.clone();
        if next.servers.remove(id).is_none() {
            return Ok(false);
        }
        self.persist(&next)?;
        *state = next;
        drop(state);
        self.disconnect_mcp(id)?;
        Ok(true)
    }
    pub fn list_mcp(&self) -> Result<Vec<McpServerSummary>> {
        let ids = lock(&self.state)?
            .servers
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        ids.iter().map(|id| self.server_summary(id)).collect()
    }
    pub fn server_summary(&self, id: &str) -> Result<McpServerSummary> {
        let server =
            lock(&self.state)?.servers.get(id).cloned().ok_or_else(|| {
                ExtensionError::Unavailable("MCP server is not registered".into())
            })?;
        let connection = lock(&self.connected)?.get(id).cloned();
        let owner = connection
            .as_ref()
            .map(|c| lock(&c.owner_session).map(|owner| owner.clone()))
            .transpose()?
            .flatten();
        Ok(McpServerSummary {
            config: server.config.clone(),
            connected: connection.is_some(),
            connecting: lock(&self.pending_connect)?.contains_key(id),
            catalog_revision: connection
                .as_ref()
                .map(|c| c.revision.clone())
                .unwrap_or_default(),
            tools: connection
                .as_ref()
                .map(|c| c.tools.clone())
                .unwrap_or_default(),
            last_error: lock(&self.connect_errors)?.get(id).cloned(),
            plugin_id: server.plugin_id.clone(),
            owner_session_id: owner,
            call_tool_name: if matches!(server.config.transport, McpTransport::Stdio { .. }) {
                "mcp_stdio_call".into()
            } else {
                "mcp_call".into()
            },
        })
    }
    pub(crate) fn allowed_server(&self, id: &str, scope: &CallScope) -> Result<ServerRecord> {
        if self.stopped.load(Ordering::SeqCst) {
            return Err(ExtensionError::Unavailable(
                "extension service has stopped".into(),
            ));
        }
        if !self.secrets.available() {
            return Err(ExtensionError::Forbidden(
                "unlock the vault before connecting or calling MCP".into(),
            ));
        }
        let state = lock(&self.state)?;
        let server = state
            .servers
            .get(id)
            .ok_or_else(|| ExtensionError::Unavailable("MCP server is not registered".into()))?;
        let c = &server.config;
        if !c.trusted
            || !c.enabled
            || !grants(&c.workspace_ids, &scope.workspace_id)
            || !grants(&c.profiles, &scope.profile)
        {
            return Err(ExtensionError::Forbidden(
                "server is disabled, untrusted or outside configured scope".into(),
            ));
        }
        if !scope.network_allowed || !(self.network_gate)() {
            return Err(ExtensionError::Forbidden("network access is denied".into()));
        }
        if matches!(c.transport, McpTransport::Stdio { .. }) && cfg!(windows) && !scope.full_access
        {
            return Err(ExtensionError::Forbidden(
                "stdio requires explicit full_access on Windows".into(),
            ));
        }
        if let Some(plugin) = &server.plugin_id {
            let p = state
                .plugins
                .get(plugin)
                .ok_or_else(|| ExtensionError::Forbidden("plugin removed".into()))?;
            if !p.manifest.enabled
                || !p.manifest.trusted
                || !grants(&p.manifest.workspace_ids, &scope.workspace_id)
                || !grants(&p.manifest.profiles, &scope.profile)
            {
                return Err(ExtensionError::Forbidden("plugin grant denied".into()));
            }
            self.verify_plugin(p)?;
        }
        Ok(server.clone())
    }
    pub fn disconnect_mcp(&self, id: &str) -> Result<()> {
        if let Some(job) = lock(&self.pending_connect)?.remove(id) {
            job.cancel.cancel();
            lock(&self.connect_errors)?.insert(id.into(), "MCP connection cancelled".into());
        }
        self.disconnect_active(id)
    }
    pub(crate) fn disconnect_active(&self, id: &str) -> Result<()> {
        let removed = { lock(&self.connected)?.remove(id) };
        if let Some(connection) = removed {
            if let Some(cancel) = lock(&connection.active_cancel)?.as_ref() {
                cancel.cancel();
            }
            lock(&connection.connection)?.disconnect();
        }
        Ok(())
    }
    pub fn disconnect_all(&self) {
        if let Ok(mut pending) = self.pending_connect.lock() {
            for (id, job) in std::mem::take(&mut *pending) {
                job.cancel.cancel();
                if let Ok(mut errors) = self.connect_errors.lock() {
                    errors.insert(id, "MCP connection cancelled".into());
                }
            }
        }
        let connections = self
            .connected
            .lock()
            .ok()
            .map(|mut connections| std::mem::take(&mut *connections));
        if let Some(connections) = connections {
            for (_, connection) in connections {
                if let Ok(cancel) = connection.active_cancel.lock()
                    && let Some(cancel) = cancel.as_ref()
                {
                    cancel.cancel();
                }
                if let Ok(mut connection) = connection.connection.lock() {
                    connection.disconnect();
                }
            }
        }
    }
    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.disconnect_all();
    }
    pub fn catalog_for(&self, scope: &CallScope) -> Result<CatalogSnapshot> {
        let state = lock(&self.state)?;
        let mut skills = Vec::new();
        for record in state.skills.values() {
            let summary = &record.summary;
            if !summary.trusted
                || !summary.enabled
                || !grants(&summary.workspace_ids, &scope.workspace_id)
                || !grants(&summary.profiles, &scope.profile)
            {
                continue;
            }
            if let Some(plugin) = &summary.plugin_id {
                let Some(p) = state.plugins.get(plugin) else {
                    continue;
                };
                if !p.manifest.trusted || !p.manifest.enabled || self.verify_plugin(p).is_err() {
                    continue;
                }
            }
            let content = read(&self.skill_path(record)?, MAX_SKILL)?;
            if digest(content.as_bytes()) != summary.digest {
                continue;
            }
            skills.push(skills::refresh(summary.clone(), &content)?);
        }
        let ids = state.servers.keys().cloned().collect::<Vec<_>>();
        drop(state);
        let mut servers = Vec::new();
        for id in ids {
            if self.allowed_server(&id, scope).is_ok() {
                let summary = self.server_summary(&id)?;
                if let Some(connection) = lock(&self.connected)?.get(&id)
                    && !connection.scope.same_mcp_scope(scope)
                {
                    continue;
                }
                servers.push(summary)
            }
        }
        let revision = hash_json(&(&skills, &servers))?;
        Ok(CatalogSnapshot {
            revision,
            skills,
            mcp_servers: servers,
        })
    }
    pub fn skill_context(&self, scope: &CallScope) -> Result<String> {
        let catalog = self.catalog_for(scope)?;
        let mut result = String::new();
        for s in catalog.skills {
            if scope.can_read_skill(&s) {
                let line = format!(
                    "- {}: {} (load with skill_read, id {})\n",
                    s.name, s.description, s.id
                );
                if result.len() + line.len() > 32768 {
                    break;
                }
                result.push_str(&line);
            }
        }
        Ok(result)
    }
    pub(crate) fn selected_connection(
        &self,
        id: &str,
        revision: &str,
        scope: &CallScope,
    ) -> Result<(ServerRecord, Arc<Connected>)> {
        let server = self.allowed_server(id, scope)?;
        let connection = lock(&self.connected)?.get(id).cloned().ok_or_else(|| {
            ExtensionError::Unavailable("connect and discover this server explicitly first".into())
        })?;
        if !connection.scope.same_mcp_scope(scope)
            || connection.revision != revision
            || connection.config_digest != hash_json(&server)?
        {
            return Err(ExtensionError::Conflict(
                "MCP scope or catalog changed".into(),
            ));
        }
        Ok((server, connection))
    }
    pub(crate) fn credential_versions(
        &self,
        server: &McpServerConfig,
    ) -> Result<std::collections::BTreeMap<String, aidex_vault::SecretVersion>> {
        server
            .secret_refs
            .values()
            .map(|name| Ok((name.clone(), self.secrets.version(&server.id, name)?)))
            .collect()
    }
    pub(crate) fn check_credentials(
        &self,
        server: &McpServerConfig,
        versions: &std::collections::BTreeMap<String, aidex_vault::SecretVersion>,
    ) -> Result<Vec<aidex_vault::SecretBytes>> {
        if versions.len()
            != server
                .secret_refs
                .values()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        {
            return Err(ExtensionError::Conflict(
                "MCP credential bindings changed".into(),
            ));
        }
        server
            .secret_refs
            .values()
            .map(|name| {
                self.secrets.resolve_versioned(
                    &server.id,
                    name,
                    versions.get(name).ok_or_else(|| {
                        ExtensionError::Conflict("MCP credential binding absent".into())
                    })?,
                )
            })
            .collect()
    }
    pub(crate) fn check_owner(
        &self,
        connection: &Connected,
        session_id: &str,
        claim: bool,
    ) -> Result<()> {
        let mut owner = lock(&connection.owner_session)?;
        if owner.as_ref().is_some_and(|owner| owner != session_id) {
            return Err(ExtensionError::Forbidden(
                "MCP connection belongs to another session; reconnect explicitly before switching"
                    .into(),
            ));
        }
        if claim && owner.is_none() {
            *owner = Some(session_id.into());
        }
        Ok(())
    }
}
