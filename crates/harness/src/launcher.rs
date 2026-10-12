//! Sandbox-backed process trees; production never launches directly.
use aidex_sandbox::{
    CommandSpec, Mount, NetworkMode, ResourceLimits, Sandbox, SandboxLevel, SandboxRequest,
};
use aidex_tools::{
    CancelToken, LaunchError, LaunchOutcome, LaunchRequest, PlanFingerprint, ProcessLauncher,
};
use std::{path::PathBuf, time::Duration};
pub(crate) struct NativeLauncher {
    sandbox: Sandbox,
    scratch: PathBuf,
    fingerprint: PlanFingerprint,
    level: SandboxLevel,
}
impl NativeLauncher {
    pub fn new(
        sandbox: Sandbox,
        scratch: PathBuf,
        fingerprint: PlanFingerprint,
        level: SandboxLevel,
    ) -> Self {
        Self {
            sandbox,
            scratch,
            fingerprint,
            level,
        }
    }
}
impl ProcessLauncher for NativeLauncher {
    fn launch(
        &self,
        r: &LaunchRequest,
        cancel: &CancelToken,
    ) -> Result<LaunchOutcome, LaunchError> {
        if r.plan_fingerprint() != self.fingerprint {
            return Err(LaunchError::Denied(
                "process plan fingerprint mismatch".into(),
            ));
        }
        let scratch = self.scratch.join(r.call_id().as_str());
        std::fs::create_dir_all(&scratch).map_err(|e| LaunchError::Io(e.to_string()))?;
        let mut request = SandboxRequest::new(self.level, r.cwd(), &scratch)
            .with_mount(Mount {
                host_path: r.workspace_root().clone(),
                access: r.workspace_access(),
            })
            .with_network(if self.level == SandboxLevel::Brokered {
                NetworkMode::Unrestricted
            } else {
                NetworkMode::Offline
            });
        request.limits = ResourceLimits {
            max_processes: Some(64),
            max_memory_bytes: None,
            max_cpu_time: None,
            wall_timeout: Some(r.timeout()),
            max_output_bytes: r.max_output_bytes(),
        };
        request.env = r.env().to_vec();
        let plan = self
            .sandbox
            .negotiate(&request)
            .map_err(LaunchError::Sandbox)?;
        let program = plan
            .resolve_program(r.program())
            .map_err(LaunchError::Sandbox)?;
        let process = self
            .sandbox
            .spawn(
                &plan,
                CommandSpec {
                    program,
                    args: r.args().to_vec(),
                    stdin: r.stdin().map(|b| b.to_vec()),
                },
            )
            .map_err(LaunchError::Sandbox)?;
        loop {
            if cancel.is_cancelled() {
                process.kill_tree().map_err(LaunchError::Sandbox)?;
            }
            if let Some(status) = process
                .wait_timeout(Duration::from_millis(20))
                .map_err(LaunchError::Sandbox)?
            {
                return Ok(LaunchOutcome {
                    process_id: Some(process.id().clone()),
                    sandbox_level: self.level,
                    status,
                    output: process.take_output(),
                });
            }
        }
    }
}
