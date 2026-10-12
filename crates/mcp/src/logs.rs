//! A bounded ring buffer of one server's log lines: its stderr, the log
//! notifications it sends, and the client's own lifecycle notes. Every line is
//! scrubbed of the server's secret values and credential patterns. A stderr line
//! is scrubbed whole: an over-long one is recorded once from a held prefix far
//! longer than the part shown, and the rest of it is dropped, so a secret is
//! never split across two records.

use std::{
    collections::VecDeque,
    sync::{Mutex, MutexGuard, PoisonError},
};

use aidex_core::Timestamp;
use serde::Serialize;

use crate::redact::{Scrubber, bounded_redacted};

/// Lines kept per server.
pub const MAX_LOG_LINES: usize = 400;
/// Bytes kept per server.
const MAX_LOG_BYTES: usize = 128 * 1024;
/// Longest line kept; longer lines are cut.
const MAX_LINE_BYTES: usize = 2048;
/// Raw bytes of one stderr line held before it is recorded. Well past
/// `MAX_LINE_BYTES`, so a secret that would show in the kept part is whole when
/// the line is scrubbed.
const MAX_HELD_BYTES: usize = 16 * 1024;

/// Where a log line came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStream {
    /// The server process's standard error.
    Stderr,
    /// A `notifications/message` log notification from the server.
    Server,
    /// AiDEX's own notes (started, stopped, failures).
    Client,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogLine {
    pub at: Timestamp,
    pub stream: LogStream,
    pub text: String,
}

/// The newest lines, oldest first.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogTail {
    pub lines: Vec<LogLine>,
    /// Lines that were evicted from the buffer so far.
    pub dropped: u64,
}

#[derive(Default)]
struct State {
    lines: VecDeque<LogLine>,
    bytes: usize,
    dropped: u64,
    partial: Vec<u8>,
    /// The current stderr line was already recorded; drop it up to its newline.
    discarding: bool,
    scrubber: Scrubber,
}

/// One server's log buffer. Shared between the supervisor and its connection.
#[derive(Default)]
pub struct LogBuffer {
    state: Mutex<State>,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Scrub these secret values from every later line.
    pub fn set_scrubber(&self, scrubber: Scrubber) {
        self.state().scrubber = scrubber;
    }

    /// Append raw stderr bytes; complete lines are recorded, the rest waits.
    pub fn push_stderr(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut state = self.state();
        let mut rest = bytes;
        while !rest.is_empty() {
            let newline = rest.iter().position(|b| *b == b'\n');
            let (chunk, ended) = match newline {
                Some(end) => (&rest[..end], true),
                None => (rest, false),
            };
            rest = newline.map_or(&[][..], |end| &rest[end + 1..]);
            if !state.discarding {
                let room = MAX_HELD_BYTES.saturating_sub(state.partial.len());
                state
                    .partial
                    .extend_from_slice(&chunk[..chunk.len().min(room)]);
                if ended || state.partial.len() >= MAX_HELD_BYTES {
                    let line = std::mem::take(&mut state.partial);
                    record(
                        &mut state,
                        LogStream::Stderr,
                        &String::from_utf8_lossy(&line),
                    );
                    state.discarding = !ended;
                }
            } else if ended {
                state.discarding = false;
            }
        }
    }

    /// Record any stderr text still waiting for a newline.
    pub fn flush_stderr(&self) {
        let mut state = self.state();
        state.discarding = false;
        if !state.partial.is_empty() {
            let line = std::mem::take(&mut state.partial);
            record(
                &mut state,
                LogStream::Stderr,
                &String::from_utf8_lossy(&line),
            );
        }
    }

    /// Record `text` (one line per newline-separated part).
    pub fn push(&self, stream: LogStream, text: &str) {
        let mut state = self.state();
        for line in text.lines() {
            record(&mut state, stream, line);
        }
    }

    /// The newest `limit` lines.
    pub fn tail(&self, limit: usize) -> LogTail {
        let state = self.state();
        let skip = state.lines.len().saturating_sub(limit);
        LogTail {
            lines: state.lines.iter().skip(skip).cloned().collect(),
            dropped: state.dropped + skip as u64,
        }
    }

    /// The newest stderr line, for failure messages.
    pub fn last_stderr(&self) -> Option<String> {
        self.state()
            .lines
            .iter()
            .rev()
            .find(|line| line.stream == LogStream::Stderr)
            .map(|line| line.text.clone())
    }
}

fn record(state: &mut State, stream: LogStream, text: &str) {
    let text = text.trim_end_matches('\r');
    if text.trim().is_empty() {
        return;
    }
    let scrubbed = state.scrubber.text(text);
    let text = bounded_redacted(&scrubbed, MAX_LINE_BYTES);
    state.bytes += text.len();
    state.lines.push_back(LogLine {
        at: Timestamp::now(),
        stream,
        text,
    });
    while state.lines.len() > MAX_LOG_LINES || state.bytes > MAX_LOG_BYTES {
        match state.lines.pop_front() {
            Some(old) => {
                state.bytes -= old.text.len();
                state.dropped += 1;
            }
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeroize::Zeroizing;

    #[test]
    fn stderr_is_split_scrubbed_and_bounded() {
        let logs = LogBuffer::new();
        logs.set_scrubber(Scrubber::new(vec![Zeroizing::new("value-9876".into())]));
        logs.push_stderr(b"first line\r\nsecret value-9876 here\npart");
        logs.push_stderr(b"ial\n");
        let tail = logs.tail(10);
        let texts: Vec<&str> = tail.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["first line", "secret [REDACTED] here", "partial"]);
        assert_eq!(logs.last_stderr().as_deref(), Some("partial"));
        for n in 0..(MAX_LOG_LINES + 10) {
            logs.push(LogStream::Client, &format!("line {n}"));
        }
        let tail = logs.tail(5);
        assert_eq!(tail.lines.len(), 5);
        assert!(tail.dropped > 0);
        assert_eq!(
            tail.lines.last().unwrap().text,
            format!("line {}", MAX_LOG_LINES + 9)
        );
    }

    fn stderr_lines(logs: &LogBuffer) -> Vec<String> {
        logs.tail(MAX_LOG_LINES)
            .lines
            .into_iter()
            .filter(|line| line.stream == LogStream::Stderr)
            .map(|line| line.text)
            .collect()
    }

    #[test]
    fn a_secret_split_across_reads_of_a_long_line_never_shows() {
        let secret = "secret-token-1234";
        let logs = LogBuffer::new();
        logs.set_scrubber(Scrubber::new(vec![Zeroizing::new(secret.into())]));
        // Past the shown length, but within the held prefix: scrubbed whole.
        logs.push_stderr(&[b'x'; MAX_LINE_BYTES - 3]);
        logs.push_stderr(b"secret-to");
        logs.push_stderr(b"ken-1234 more\nnext\n");
        // Past the held prefix: recorded once, and the rest of the line dropped.
        logs.push_stderr(&[b'y'; MAX_HELD_BYTES - 4]);
        logs.push_stderr(b"secret-to");
        logs.push_stderr(b"ken-1234 ");
        logs.push_stderr(&[b'z'; 3 * MAX_HELD_BYTES]);
        logs.push_stderr(b"\nafter\nunfinished");
        logs.flush_stderr();
        let lines = stderr_lines(&logs);
        assert_eq!(lines.len(), 5, "{lines:?}");
        for line in &lines {
            for fragment in ["secret", "ken-1234", "-to", "z"] {
                assert!(!line.contains(fragment), "{fragment} in {line}");
            }
            assert!(line.len() <= MAX_LINE_BYTES + 3);
        }
        assert!(lines[0].starts_with("xxx") && lines[0].ends_with("..."));
        assert!(lines[2].starts_with("yyy") && lines[2].ends_with("..."));
        assert_eq!(lines[1], "next");
        assert_eq!(&lines[3..], ["after", "unfinished"]);
    }
}
