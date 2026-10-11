//! Explicit MCP connection jobs and their publication/cancellation boundary.
use crate::{
    service::{Connected, PendingConnect, hash_json, lock},
    transport::{BoundSecrets, reflected},
    *,
};
use aidex_tools::CancelToken;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
struct JobLease(Arc<AtomicUsize>);
impl Drop for JobLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
fn reserve(budget: &Arc<AtomicUsize>) -> Result<JobLease> {
    let mut count = budget.load(Ordering::SeqCst);
    loop {
        if count >= 16 {
            return Err(ExtensionError::TooLarge);
        }
        match budget.compare_exchange(count, count + 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return Ok(JobLease(budget.clone())),
            Err(current) => count = current,
        }
    }
}

impl ExtensionService {
    fn begin_connect(&self, id: &str, scope: &CallScope, cancel: &CancelToken) -> Result<String> {
        if self.stopped.load(Ordering::SeqCst) || cancel.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        self.allowed_server(id, scope)?;
        let generation = aidex_core::EntityId::generate("connect")
            .map_err(|_| ExtensionError::Io)?
            .to_string();
        let mut pending = lock(&self.pending_connect)?;
        if self.stopped.load(Ordering::SeqCst) || cancel.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        if pending.len() >= 16 && !pending.contains_key(id) {
            return Err(ExtensionError::TooLarge);
        }
        if let Some(old) = pending.insert(
            id.into(),
            PendingConnect {
                generation: generation.clone(),
                cancel: cancel.clone(),
            },
        ) {
            old.cancel.cancel();
        }
        lock(&self.connect_errors)?.remove(id);
        Ok(generation)
    }
    fn finish_connect(&self, id: &str, generation: &str, result: &Result<()>) {
        if let Ok(mut pending) = self.pending_connect.lock()
            && pending
                .get(id)
                .is_some_and(|job| job.generation == generation)
        {
            pending.remove(id);
            if let Ok(mut errors) = self.connect_errors.lock() {
                if let Err(error) = result {
                    errors.insert(id.into(), job_error(error).into());
                } else {
                    errors.remove(id);
                }
            }
        }
    }
    pub fn connect_mcp(
        &self,
        id: &str,
        scope: &CallScope,
        cancel: &CancelToken,
    ) -> Result<McpServerSummary> {
        let generation = self.begin_connect(id, scope, cancel)?;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.connect_registered(id, scope, cancel, &generation)
        }))
        .unwrap_or_else(|_| {
            Err(ExtensionError::Unavailable(
                "MCP connection job failed".into(),
            ))
        });
        self.finish_connect(id, &generation, &result);
        result?;
        self.server_summary(id)
    }
    /// User-originated job; deliberately survives the initiating IPC response.
    pub fn connect_mcp_async(
        self: &Arc<Self>,
        id: &str,
        scope: &CallScope,
    ) -> Result<McpServerSummary> {
        let lease = reserve(&self.connect_jobs)?;
        let cancel = CancelToken::new();
        let generation = self.begin_connect(id, scope, &cancel)?;
        let service = self.clone();
        let worker_id = id.to_owned();
        let worker_scope = scope.clone();
        let worker_generation = generation.clone();
        let start = std::thread::Builder::new()
            .name("aidex-mcp-connect".into())
            .spawn(move || {
                let _lease = lease;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    service.connect_registered(
                        &worker_id,
                        &worker_scope,
                        &cancel,
                        &worker_generation,
                    )
                }))
                .unwrap_or_else(|_| {
                    Err(ExtensionError::Unavailable(
                        "MCP connection job failed".into(),
                    ))
                });
                service.finish_connect(&worker_id, &worker_generation, &result);
            });
        if start.is_err() {
            let error = Err(ExtensionError::Unavailable(
                "cannot start MCP connection job".into(),
            ));
            self.finish_connect(id, &generation, &error);
            return Err(ExtensionError::Unavailable(
                "cannot start MCP connection job".into(),
            ));
        }
        self.server_summary(id)
    }
    pub fn discover_mcp(
        &self,
        id: &str,
        scope: &CallScope,
        cancel: &CancelToken,
    ) -> Result<McpServerSummary> {
        self.connect_mcp(id, scope, cancel)
    }
    pub fn discover_mcp_async(
        self: &Arc<Self>,
        id: &str,
        scope: &CallScope,
    ) -> Result<McpServerSummary> {
        self.connect_mcp_async(id, scope)
    }
    fn connect_registered(
        &self,
        id: &str,
        scope: &CallScope,
        cancel: &CancelToken,
        generation: &str,
    ) -> Result<()> {
        let _lifecycle = lock(&self.lifecycle)?;
        self.check_connect(id, scope, cancel, generation)?;
        self.disconnect_active(id)?;
        let server = self.allowed_server(id, scope)?;
        if lock(&self.connected)?.len() >= 16 {
            return Err(ExtensionError::TooLarge);
        }
        let config_digest = hash_json(&server)?;
        let credential_versions = self.credential_versions(&server.config)?;
        let protected = self.check_credentials(&server.config, &credential_versions)?;
        self.check_connect(id, scope, cancel, generation)?;
        let secrets = Arc::new(BoundSecrets {
            source: self.secrets.clone(),
            server_id: id.into(),
            versions: credential_versions.clone(),
        });
        let mut connection = self
            .connector
            .connect(&server.config, scope, secrets, cancel)?;
        let result = (|| {
            self.check_connect(id, scope, cancel, generation)?;
            connection.initialize(cancel)?;
            self.check_connect(id, scope, cancel, generation)?;
            let tools = connection.list_tools(cancel)?;
            self.check_connect(id, scope, cancel, generation)?;
            for tool in &tools {
                let value = serde_json::to_value(tool)
                    .map_err(|_| ExtensionError::Protocol("invalid tool metadata".into()))?;
                if reflected(&value, &protected) {
                    return Err(ExtensionError::Protocol(
                        "server reflected a credential in metadata".into(),
                    ));
                }
                crate::schema::check(&tool.input_schema)?;
                if let Some(output) = &tool.output_schema {
                    crate::schema::check(output)?;
                }
            }
            if self.credential_versions(&server.config)? != credential_versions
                || hash_json(&self.allowed_server(id, scope)?)? != config_digest
            {
                return Err(ExtensionError::Conflict(
                    "server definition or credentials changed during connection".into(),
                ));
            }
            let revision = hash_json(&(
                &config_digest,
                &tools,
                &credential_versions,
                aidex_core::EntityId::generate("catalog").map_err(|_| ExtensionError::Io)?,
            ))?;
            Ok((tools, revision))
        })();
        let (tools, revision) = match result {
            Ok(value) => value,
            Err(error) => {
                connection.disconnect();
                return Err(error);
            }
        };
        // Hold the generation latch while publishing; cancellation/shutdown removes it first.
        let pending = lock(&self.pending_connect)?;
        if self.stopped.load(Ordering::SeqCst)
            || cancel.is_cancelled()
            || !(self.network_gate)()
            || !pending
                .get(id)
                .is_some_and(|job| job.generation == generation && !job.cancel.is_cancelled())
        {
            connection.disconnect();
            return Err(ExtensionError::Cancelled);
        }
        lock(&self.connected)?.insert(
            id.into(),
            Arc::new(Connected {
                scope: scope.mcp_scope(),
                config_digest,
                revision,
                tools,
                credential_versions,
                connection: Mutex::new(connection),
                active_cancel: Mutex::new(None),
                owner_session: Mutex::new(None),
            }),
        );
        Ok(())
    }
    fn check_connect(
        &self,
        id: &str,
        scope: &CallScope,
        cancel: &CancelToken,
        generation: &str,
    ) -> Result<()> {
        if self.stopped.load(Ordering::SeqCst)
            || cancel.is_cancelled()
            || !lock(&self.pending_connect)?
                .get(id)
                .is_some_and(|job| job.generation == generation && !job.cancel.is_cancelled())
        {
            return Err(ExtensionError::Cancelled);
        }
        self.allowed_server(id, scope)?;
        Ok(())
    }
}
fn job_error(error: &ExtensionError) -> &'static str {
    match error {
        ExtensionError::Invalid(_) => "MCP configuration is invalid",
        ExtensionError::Forbidden(_) => "MCP access denied; check trust, scope, network and vault",
        ExtensionError::Conflict(_) => {
            "MCP configuration or credentials changed; reconnect explicitly"
        }
        ExtensionError::Protocol(_) => "MCP server returned an unsupported response",
        ExtensionError::Cancelled => "MCP connection cancelled",
        ExtensionError::Timeout => "MCP connection timed out",
        ExtensionError::TooLarge => "MCP connection exceeded a configured limit",
        _ => "MCP connection unavailable",
    }
}
