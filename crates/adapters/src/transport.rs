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
    /// The first 16 KiB of stderr, kept only to name a closed failure category.
    stderr: zeroize::Zeroizing<Vec<u8>>,
    stderr_bytes: u64,
    stderr_truncated: bool,
}
/// Stderr kept for diagnostics; never returned or logged.
const STDERR_SAMPLE: usize = 16 * 1024;
impl Transport {
    pub fn new(process: ManagedProcess, secrets: Vec<zeroize::Zeroizing<String>>) -> Self {
        Self {
            process: std::sync::Arc::new(process),
            pending: vec![],
            frames: VecDeque::new(),
            total: 0,
            secrets,
            stderr: zeroize::Zeroizing::new(Vec::new()),
            stderr_bytes: 0,
            stderr_truncated: false,
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
            if !self.fill()? {
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
    /// The next frame without waiting: `Ok(None)` while the process runs and
    /// no complete frame arrived. Ends like [`Self::next`] (bounds, a closed
    /// process), for a caller that checks its own conditions between polls.
    pub fn poll(&mut self) -> Result<Option<Value>, RuntimeError> {
        if self.frames.is_empty() {
            self.fill()?;
        }
        Ok(self.frames.pop_front())
    }
    /// Take the process output buffered so far and parse its complete
    /// frames. `Ok(true)` when frames are ready, `Ok(false)` while the process
    /// runs without one; a process that ended without one is `process_closed`.
    fn fill(&mut self) -> Result<bool, RuntimeError> {
        let output = self.process.take_output();
        self.stderr_bytes = output.stderr.total_bytes;
        self.stderr_truncated |= output.stderr.truncated;
        let room = STDERR_SAMPLE.saturating_sub(self.stderr.len());
        let kept = room.min(output.stderr.bytes.len());
        self.stderr.extend_from_slice(&output.stderr.bytes[..kept]);
        self.stderr_truncated |= kept < output.stderr.bytes.len();
        drop(zeroize::Zeroizing::new(output.stderr.bytes));
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
            return Ok(true);
        }
        if !self.process.is_running() {
            let exit = self.process.try_wait().ok().flatten();
            let reason = if self.pending.is_empty() {
                "Runtime exited before root completion"
            } else {
                "Truncated runtime protocol frame"
            };
            // Exit code, counts and a closed category: never vendor output.
            return Err(error(
                "process_closed",
                &format!(
                    "{reason} (exit={:?}; stderr={}; bytes={}; truncated={})",
                    exit.and_then(|status| status.code),
                    crate::diagnostics::stderr_category(&self.stderr),
                    self.stderr_bytes,
                    self.stderr_truncated
                ),
            ));
        }
        Ok(false)
    }
    pub fn stop(&self) {
        let _ = self.process.kill_tree();
        let _ = self.process.wait_timeout(Duration::from_secs(5));
    }
    /// Scrub every handed-over credential from text assembled out of several
    /// frames (a value split across streamed fragments).
    pub fn scrub_text(&self, text: String) -> String {
        scrub_text(text, &self.secrets)
    }
    /// Also scrub these values from later frames: a credential the vendor
    /// refreshed after launch. Values already scrubbed are kept.
    pub fn add_secrets(&mut self, secrets: Vec<zeroize::Zeroizing<String>>) {
        for secret in secrets {
            if !secret.is_empty() && !self.secrets.iter().any(|s| **s == *secret) {
                self.secrets.push(secret);
            }
        }
    }
}
impl Drop for Transport {
    // Dropping the `ManagedProcess` alone only requests the kill: the tree stays live in
    // the process registry until its monitor reaps it. Error paths that return without
    // `stop` (an error response, a missing native identity, a write that fails because
    // the tree was killed by the cancel watcher) would then report failure while the
    // native process is still registered, so emergency stop and configuration changes
    // would see a busy runtime. Stopping here makes every exit path confirm the tree is
    // gone (bounded like `stop`); a run that already ended skips it.
    fn drop(&mut self) {
        if self.process.is_running() {
            self.stop();
        }
    }
}
fn scrub(value: Value, secrets: &[zeroize::Zeroizing<String>]) -> Value {
    match value {
        Value::String(s) => Value::String(scrub_text(s, secrets)),
        Value::Array(v) => Value::Array(v.into_iter().map(|v| scrub(v, secrets)).collect()),
        // A vendor can reflect a credential as an object key or as a number.
        Value::Object(v) => Value::Object(
            v.into_iter()
                .map(|(k, v)| (scrub_text(k, secrets), scrub(v, secrets)))
                .collect(),
        ),
        Value::Number(_) | Value::Bool(_) | Value::Null => {
            let text = value.to_string();
            if secrets
                .iter()
                .any(|s| !s.is_empty() && text.contains(s.as_str()))
            {
                Value::String("[REDACTED]".into())
            } else {
                value
            }
        }
    }
}
fn scrub_text(mut s: String, secrets: &[zeroize::Zeroizing<String>]) -> String {
    for secret in secrets {
        if !secret.is_empty() {
            s = s.replace(secret.as_str(), "[REDACTED]");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_reflections_are_scrubbed_from_keys_and_non_string_values() {
        let protected = vec![
            zeroize::Zeroizing::new("opaque-key-marker".into()),
            zeroize::Zeroizing::new("48199273".into()),
        ];
        let input = serde_json::json!({"opaque-key-marker":{"nested":[48199273,"opaque-key-marker"]},"numeric":48199273,"flag":true});
        let output = scrub(input, &protected);
        let text = output.to_string();
        assert!(!text.contains("opaque-key-marker"));
        assert!(!text.contains("48199273"));
        assert_eq!(output["numeric"], "[REDACTED]");
        assert_eq!(output["flag"], true);
    }
    #[test]
    fn text_joined_from_fragments_is_scrubbed_again() {
        let fragments = ["prefix Split", "Credential", "Value_77 suffix"];
        let protected = vec![zeroize::Zeroizing::new("SplitCredentialValue_77".into())];
        // No single fragment holds the value, so per-frame scrubbing keeps them.
        for fragment in fragments {
            assert_eq!(scrub_text(fragment.into(), &protected), fragment);
        }
        assert_eq!(
            scrub_text(fragments.concat(), &protected),
            "prefix [REDACTED] suffix"
        );
    }
}
