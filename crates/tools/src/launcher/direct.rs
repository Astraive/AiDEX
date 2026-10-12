//! **Development and test only.** A launcher that runs processes directly as the
//! current user with no sandbox, no environment allowlist, and no process-tree
//! containment. Enabled by the non-default `direct-launcher` feature; production
//! daemons use the broker's sandboxed launcher.

use std::{
    io::{ErrorKind, Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant},
};

use aidex_sandbox::{CapturedOutput, ExitStatusInfo, KillReason, SandboxLevel, StreamCapture};

use crate::{CancelToken, LaunchError, LaunchOutcome, LaunchRequest, ProcessLauncher};

/// How often the launcher polls for exit, cancellation, and timeout.
const POLL: Duration = Duration::from_millis(10);
/// How long to wait for output pipes to close after the process ended.
const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Variables that would redirect git away from the workspace; always removed.
const GIT_REDIRECTS: [&str; 7] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// Runs processes directly (sandbox level `direct`). See the module documentation:
/// this is for tests and local development only.
#[derive(Clone, Copy, Debug, Default)]
pub struct DirectLauncher;

impl DirectLauncher {
    pub fn new() -> Self {
        Self
    }
}

fn reader(
    mut stream: impl Read + Send + 'static,
    capture: Arc<Mutex<StreamCapture>>,
    max: usize,
) -> Receiver<()> {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let mut capture = capture.lock().unwrap_or_else(PoisonError::into_inner);
                    capture.total_bytes += read as u64;
                    let room = max.saturating_sub(capture.bytes.len());
                    let kept = room.min(read);
                    capture.bytes.extend_from_slice(&buffer[..kept]);
                    if kept < read {
                        capture.truncated = true;
                        capture.dropped_bytes += (read - kept) as u64;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = done.send(());
    });
    finished
}

impl ProcessLauncher for DirectLauncher {
    fn launch(
        &self,
        request: &LaunchRequest,
        cancel: &CancelToken,
    ) -> Result<LaunchOutcome, LaunchError> {
        let mut command = Command::new(request.program());
        command
            .args(request.args())
            .current_dir(request.cwd())
            .stdin(if request.stdin().is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in GIT_REDIRECTS {
            command.env_remove(name);
        }
        for (name, value) in request.env() {
            command.env(name, value);
        }
        let started = Instant::now();
        let mut child = command.spawn().map_err(|error| {
            if error.kind() == ErrorKind::NotFound {
                LaunchError::ProgramNotFound(request.program().to_owned())
            } else {
                LaunchError::Spawn(error.to_string())
            }
        })?;
        if let (Some(bytes), Some(mut stdin)) = (request.stdin(), child.stdin.take()) {
            let bytes = bytes.to_vec();
            thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            });
        }
        let max = request.max_output_bytes();
        let stdout = Arc::new(Mutex::new(StreamCapture::default()));
        let stderr = Arc::new(Mutex::new(StreamCapture::default()));
        let mut drains = Vec::new();
        if let Some(pipe) = child.stdout.take() {
            drains.push(reader(pipe, stdout.clone(), max));
        }
        if let Some(pipe) = child.stderr.take() {
            drains.push(reader(pipe, stderr.clone(), max));
        }
        let mut killed_by = None;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(error) => return Err(LaunchError::Io(error.to_string())),
            }
            if killed_by.is_none() {
                if cancel.is_stopped() {
                    killed_by = Some(KillReason::Requested);
                } else if started.elapsed() >= request.timeout() {
                    killed_by = Some(KillReason::WallTimeout);
                }
                if killed_by.is_some() {
                    let _ = child.kill();
                }
            }
            thread::sleep(POLL);
        };
        let deadline = Instant::now() + DRAIN_GRACE;
        let output_complete = drains.iter().all(|drain| {
            drain
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .is_ok()
        });
        let take = |capture: &Arc<Mutex<StreamCapture>>| {
            std::mem::take(&mut *capture.lock().unwrap_or_else(PoisonError::into_inner))
        };
        #[cfg(unix)]
        let signal = std::os::unix::process::ExitStatusExt::signal(&status);
        #[cfg(not(unix))]
        let signal = None;
        Ok(LaunchOutcome {
            process_id: None,
            sandbox_level: SandboxLevel::Direct,
            status: ExitStatusInfo {
                code: status.code(),
                signal,
                killed_by,
                elapsed: started.elapsed(),
                output_complete,
            },
            output: CapturedOutput {
                stdout: take(&stdout),
                stderr: take(&stderr),
            },
        })
    }
}
