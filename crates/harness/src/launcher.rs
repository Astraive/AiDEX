//! Sandbox-backed process trees; production never launches directly.
//!
//! Each launch publishes `process.started`, live `process.output` chunks
//! about every 100 ms, and `process.exited` to the call's event sink. Output
//! is published only up to the last complete line, and never from the start
//! of a private-key block whose end line has not arrived, so a credential
//! split across pipe reads is redacted as one value. The complete output
//! stays in the tool result.
use aidex_core::EntityId;
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

/// How often captured output is published while a process runs.
const OUTPUT_INTERVAL: Duration = Duration::from_millis(100);
/// Largest `process.output` chunk, in bytes.
const MAX_CHUNK: usize = 8 * 1024;

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

    /// Publish the part of `bytes` after `offset` that is safe to show now
    /// (everything when `flush`, at exit) and advance `offset` past it.
    fn stream_output(
        &self,
        process_id: &EntityId,
        stream: ProcessStream,
        bytes: &[u8],
        offset: &mut usize,
        flush: bool,
    ) {
        let Some(pending) = bytes.get(*offset..).filter(|pending| !pending.is_empty()) else {
            return;
        };
        let end = if flush {
            pending.len()
        } else {
            publishable(pending)
        };
        if end == 0 {
            return;
        }
        let text = String::from_utf8_lossy(&pending[..end]);
        let redacted = aidex_events::redact_text(&text);
        let mut rest = redacted.as_ref();
        while !rest.is_empty() {
            let chunk = crate::untrusted::bounded(rest, MAX_CHUNK);
            // A first character longer than the chunk cannot happen (UTF-8
            // characters are at most four bytes), but never loop on it.
            let chunk = if chunk.is_empty() { rest } else { chunk };
            rest = &rest[chunk.len()..];
            let _ = self.events.emit(
                self.scope.clone(),
                EventKind::ProcessOutput {
                    process_id: process_id.clone(),
                    stream,
                    chunk: chunk.to_owned(),
                },
            );
        }
        *offset += end;
    }

    /// Publish both streams of `output`.
    fn stream_both(
        &self,
        process_id: &EntityId,
        output: &aidex_sandbox::CapturedOutput,
        offsets: &mut [usize; 2],
        flush: bool,
    ) {
        let [stdout, stderr] = offsets;
        self.stream_output(
            process_id,
            ProcessStream::Stdout,
            &output.stdout.bytes,
            stdout,
            flush,
        );
        self.stream_output(
            process_id,
            ProcessStream::Stderr,
            &output.stderr.bytes,
            stderr,
            flush,
        );
    }
}

/// How many leading bytes of `pending` may be published before the process
/// exits: whole lines only, ending before the first private-key block that
/// has no end line yet.
fn publishable(pending: &[u8]) -> usize {
    const BEGIN: &[u8] = b"-----BEGIN ";
    const DASHES: &[u8] = b"-----";
    let lines = pending
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let complete = &pending[..lines];
    let mut from = 0;
    while let Some(start) = find(complete, from, BEGIN) {
        let label_start = start + BEGIN.len();
        // `complete` ends with a newline, so every header line has an end.
        let line_end = find(complete, label_start, b"\n").unwrap_or(complete.len());
        let Some(label_end) = find(&complete[..line_end], label_start, DASHES) else {
            from = label_start;
            continue;
        };
        let label = &complete[label_start..label_end];
        let after = label_end + DASHES.len();
        if !label.windows(11).any(|window| window == b"PRIVATE KEY") {
            from = label_start;
            continue;
        }
        let footer = [b"-----END ".as_slice(), label, DASHES].concat();
        match find(complete, after, &footer) {
            Some(end) => from = end + footer.len(),
            None => return start,
        }
    }
    lines
}

/// Position of `needle` in `haystack` at or after `from`.
fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

fn event_level(level: SandboxLevel) -> aidex_events::SandboxLevel {
    match level {
        SandboxLevel::Direct => aidex_events::SandboxLevel::Direct,
        SandboxLevel::Brokered => aidex_events::SandboxLevel::Brokered,
        SandboxLevel::Restricted => aidex_events::SandboxLevel::Restricted,
        SandboxLevel::Ephemeral => aidex_events::SandboxLevel::Ephemeral,
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
        // The command line is in the encrypted tool plan; the event names
        // only the process.
        let _ = self.events.emit(
            self.scope.clone(),
            EventKind::ProcessStarted {
                process_id: process.id().clone(),
                command_summary: "native tool process".into(),
                sandbox_level: event_level(self.level),
            },
        );
        let mut offsets = [0; 2];
        let mut last_output = Instant::now();
        loop {
            if cancel.is_cancelled() {
                process.kill_tree().map_err(LaunchError::Sandbox)?;
            }
            if let Some(status) = process
                .wait_timeout(Duration::from_millis(20))
                .map_err(LaunchError::Sandbox)?
            {
                let output = process.take_output();
                self.stream_both(process.id(), &output, &mut offsets, true);
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
            if last_output.elapsed() >= OUTPUT_INTERVAL {
                last_output = Instant::now();
                self.stream_both(process.id(), &process.output_so_far(), &mut offsets, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launcher(events: Arc<aidex_events::MemoryEventSink>) -> NativeLauncher {
        NativeLauncher::new(
            Sandbox::locate(),
            PathBuf::new(),
            PlanFingerprint::from_bytes([0; 32]),
            SandboxLevel::Brokered,
            events,
            EventScope::global(),
        )
    }

    fn published(events: &aidex_events::MemoryEventSink) -> String {
        events
            .events()
            .iter()
            .filter_map(|event| match &event.event {
                EventKind::ProcessOutput { chunk, .. } => Some(chunk.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn live_output_waits_for_complete_line_before_redacting_split_credentials() {
        let events = Arc::new(aidex_events::MemoryEventSink::new());
        let launcher = launcher(events.clone());
        let id = EntityId::new("process_fixture").unwrap();
        let mut offset = 0;
        launcher.stream_output(
            &id,
            ProcessStream::Stdout,
            b"Authorization: Bearer fixture",
            &mut offset,
            false,
        );
        assert!(events.is_empty(), "an incomplete line is held");
        assert_eq!(offset, 0);
        let line = b"Authorization: Bearer fixture-secret-value\n";
        launcher.stream_output(&id, ProcessStream::Stdout, line, &mut offset, false);
        assert!(!events.is_empty());
        assert!(!published(&events).contains("fixture-secret-value"));
        assert_eq!(offset, line.len());
        // Later output continues after what was published.
        let more = [line.as_slice(), b"build ok\npartial"].concat();
        launcher.stream_output(&id, ProcessStream::Stdout, &more, &mut offset, false);
        assert!(published(&events).ends_with("build ok\n"));
        launcher.stream_output(&id, ProcessStream::Stdout, &more, &mut offset, true);
        assert!(published(&events).ends_with("build ok\npartial"));
        assert_eq!(offset, more.len());
    }

    #[test]
    fn live_output_keeps_private_key_blocks_together_across_lines() {
        let events = Arc::new(aidex_events::MemoryEventSink::new());
        let launcher = launcher(events.clone());
        let id = EntityId::new("process_fixture").unwrap();
        let mut offset = 0;
        let header = b"before\n-----BEGIN PRIVATE KEY-----\n".as_slice();
        launcher.stream_output(&id, ProcessStream::Stdout, header, &mut offset, false);
        assert_eq!(published(&events), "before\n", "the block start is held");
        let body = [header, b"cHJpdmF0ZS1rZXktcGF5bG9hZA==\n"].concat();
        launcher.stream_output(&id, ProcessStream::Stdout, &body, &mut offset, false);
        assert!(!published(&events).contains("cHJpdmF0ZS1rZXktcGF5bG9hZA=="));
        let block = [body.as_slice(), b"-----END PRIVATE KEY-----\nafter\n"].concat();
        launcher.stream_output(&id, ProcessStream::Stdout, &block, &mut offset, false);
        let text = published(&events);
        assert!(!text.contains("cHJpdmF0ZS1rZXktcGF5bG9hZA=="));
        assert!(text.contains(aidex_events::redact::REDACTED) && text.ends_with("after\n"));
        assert_eq!(offset, block.len());
        // Other PEM blocks (certificates) are not held.
        assert_eq!(
            publishable(b"-----BEGIN CERTIFICATE-----\nAAAA\n"),
            b"-----BEGIN CERTIFICATE-----\nAAAA\n".len()
        );
    }
}
