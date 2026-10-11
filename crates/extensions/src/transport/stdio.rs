use super::{ExtensionSecrets, protect_frame, protocol::Wire};
use crate::{CallScope, ExtensionError, McpServerConfig, McpTransport, Result};
use aidex_sandbox::{
    CommandSpec, ManagedProcess, Mount, NetworkMode, ResourceLimits, Sandbox, SandboxLevel,
    SandboxRequest,
};
use aidex_tools::CancelToken;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) struct StdioWire {
    process: ManagedProcess,
    buffer: Vec<u8>,
    timeout: Duration,
    max: usize,
    secrets: Vec<aidex_vault::SecretBytes>,
    scratch: PathBuf,
    gate: Arc<dyn Fn() -> bool + Send + Sync>,
}
pub(super) struct LaunchPaths<'a> {
    pub scratch_root: &'a Path,
    pub drive_root: Option<&'a Path>,
}
impl StdioWire {
    pub fn start(
        sandbox: &Sandbox,
        paths: LaunchPaths<'_>,
        config: &McpServerConfig,
        scope: &CallScope,
        secrets: Arc<dyn ExtensionSecrets>,
        cancel: &CancelToken,
        gate: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Self> {
        let LaunchPaths {
            scratch_root,
            drive_root,
        } = paths;
        if !scope.network_allowed || !gate() {
            return Err(ExtensionError::Forbidden(
                "stdio denied because network confinement is required".into(),
            ));
        }
        if cfg!(windows) && !scope.full_access {
            return Err(ExtensionError::Forbidden(
                "Windows stdio requires explicit full_access".into(),
            ));
        }
        let McpTransport::Stdio { command, args, cwd } = &config.transport else {
            return Err(ExtensionError::Invalid("not stdio".into()));
        };
        let command = crate::portable::resolve(drive_root, command)?;
        let args = args
            .iter()
            .map(|arg| crate::portable::resolve(drive_root, arg))
            .collect::<Result<Vec<_>>>()?;
        let cwd = crate::portable::cwd(drive_root, &scope.workspace_root, cwd.as_deref())?;
        let cwd = fs::canonicalize(cwd)?;
        let root = fs::canonicalize(&scope.workspace_root)?;
        if !cwd.starts_with(&root) {
            return Err(ExtensionError::Forbidden(
                "MCP working directory escapes workspace".into(),
            ));
        }
        let id = aidex_core::EntityId::generate("mcp").map_err(|_| ExtensionError::Io)?;
        fs::create_dir_all(scratch_root)?;
        let scratch = crate::storage::safe_path(scratch_root, id.as_str())?;
        fs::create_dir(&scratch)?;
        let level = if scope.full_access {
            SandboxLevel::Brokered
        } else {
            SandboxLevel::Restricted
        };
        let mut request = SandboxRequest::new(level, &cwd, &scratch)
            .with_mount(Mount::read_write(&root))
            .with_network(NetworkMode::Normal)
            .with_limits(ResourceLimits {
                wall_timeout: Some(Duration::from_secs(3600)),
                max_output_bytes: config.max_output_bytes,
                max_processes: Some(16),
                ..ResourceLimits::default()
            });
        let mut protected = Vec::new();
        for (name, reference) in &config.secret_refs {
            if cancel.is_cancelled() {
                return Err(ExtensionError::Cancelled);
            }
            if !gate() {
                return Err(ExtensionError::Forbidden("network access is denied".into()));
            }
            let value = secrets.resolve(&config.id, reference)?;
            let text = std::str::from_utf8(value.expose())
                .map_err(|_| ExtensionError::Invalid("credential is not UTF-8".into()))?;
            if text.contains('\0') {
                return Err(ExtensionError::Invalid("credential contains NUL".into()));
            }
            request = request.with_env(name, text);
            protected.push(value);
        }
        if cancel.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        if !gate() {
            return Err(ExtensionError::Forbidden("network access is denied".into()));
        }
        let plan = sandbox.negotiate(&request).map_err(|_| {
            ExtensionError::Unavailable(
                "requested MCP process confinement cannot be enforced".into(),
            )
        })?;
        let program = plan
            .resolve_program(&command)
            .map_err(|_| ExtensionError::Unavailable("MCP command cannot be resolved".into()))?;
        if cancel.is_cancelled() || !gate() {
            return Err(ExtensionError::Cancelled);
        }
        let process = sandbox
            .spawn_interactive(&plan, CommandSpec::new(program).args(args.clone()))
            .map_err(|_| {
                ExtensionError::Unavailable("MCP sandbox process could not start".into())
            })?;
        if cancel.is_cancelled() || !gate() {
            let _ = process.kill_tree();
            return Err(ExtensionError::Cancelled);
        }
        Ok(Self {
            process,
            buffer: Vec::new(),
            timeout: Duration::from_millis(config.timeout_ms),
            max: config.max_output_bytes,
            secrets: protected,
            scratch,
            gate,
        })
    }
    fn send(&self, message: &Value) -> Result<()> {
        if !(self.gate)() {
            return Err(ExtensionError::Forbidden("network access is denied".into()));
        }
        let mut bytes = serde_json::to_vec(message)
            .map_err(|_| ExtensionError::Protocol("cannot encode JSON-RPC".into()))?;
        if bytes.len() > self.max {
            return Err(ExtensionError::TooLarge);
        }
        bytes.push(b'\n');
        self.process
            .write_stdin(&bytes)
            .map_err(|_| ExtensionError::Unavailable("MCP process input closed".into()))
    }
}
impl Wire for StdioWire {
    fn exchange(&mut self, message: Value, cancel: &CancelToken) -> Result<Option<Value>> {
        if cancel.is_cancelled() {
            return Err(ExtensionError::Cancelled);
        }
        if !(self.gate)() {
            let _ = self.process.kill_tree();
            return Err(ExtensionError::Forbidden("network access is denied".into()));
        }
        self.send(&message)?;
        let Some(id) = message.get("id") else {
            return Ok(None);
        };
        let start = Instant::now();
        let mut frames = 0;
        loop {
            if !(self.gate)() {
                let _ = self.process.kill_tree();
                return Err(ExtensionError::Forbidden("network access is denied".into()));
            }
            if cancel.is_cancelled() || start.elapsed() > self.timeout {
                let _=self.send(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id,"reason":"client stopped waiting"}}));
                let _ = self.process.kill_tree();
                return Err(if cancel.is_cancelled() {
                    ExtensionError::Cancelled
                } else {
                    ExtensionError::Timeout
                });
            }
            let output = self.process.take_output();
            if output.stdout.truncated || output.stdout.dropped_bytes > 0 {
                return Err(ExtensionError::TooLarge);
            }
            self.buffer.extend(output.stdout.bytes);
            if self.buffer.len() > self.max {
                return Err(ExtensionError::TooLarge);
            }
            while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
                let bytes = self.buffer.drain(..=end).collect::<Vec<_>>();
                let mut frame: Value = serde_json::from_slice(&bytes).map_err(|_| {
                    ExtensionError::Protocol("MCP stdout is not newline-delimited JSON-RPC".into())
                })?;
                frames += 1;
                if frames > 256 {
                    return Err(ExtensionError::TooLarge);
                }
                if frame.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    return Err(ExtensionError::Protocol("invalid MCP frame".into()));
                }
                if frame.get("id") == Some(id) && frame.get("method").is_none() {
                    protect_frame(
                        &mut frame,
                        &self.secrets,
                        message.get("method").and_then(Value::as_str),
                    )?;
                    return Ok(Some(frame));
                }
                if frame.get("method").and_then(Value::as_str)
                    == Some("notifications/tools/list_changed")
                {
                    return Err(ExtensionError::Conflict(
                        "server tool catalog changed; rediscover before further calls".into(),
                    ));
                }
                if let Some(request_id) = frame.get("id") {
                    self.send(&json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":"client capability not supported"}}))?
                }
            }
            if self
                .process
                .try_wait()
                .map_err(|_| ExtensionError::Unavailable("MCP process status unavailable".into()))?
                .is_some()
            {
                return Err(ExtensionError::Unavailable("MCP process exited".into()));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn disconnect(&mut self) {
        self.process.close_stdin();
        if self
            .process
            .wait_timeout(Duration::from_millis(100))
            .ok()
            .flatten()
            .is_none()
        {
            let _ = self.process.kill_tree();
        }
    }
}
impl Drop for StdioWire {
    fn drop(&mut self) {
        self.disconnect();
        let _ = fs::remove_dir_all(&self.scratch);
    }
}
