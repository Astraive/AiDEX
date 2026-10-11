//! Sandbox-backed process trees; production never launches directly.
use aidex_events::{EventKind, EventScope, EventSink, ProcessStream};
use aidex_sandbox::{
    CommandSpec, Mount, NetworkMode, ResourceLimits, Sandbox, SandboxLevel, SandboxRequest,
};
use aidex_tools::{
    CancelToken, LaunchError, LaunchOutcome, LaunchRequest, PlanFingerprint, ProcessLauncher,
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
pub(crate) struct NativeLauncher {
    sandbox: Sandbox,
    scratch: PathBuf,
    fingerprint: PlanFingerprint,
    level: SandboxLevel,
    events: Arc<dyn EventSink>,
    scope: EventScope,
}
impl NativeLauncher {
    pub fn new(
        sandbox: Sandbox,
        scratch: PathBuf,
        fingerprint: PlanFingerprint,
        level: SandboxLevel,
        events: Arc<dyn EventSink>,
        scope: EventScope,
    ) -> Self {
        Self {
            sandbox,
            scratch,
            fingerprint,
            level,
            events,
            scope,
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
        let _ = self.events.emit(
            self.scope.clone(),
            EventKind::ProcessStarted {
                process_id: process.id().clone(),
                command_summary: "native tool process".into(),
                sandbox_level: match self.level {
                    SandboxLevel::Direct => aidex_events::SandboxLevel::Direct,
                    SandboxLevel::Brokered => aidex_events::SandboxLevel::Brokered,
                    SandboxLevel::Restricted => aidex_events::SandboxLevel::Restricted,
                    SandboxLevel::Ephemeral => aidex_events::SandboxLevel::Ephemeral,
                },
            },
        );
        let mut stdout = 0;
        let mut stderr = 0;
        let mut last_output = Instant::now() - Duration::from_millis(100);
        loop {
            if cancel.is_cancelled() {
                process.kill_tree().map_err(LaunchError::Sandbox)?;
            }
            if let Some(status) = process
                .wait_timeout(Duration::from_millis(20))
                .map_err(LaunchError::Sandbox)?
            {
                let output = process.take_output();
                self.stream_output(
                    process.id(),
                    ProcessStream::Stdout,
                    &output.stdout.bytes,
                    &mut stdout,
                    true,
                );
                self.stream_output(
                    process.id(),
                    ProcessStream::Stderr,
                    &output.stderr.bytes,
                    &mut stderr,
                    true,
                );
                let _ = self.events.emit(
                    self.scope.clone(),
                    EventKind::ProcessExited {
                        process_id: process.id().clone(),
                        exit_code: status.code,
                        signal: status.signal,
                    },
                );
                return Ok(LaunchOutcome {
                    process_id: Some(process.id().clone()),
                    sandbox_level: self.level,
                    status,
                    output,
                });
            }
            if last_output.elapsed() >= Duration::from_millis(100) {
                let output = process.output_so_far();
                self.stream_output(
                    process.id(),
                    ProcessStream::Stdout,
                    &output.stdout.bytes,
                    &mut stdout,
                    false,
                );
                self.stream_output(
                    process.id(),
                    ProcessStream::Stderr,
                    &output.stderr.bytes,
                    &mut stderr,
                    false,
                );
                last_output = Instant::now();
            }
        }
    }
}
impl NativeLauncher {
    fn stream_output(
        &self,
        process_id: &aidex_core::EntityId,
        stream: ProcessStream,
        bytes: &[u8],
        offset: &mut usize,
        flush: bool,
    ) {
        if *offset >= bytes.len() {
            return;
        }
        let pending = &bytes[*offset..];
        // Hold incomplete lines until a newline or exit, so credentials split
        // across pipe reads are redacted as one value before public streaming.
        let mut end = if flush {
            pending.len()
        } else {
            pending
                .iter()
                .rposition(|b| *b == b'\n')
                .map_or(0, |index| index + 1)
        };
        if !flush {
            const BEGIN: &[u8] = b"-----BEGIN ";
            let complete = &pending[..end];
            for (start, marker) in complete.windows(BEGIN.len()).enumerate() {
                if marker != BEGIN {
                    continue;
                }
                let label_start = start + BEGIN.len();
                let Some(label_length) = complete[label_start..]
                    .windows(5)
                    .position(|part| part == b"-----")
                else {
                    end = start;
                    break;
                };
                let label = &complete[label_start..label_start + label_length];
                if !label.windows(11).any(|part| part == b"PRIVATE KEY") {
                    continue;
                }
                let footer = [b"-----END ".as_slice(), label, b"-----"].concat();
                if !complete[label_start + label_length + 5..]
                    .windows(footer.len())
                    .any(|part| part == footer)
                {
                    end = start;
                    break;
                }
            }
        }
        let text = String::from_utf8_lossy(&pending[..end]);
        let mut remaining = aidex_events::redact_text(&text).into_owned();
        while !remaining.is_empty() {
            let chunk = crate::context::prefix(&remaining, 8192);
            remaining.drain(..chunk.len());
            let _ = self.events.emit(
                self.scope.clone(),
                EventKind::ProcessOutput {
                    process_id: process_id.clone(),
                    stream,
                    chunk,
                },
            );
        }
        *offset += end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_output_waits_for_complete_line_before_redacting_split_credentials() {
        let events = Arc::new(aidex_events::MemoryEventSink::new());
        let launcher = NativeLauncher::new(
            Sandbox::locate(),
            PathBuf::new(),
            PlanFingerprint::from_bytes([0; 32]),
            SandboxLevel::Brokered,
            events.clone(),
            EventScope::global(),
        );
        let id = aidex_core::EntityId::new("process_fixture").unwrap();
        let mut offset = 0;
        launcher.stream_output(
            &id,
            ProcessStream::Stdout,
            b"Authorization: Bearer fixture",
            &mut offset,
            false,
        );
        assert!(events.is_empty());
        launcher.stream_output(
            &id,
            ProcessStream::Stdout,
            b"Authorization: Bearer fixture-secret-value\n",
            &mut offset,
            false,
        );
        assert!(!events.is_empty());
        assert!(
            !serde_json::to_string(&events.events())
                .unwrap()
                .contains("fixture-secret-value")
        );
        assert_eq!(
            offset,
            b"Authorization: Bearer fixture-secret-value\n".len()
        );
    }
    #[test]
    fn live_output_keeps_private_key_blocks_together_across_lines() {
        let events = Arc::new(aidex_events::MemoryEventSink::new());
        let launcher = NativeLauncher::new(
            Sandbox::locate(),
            PathBuf::new(),
            PlanFingerprint::from_bytes([0; 32]),
            SandboxLevel::Brokered,
            events.clone(),
            EventScope::global(),
        );
        let id = aidex_core::EntityId::new("process_fixture").unwrap();
        let mut offset = 0;
        launcher.stream_output(
            &id,
            ProcessStream::Stdout,
            b"-----BEGIN PRIVATE KEY-----\n",
            &mut offset,
            false,
        );
        launcher.stream_output(
            &id,
            ProcessStream::Stdout,
            b"-----BEGIN PRIVATE KEY-----\ncHJpdmF0ZS1rZXktcGF5bG9hZA==\n",
            &mut offset,
            false,
        );
        assert!(
            !serde_json::to_string(&events.events())
                .unwrap()
                .contains("cHJpdmF0ZS1rZXktcGF5bG9hZA==")
        );
        launcher.stream_output(&id,ProcessStream::Stdout,b"-----BEGIN PRIVATE KEY-----\ncHJpdmF0ZS1rZXktcGF5bG9hZA==\n-----END PRIVATE KEY-----\n",&mut offset,false);
        assert!(!events.is_empty());
        assert!(
            !serde_json::to_string(&events.events())
                .unwrap()
                .contains("cHJpdmF0ZS1rZXktcGF5bG9hZA==")
        );
    }
}
