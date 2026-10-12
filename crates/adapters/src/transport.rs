use crate::state::error;
use aidex_core::CancelToken;
use aidex_native_sessions::RuntimeError;
use aidex_sandbox::ManagedProcess;
use serde_json::Value;
use std::{collections::VecDeque, thread, time::Duration};
pub(crate) const MAX_FRAME: usize = 1024 * 1024;
pub(crate) struct Transport {
    pub process: std::sync::Arc<ManagedProcess>,
    pending: Vec<u8>,
    frames: VecDeque<Value>,
    total: u64,
    secrets: Vec<zeroize::Zeroizing<String>>,
}
impl Transport {
    pub fn new(process: ManagedProcess, secrets: Vec<zeroize::Zeroizing<String>>) -> Self {
        Self {
            process: std::sync::Arc::new(process),
            pending: vec![],
            frames: VecDeque::new(),
            total: 0,
            secrets,
        }
    }
    pub fn send(&self, value: Value) -> Result<(), RuntimeError> {
        let mut bytes = serde_json::to_vec(&value)
            .map_err(|_| error("protocol_error", "Invalid outgoing protocol frame"))?;
        if bytes.len() > MAX_FRAME {
            return Err(error("output_limit", "Protocol input exceeds limit"));
        }
        bytes.push(b'\n');
        self.process
            .write_stdin(&bytes)
            .map_err(|_| error("process_closed", "Runtime input closed"))
    }
    pub fn next(&mut self, cancel: &CancelToken) -> Result<Value, RuntimeError> {
        loop {
            if cancel.is_stopped() {
                self.stop();
                return Err(error("cancelled", "Runtime cancelled"));
            }
            if let Some(frame) = self.frames.pop_front() {
                return Ok(frame);
            }
            let output = self.process.take_output();
            self.total = self.total.saturating_add(output.stdout.bytes.len() as u64);
            if output.stdout.truncated || self.total > 16 * 1024 * 1024 {
                self.stop();
                return Err(error(
                    "output_limit",
                    "Runtime protocol output exceeds 16 MiB",
                ));
            }
            self.pending.extend(output.stdout.bytes);
            while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                if end > MAX_FRAME {
                    self.stop();
                    return Err(error(
                        "output_limit",
                        "Runtime protocol frame exceeds 1 MiB",
                    ));
                }
                let line: Vec<u8> = self.pending.drain(..=end).collect();
                let line = &line[..line.len() - 1];
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.is_empty() {
                    continue;
                }
                let frame: Value = serde_json::from_slice(line).map_err(|_| {
                    self.stop();
                    error("protocol_error", "Malformed runtime protocol frame")
                })?;
                if !frame.is_object() {
                    self.stop();
                    return Err(error("protocol_error", "Runtime frame must be an object"));
                }
                self.frames.push_back(scrub(frame, &self.secrets));
            }
            if self.pending.len() > MAX_FRAME {
                self.stop();
                return Err(error("output_limit", "Runtime frame exceeds 1 MiB"));
            }
            if !self.frames.is_empty() {
                continue;
            }
            if !self.process.is_running() {
                return Err(error(
                    "process_closed",
                    if self.pending.is_empty() {
                        "Runtime exited before root completion"
                    } else {
                        "Truncated runtime protocol frame"
                    },
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn stop(&self) {
        let _ = self.process.kill_tree();
        let _ = self.process.wait_timeout(Duration::from_secs(5));
    }
}
fn scrub(value: Value, secrets: &[zeroize::Zeroizing<String>]) -> Value {
    match value {
        Value::String(mut s) => {
            for secret in secrets {
                if !secret.is_empty() {
                    s = s.replace(secret.as_str(), "[REDACTED]");
                }
            }
            Value::String(s)
        }
        Value::Array(v) => Value::Array(v.into_iter().map(|v| scrub(v, secrets)).collect()),
        Value::Object(v) => {
            Value::Object(v.into_iter().map(|(k, v)| (k, scrub(v, secrets))).collect())
        }
        v => v,
    }
}
