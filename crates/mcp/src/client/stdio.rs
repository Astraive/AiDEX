//! MCP over a sandboxed process's stdin/stdout: newline-delimited JSON-RPC,
//! at most 1 MiB per frame. A reader thread drains the process continuously so a
//! chatty server never fills its pipes: stderr goes to the log buffer, server
//! notifications and requests are handled in place, and the response to the
//! request in flight is handed to the waiting caller (any other response is
//! dropped, so a server cannot queue frames in the daemon).
//!
//! A writer thread owns stdin, fed by a bounded queue, so no thread ever blocks
//! on a server that stopped reading its input: answers to the server's own
//! requests are dropped once the queue is full, a request gives up when its
//! cancel token fires (killing the tree when its frame may be half written, as
//! the stream is then unusable), and closing kills the tree instead of waiting.

use std::{
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
        mpsc::{
            Receiver, RecvTimeoutError, Sender, SyncSender, TrySendError, channel, sync_channel,
        },
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use aidex_core::CancelToken;
use aidex_sandbox::ManagedProcess;
use serde_json::Value;

use super::{
    Channel, cancelled_error,
    shared::{Routed, Shared, id_matches, route},
};
use crate::{LogStream, McpError, McpErrorKind, redact::Scrubber, redact::bounded_redacted};

/// Largest single protocol frame.
pub const MAX_FRAME: usize = 1024 * 1024;
const POLL: Duration = Duration::from_millis(10);
/// How long a server may take to exit after its stdin closes.
const GRACEFUL_EXIT: Duration = Duration::from_secs(2);
/// How long closing waits for the writer to end the input, which a write
/// blocked on a server that stopped reading holds, before the tree is killed.
const STDIN_CLOSE_GRACE: Duration = Duration::from_millis(500);
/// How long closing waits for the reader and writer threads once the tree is
/// gone.
const THREAD_EXIT: Duration = Duration::from_secs(5);
/// Responses queued for the caller; only the awaited id is queued at all.
const MAX_QUEUED: usize = 16;
/// Frames queued for the writer. Once a server stops reading its input, answers
/// to its requests beyond this are dropped.
const MAX_QUEUED_WRITES: usize = 64;
/// `awaiting` when no request is in flight (request ids start at 1).
const NOT_AWAITING: i64 = 0;

type Delivery = Result<(Value, Result<Value, McpError>), McpError>;

/// One frame for the server's stdin. `written` learns whether all of it was
/// written (answers to the server's own requests are not waited for).
struct Frame {
    bytes: Vec<u8>,
    written: Option<Sender<bool>>,
}

pub(crate) struct StdioChannel {
    process: Arc<ManagedProcess>,
    shared: Arc<Shared>,
    responses: Receiver<Delivery>,
    /// The id of the request in flight, or [`NOT_AWAITING`].
    awaiting: Arc<AtomicI64>,
    /// Frames for the writer thread; `None` once closed.
    writes: Option<SyncSender<Frame>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
}

impl StdioChannel {
    pub fn start(
        process: ManagedProcess,
        shared: Arc<Shared>,
        scrubber: Scrubber,
    ) -> Result<Self, McpError> {
        let process = Arc::new(process);
        let (sender, responses) = sync_channel(MAX_QUEUED);
        let (writes, queued) = sync_channel(MAX_QUEUED_WRITES);
        let awaiting = Arc::new(AtomicI64::new(NOT_AWAITING));
        let failed = |process: &ManagedProcess, shared: &Shared| {
            shared.close();
            let _ = process.kill_tree();
            McpError::new(
                McpErrorKind::Unavailable,
                "cannot start the MCP reader and writer threads",
            )
        };
        let writer = {
            let writing = Arc::clone(&process);
            let state = Arc::clone(&shared);
            thread::Builder::new()
                .name("aidex-mcp-stdin".into())
                .spawn(move || write_loop(&writing, &state, &queued))
                .map_err(|_| failed(&process, &shared))?
        };
        let reader = {
            let reading = Arc::clone(&process);
            let state = Arc::clone(&shared);
            let awaited = Arc::clone(&awaiting);
            let replies = writes.clone();
            thread::Builder::new()
                .name("aidex-mcp-stdio".into())
                .spawn(move || {
                    read_loop(&reading, &state, &scrubber, &awaited, &sender, &replies);
                })
                .map_err(|_| failed(&process, &shared))?
        };
        Ok(Self {
            process,
            shared,
            responses,
            awaiting,
            writes: Some(writes),
            reader: Some(reader),
            writer: Some(writer),
        })
    }

    pub fn process(&self) -> Arc<ManagedProcess> {
        Arc::clone(&self.process)
    }

    /// Why the connection is unusable: stopped by AiDEX (`cancelled`), or the
    /// server's own failure.
    fn disconnected(&self) -> McpError {
        if self.shared.is_closed() {
            return McpError::new(McpErrorKind::Cancelled, "the MCP server was stopped");
        }
        self.shared.ended().unwrap_or_else(|| {
            McpError::new(McpErrorKind::Disconnected, "the MCP server is not running")
        })
    }

    fn wait_response(&mut self, id: i64, cancel: &CancelToken) -> Result<Value, McpError> {
        loop {
            if let Some(error) = cancelled_error(cancel, &self.shared) {
                return Err(error);
            }
            let wait = cancel.remaining().map_or(POLL, |left| left.min(POLL));
            match self
                .responses
                .recv_timeout(wait.max(Duration::from_millis(1)))
            {
                Ok(Ok((response_id, outcome))) if id_matches(&response_id, id) => return outcome,
                Ok(Ok(_)) => {}
                Ok(Err(error)) => return Err(error),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return Err(self.disconnected()),
            }
        }
    }

    /// Hand `bytes` to the writer and wait until they are written, at most until
    /// `cancel` fires. Giving up before the writer took the frame leaves the
    /// stream intact; giving up after it may leave half a frame in the pipe, so
    /// the tree is killed (a server holds a write that long only when it stopped
    /// reading its input).
    fn write(&self, bytes: Vec<u8>, cancel: &CancelToken) -> Result<(), McpError> {
        let Some(writes) = &self.writes else {
            return Err(self.disconnected());
        };
        let (written, done) = channel();
        let mut frame = Frame {
            bytes,
            written: Some(written),
        };
        loop {
            match writes.try_send(frame) {
                Ok(()) => break,
                Err(TrySendError::Full(back)) => {
                    if let Some(error) = cancelled_error(cancel, &self.shared) {
                        return Err(error);
                    }
                    frame = back;
                    thread::sleep(POLL);
                }
                Err(TrySendError::Disconnected(_)) => return Err(self.disconnected()),
            }
        }
        loop {
            match done.recv_timeout(POLL) {
                Ok(true) => return Ok(()),
                Ok(false) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(self.disconnected());
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(error) = cancelled_error(cancel, &self.shared) {
                        self.abandon_write();
                        return Err(error);
                    }
                }
            }
        }
    }

    /// End a connection whose input may hold half a frame.
    fn abandon_write(&self) {
        if !self.shared.is_closed() {
            let error = McpError::new(
                McpErrorKind::Disconnected,
                "the MCP server stopped reading its input, so it was stopped",
            );
            self.shared.logs.push(LogStream::Client, &error.message);
            self.shared.end(error);
        }
        let _ = self.process.kill_tree();
    }
}

impl Channel for StdioChannel {
    fn send(&mut self, message: &Value, cancel: &CancelToken) -> Result<(), McpError> {
        if !self.shared.is_alive() {
            return Err(self.disconnected());
        }
        if let Some(error) = cancelled_error(cancel, &self.shared) {
            return Err(error);
        }
        let mut line = serde_json::to_vec(message)
            .map_err(|_| McpError::protocol("cannot encode the MCP message"))?;
        if line.len() > MAX_FRAME {
            return Err(McpError::new(
                McpErrorKind::OutputLimit,
                "the MCP request exceeds 1 MiB",
            ));
        }
        line.push(b'\n');
        // A request's answer is accepted from the moment it is sent.
        let request_id = message
            .get("method")
            .and(message.get("id"))
            .and_then(Value::as_i64);
        if let Some(id) = request_id {
            self.awaiting.store(id, Ordering::SeqCst);
        }
        self.write(line, cancel).inspect_err(|_| {
            if request_id.is_some() {
                self.awaiting.store(NOT_AWAITING, Ordering::SeqCst);
            }
        })
    }

    fn response(&mut self, id: i64, cancel: &CancelToken) -> Result<Value, McpError> {
        let outcome = self.wait_response(id, cancel);
        // A late or repeated answer is dropped from now on.
        self.awaiting.store(NOT_AWAITING, Ordering::SeqCst);
        outcome
    }

    fn is_alive(&self) -> bool {
        self.shared.is_alive() && self.process.is_running()
    }

    fn close(&mut self) {
        self.shared.close();
        self.writes = None;
        // The writer ends the input once it sees the close, unless a write
        // blocked on a server that stopped reading holds it.
        let input_closed = self
            .writer
            .as_ref()
            .is_none_or(|writer| finished_within(writer, STDIN_CLOSE_GRACE));
        if !input_closed || matches!(self.process.wait_timeout(GRACEFUL_EXIT), Ok(None)) {
            // Killing the tree also fails a write blocked on its input.
            let _ = self.process.kill_tree();
            let _ = self.process.wait_timeout(Duration::from_secs(5));
        }
        join_within(self.writer.take(), THREAD_EXIT);
        join_within(self.reader.take(), THREAD_EXIT);
    }
}

impl Drop for StdioChannel {
    fn drop(&mut self) {
        if self.reader.is_some() || self.writer.is_some() {
            self.shared.close();
            let _ = self.process.kill_tree();
            join_within(self.writer.take(), THREAD_EXIT);
            join_within(self.reader.take(), THREAD_EXIT);
        }
    }
}

/// Whether `thread` finished within `limit`.
fn finished_within(thread: &JoinHandle<()>, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while !thread.is_finished() && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    thread.is_finished()
}

/// Join `thread`, or leave it to finish on its own after `limit` (it holds only
/// shared handles and ends once the connection is closed and the tree is gone).
fn join_within(thread: Option<JoinHandle<()>>, limit: Duration) {
    if let Some(thread) = thread
        && finished_within(&thread, limit)
    {
        let _ = thread.join();
    }
}

/// Write queued frames in order until the connection is closed or ends, or a
/// write fails, then end the server's input.
fn write_loop(process: &ManagedProcess, shared: &Shared, queued: &Receiver<Frame>) {
    loop {
        let frame = match queued.recv_timeout(POLL) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) if shared.is_alive() => continue,
            Err(_) => break,
        };
        if !shared.is_alive() {
            break;
        }
        let written = process.write_stdin(&frame.bytes).is_ok();
        if let Some(sender) = frame.written {
            let _ = sender.send(written);
        }
        if !written {
            break;
        }
    }
    process.close_stdin();
}

fn read_loop(
    process: &Arc<ManagedProcess>,
    shared: &Arc<Shared>,
    scrubber: &Scrubber,
    awaiting: &AtomicI64,
    sender: &SyncSender<Delivery>,
    replies: &SyncSender<Frame>,
) {
    let mut pending: Vec<u8> = Vec::new();
    // Answers are being dropped because the server is not reading its input.
    let mut dropping_replies = false;
    loop {
        let running = process.is_running();
        let output = process.take_output();
        shared.logs.push_stderr(&output.stderr.bytes);
        if output.stdout.truncated {
            return fatal(
                process,
                shared,
                sender,
                McpError::new(
                    McpErrorKind::OutputLimit,
                    "the MCP server wrote more output than AiDEX could buffer",
                ),
            );
        }
        pending.extend_from_slice(&output.stdout.bytes);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            if end > MAX_FRAME {
                return fatal(process, shared, sender, frame_too_large());
            }
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = &line[..line.len() - 1];
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let Ok(frame) = serde_json::from_slice::<Value>(line) else {
                shared.logs.push(
                    LogStream::Client,
                    &format!(
                        "ignored a non-JSON line on stdout: {}",
                        bounded_redacted(&scrubber.text(&String::from_utf8_lossy(line)), 200)
                    ),
                );
                continue;
            };
            match route(shared, scrubber.value(frame)) {
                Ok(Routed::Response { id, outcome }) => {
                    let awaited = awaiting.load(Ordering::SeqCst);
                    if awaited == NOT_AWAITING || !id_matches(&id, awaited) {
                        shared.logs.push(
                            LogStream::Client,
                            "ignored a response to no request in flight",
                        );
                        continue;
                    }
                    match sender.try_send(Ok((id, outcome))) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => shared.logs.push(
                            LogStream::Client,
                            "ignored a repeated response to the request in flight",
                        ),
                        Err(TrySendError::Disconnected(_)) => return,
                    }
                }
                // Nothing is written once the connection is closing.
                Ok(Routed::Reply(_)) if shared.is_closed() => {}
                Ok(Routed::Reply(reply)) => {
                    let Ok(mut bytes) = serde_json::to_vec(&reply) else {
                        continue;
                    };
                    bytes.push(b'\n');
                    let frame = Frame {
                        bytes,
                        written: None,
                    };
                    match replies.try_send(frame) {
                        Ok(()) => dropping_replies = false,
                        Err(TrySendError::Full(_)) if !dropping_replies => {
                            dropping_replies = true;
                            shared.logs.push(
                                LogStream::Client,
                                "dropped answers to the server's requests: it is not reading its input",
                            );
                        }
                        Err(_) => {}
                    }
                }
                Ok(Routed::Handled) => {}
                Err(error) => shared.logs.push(
                    LogStream::Client,
                    &format!("ignored an invalid message: {}", error.message),
                ),
            }
        }
        if pending.len() > MAX_FRAME {
            return fatal(process, shared, sender, frame_too_large());
        }
        if !running {
            shared.logs.flush_stderr();
            if !shared.is_closed() {
                let code = process
                    .try_wait()
                    .ok()
                    .flatten()
                    .and_then(|status| status.code);
                let detail = shared
                    .logs
                    .last_stderr()
                    .map(|line| format!(": {line}"))
                    .unwrap_or_default();
                let error = McpError::new(
                    McpErrorKind::Disconnected,
                    match code {
                        Some(code) => format!("the MCP server exited with code {code}{detail}"),
                        None => format!("the MCP server exited{detail}"),
                    },
                );
                shared.logs.push(LogStream::Client, &error.message);
                shared.end(error.clone());
                // When the queue is full the caller still learns why from
                // `shared` once this thread's sender is gone.
                let _ = sender.try_send(Err(error));
            }
            return;
        }
        if shared.is_closed() {
            return;
        }
        thread::sleep(POLL);
    }
}

fn frame_too_large() -> McpError {
    McpError::new(
        McpErrorKind::OutputLimit,
        "the MCP server sent a frame larger than 1 MiB",
    )
}

fn fatal(
    process: &ManagedProcess,
    shared: &Shared,
    sender: &SyncSender<Delivery>,
    error: McpError,
) {
    shared.logs.push(LogStream::Client, &error.message);
    shared.end(error.clone());
    let _ = process.kill_tree();
    let _ = sender.try_send(Err(error));
}
