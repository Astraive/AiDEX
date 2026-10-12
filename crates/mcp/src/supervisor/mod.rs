//! The MCP supervisor: one slot per configured server holding its connection,
//! health, tool list, and log buffer.
//!
//! Servers start explicitly ([`McpSupervisor::start`]) or on demand (the first
//! [`McpSupervisor::list_tools`] or [`McpSupervisor::call_tool`]). Health is
//! `stopped | starting | running | failed | needs_auth`. A connection serves one
//! request at a time; [`McpSupervisor::stop`] interrupts an in-flight request or
//! start before closing, so it never waits for a slow server. A request waiting
//! for a busy connection honours its cancel token, and listing tools while a
//! call is in flight answers from the cached list.
//!
//! A start the caller gives up on leaves the server `stopped`, not `failed`.
//! When the caller's deadline (not a Stop) ended it, the server is not started
//! on demand again for five minutes, so a server that never finishes starting
//! does not cost every later caller its whole time budget; an explicit
//! [`McpSupervisor::start`] retries at once.

mod host;
mod roots;

pub use host::{
    LaunchedServer, McpGate, McpHost, OpenGate, SandboxHost, SecretSource, VaultSecrets,
};

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex, TryLockError},
    thread,
    time::{Duration, Instant},
};

/// How long `stop` waits for a busy connection before killing the server.
const BUSY_GRACE: Duration = Duration::from_secs(3);
/// How often a request waiting for a busy connection checks its cancel token.
const BUSY_POLL: Duration = Duration::from_millis(10);
/// How long a server whose start ran past its caller's deadline is not started
/// on demand again.
const START_BACKOFF: Duration = Duration::from_secs(5 * 60);

use aidex_core::{CancelReason, CancelToken, Timestamp};
use serde::Serialize;
use serde_json::Value;

use self::host::lock;
use crate::{
    LogBuffer, LogStream, LogTail, McpClient, McpConfigStore, McpError, McpErrorKind,
    McpToolResult, McpToolSpec, ServerConfig, ServerIdentity, Timeouts, TransportKind,
    client::{Interrupter, ToolListing},
    redact::{Scrubber, bounded_redacted},
};

/// Health of one server.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum McpStatus {
    #[default]
    Stopped,
    Starting,
    Running,
    Failed,
    /// A credential is missing or was refused (or the vault is locked).
    NeedsAuth,
}

impl McpStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Failed => "failed",
            Self::NeedsAuth => "needs_auth",
        }
    }
}

/// The last failure of a server, as shown to clients.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatusError {
    /// Stable [`McpErrorKind`] name.
    pub reason: McpErrorKind,
    pub message: String,
}

impl From<&McpError> for StatusError {
    fn from(error: &McpError) -> Self {
        Self {
            reason: error.kind,
            message: error.message.clone(),
        }
    }
}

/// A snapshot of one server's runtime state.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ServerState {
    pub status: McpStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<StatusError>,
    /// The tools of the running server (empty otherwise).
    pub tools: Vec<McpToolSpec>,
    /// Notes from the last tool listing (skipped or reduced tools).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<Timestamp>,
    /// Registry id of the stdio server process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
}

/// The tools of one enabled, running server (for the harness).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct McpToolset {
    pub server_id: String,
    pub server_name: String,
    pub auto_approve_tools: Vec<String>,
    pub tools: Vec<McpToolSpec>,
}

#[derive(Default)]
struct SlotState {
    status: McpStatus,
    error: Option<McpError>,
    tools: Vec<McpToolSpec>,
    notes: Vec<String>,
    identity: Option<ServerIdentity>,
    started_at: Option<Timestamp>,
    process_id: Option<String>,
    /// Interrupts the live (or starting) connection without its lock.
    interrupter: Option<Interrupter>,
    /// Bumped by every start and stop; a start that finds it changed was
    /// superseded and discards its connection.
    generation: u64,
    /// Not started on demand before this: its last start ran past the
    /// caller's deadline.
    retry_after: Option<Instant>,
}

impl SlotState {
    /// The status after noticing a connection that died on its own.
    fn refresh(&mut self, logs: &LogBuffer) {
        if self.status != McpStatus::Running {
            return;
        }
        let Some(interrupter) = &self.interrupter else {
            return;
        };
        if interrupter.is_alive() {
            return;
        }
        let failure = interrupter
            .failure()
            .unwrap_or_else(|| McpError::new(McpErrorKind::Disconnected, "the MCP server stopped"));
        logs.push(LogStream::Client, &failure.message);
        self.fail(failure);
    }

    fn fail(&mut self, error: McpError) {
        self.status = if error.needs_auth() {
            McpStatus::NeedsAuth
        } else {
            McpStatus::Failed
        };
        self.error = Some(error);
        self.tools.clear();
        self.notes.clear();
    }

    fn snapshot(&self) -> ServerState {
        ServerState {
            status: self.status,
            error: self.error.as_ref().map(StatusError::from),
            tools: self.tools.clone(),
            notes: self.notes.clone(),
            server: self.identity.clone(),
            started_at: self.started_at.clone(),
            process_id: self.process_id.clone(),
        }
    }
}

struct Slot {
    logs: Arc<LogBuffer>,
    /// Serializes start, stop, and restart of this server.
    lifecycle: Mutex<()>,
    state: Mutex<SlotState>,
    /// The live connection; held for the duration of each request.
    client: Mutex<Option<McpClient>>,
}

impl Slot {
    fn new() -> Self {
        Self {
            logs: Arc::new(LogBuffer::new()),
            lifecycle: Mutex::new(()),
            state: Mutex::new(SlotState::default()),
            client: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> ServerState {
        let mut state = lock(&self.state);
        state.refresh(&self.logs);
        state.snapshot()
    }

    /// The status, after noticing a connection that died on its own.
    fn status(&self) -> McpStatus {
        let mut state = lock(&self.state);
        state.refresh(&self.logs);
        state.status
    }

    fn is_running(&self) -> bool {
        self.status() == McpStatus::Running
    }

    /// Refuse an on-demand start while an earlier one that ran past its
    /// caller's deadline is recent.
    fn check_backoff(&self, id: &str) -> Result<(), McpError> {
        let retry_after = lock(&self.state).retry_after;
        let Some(left) = retry_after.and_then(|at| at.checked_duration_since(Instant::now()))
        else {
            return Ok(());
        };
        Err(McpError::new(
            McpErrorKind::Unavailable,
            format!(
                "the MCP server `{id}` did not finish starting in time before; it is started on \
                 demand again in {} s, or start it now",
                left.as_secs().max(1)
            ),
        ))
    }
}

/// Supervises every configured server. See the module documentation.
pub struct McpSupervisor {
    store: McpConfigStore,
    host: Arc<dyn McpHost>,
    timeouts: Timeouts,
    slots: Mutex<BTreeMap<String, Arc<Slot>>>,
}

impl fmt::Debug for McpSupervisor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpSupervisor")
            .field("store", &self.store.dir())
            .field("servers", &lock(&self.slots).len())
            .finish_non_exhaustive()
    }
}

impl McpSupervisor {
    pub fn new(store: McpConfigStore, host: Arc<dyn McpHost>) -> Self {
        Self::with_timeouts(store, host, Timeouts::default())
    }

    pub fn with_timeouts(
        store: McpConfigStore,
        host: Arc<dyn McpHost>,
        timeouts: Timeouts,
    ) -> Self {
        Self {
            store,
            host,
            timeouts,
            slots: Mutex::new(BTreeMap::new()),
        }
    }

    /// The configuration store the supervisor reads.
    pub fn store(&self) -> &McpConfigStore {
        &self.store
    }

    fn slot(&self, id: &str) -> Arc<Slot> {
        Arc::clone(
            lock(&self.slots)
                .entry(id.to_owned())
                .or_insert_with(|| Arc::new(Slot::new())),
        )
    }

    fn existing(&self, id: &str) -> Option<Arc<Slot>> {
        lock(&self.slots).get(id).cloned()
    }

    /// The runtime state of `id` (never starts it).
    pub fn state(&self, id: &str) -> ServerState {
        self.existing(id)
            .map(|slot| slot.snapshot())
            .unwrap_or_default()
    }

    /// Whether any server is starting or running.
    pub fn has_active(&self) -> bool {
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().cloned().collect();
        slots
            .iter()
            .any(|slot| matches!(slot.status(), McpStatus::Running | McpStatus::Starting))
    }

    /// The newest `limit` log lines of `id`.
    pub fn logs(&self, id: &str, limit: usize) -> LogTail {
        self.existing(id).map_or_else(
            || LogTail {
                lines: Vec::new(),
                dropped: 0,
            },
            |slot| slot.logs.tail(limit),
        )
    }

    fn enabled_config(&self, id: &str) -> Result<ServerConfig, McpError> {
        let config = self.store.get(id)?;
        if !config.enabled {
            return Err(McpError::new(
                McpErrorKind::Disabled,
                format!("the MCP server `{id}` is disabled; enable it first"),
            ));
        }
        Ok(config)
    }

    /// Start `id` (a no-op when it is already running). Fails when the server is
    /// disabled, cannot be launched or reached, or does not complete `initialize`
    /// and the tool listing in time.
    pub fn start(&self, id: &str, cancel: &CancelToken) -> Result<ServerState, McpError> {
        self.start_server(id, cancel, false)
    }

    /// [`Self::start`]; `on_demand` starts wait out an earlier start that ran
    /// past its caller's deadline.
    fn start_server(
        &self,
        id: &str,
        cancel: &CancelToken,
        on_demand: bool,
    ) -> Result<ServerState, McpError> {
        let config = self.enabled_config(id)?;
        let slot = self.slot(id);
        let _lifecycle = lock(&slot.lifecycle);
        if slot.is_running() {
            return Ok(slot.snapshot());
        }
        if on_demand {
            slot.check_backoff(id)?;
        }
        self.teardown(&slot);
        let generation = {
            let mut state = lock(&slot.state);
            state.generation += 1;
            *state = SlotState {
                status: McpStatus::Starting,
                generation: state.generation,
                ..SlotState::default()
            };
            state.generation
        };
        slot.logs.push(
            LogStream::Client,
            &format!("starting `{id}` ({})", config.transport.as_str()),
        );
        match self.connect(&config, &slot, generation, cancel) {
            Ok((client, listing)) => {
                // Lock order everywhere: lifecycle, then client, then state.
                let mut live = lock(&slot.client);
                let mut state = lock(&slot.state);
                if state.generation != generation {
                    drop(state);
                    drop(live);
                    self.close_client(client);
                    return Err(stopped_while_starting());
                }
                state.status = McpStatus::Running;
                state.error = None;
                state.identity = client.identity().cloned();
                state.process_id = client.process_id().map(|id| id.as_str().to_owned());
                state.tools = listing.tools;
                state.notes = listing.notes;
                state.started_at = Some(Timestamp::now());
                *live = Some(client);
                slot.logs.push(
                    LogStream::Client,
                    &format!("running with {} tools", state.tools.len()),
                );
                Ok(state.snapshot())
            }
            Err((error, caller)) => {
                let mut state = lock(&slot.state);
                if state.generation == generation {
                    // The caller gave up (its run was stopped, or its own deadline
                    // came before the server's): the server did not fail. A run's
                    // Stop lets the next use start it again; after a deadline it
                    // waits a while, so each later caller does not wait as long.
                    let abandoned = caller.is_some()
                        && matches!(error.kind, McpErrorKind::Cancelled | McpErrorKind::Timeout);
                    if abandoned {
                        let retry_after = (caller == Some(CancelReason::DeadlineExceeded))
                            .then(|| Instant::now() + START_BACKOFF);
                        *state = SlotState {
                            generation,
                            retry_after,
                            ..SlotState::default()
                        };
                        slot.logs.push(
                            LogStream::Client,
                            &format!("start abandoned: {}", error.message),
                        );
                        if retry_after.is_some() {
                            slot.logs.push(
                                LogStream::Client,
                                &format!(
                                    "not started on demand again for {} minutes; start it to retry now",
                                    START_BACKOFF.as_secs() / 60
                                ),
                            );
                        }
                    } else {
                        state.interrupter = None;
                        state.fail(error.clone());
                        slot.logs.push(
                            LogStream::Client,
                            &format!("start failed: {}", error.message),
                        );
                    }
                }
                Err(error)
            }
        }
    }

    /// Launch or reach the server, `initialize`, and list its tools. A failure
    /// comes with why the caller's `cancel` had fired when the server's answer
    /// was given up on (closing the connection afterwards can take seconds,
    /// during which the caller's own deadline may pass too).
    fn connect(
        &self,
        config: &ServerConfig,
        slot: &Slot,
        generation: u64,
        cancel: &CancelToken,
    ) -> Result<(McpClient, ToolListing), (McpError, Option<CancelReason>)> {
        let not_cancelled = |error| (error, None);
        let mut client = match config.transport {
            TransportKind::Stdio => {
                let launched = self
                    .host
                    .launch_stdio(config, &slot.logs)
                    .map_err(not_cancelled)?;
                McpClient::stdio(
                    launched.process,
                    Arc::clone(&slot.logs),
                    Scrubber::new(launched.secrets),
                    self.timeouts,
                )
                .map_err(not_cancelled)?
            }
            TransportKind::Http => McpClient::http(
                self.host.http_endpoint(config).map_err(not_cancelled)?,
                Arc::clone(&slot.logs),
                self.timeouts,
            ),
        };
        {
            let mut state = lock(&slot.state);
            if state.generation != generation {
                drop(state);
                self.close_client(client);
                return Err((stopped_while_starting(), None));
            }
            state.interrupter = Some(client.interrupter());
        }
        let outcome = client
            .initialize(cancel)
            .and_then(|_| client.list_tools(cancel));
        match outcome {
            Ok(listing) => Ok((client, listing)),
            Err(error) => {
                let caller = cancel.state();
                self.close_client(client);
                Err((error, caller))
            }
        }
    }

    fn close_client(&self, mut client: McpClient) {
        let interrupter = client.interrupter();
        client.close();
        drop(client);
        if let Some(process) = interrupter.process() {
            self.host.process_exited(process);
        }
    }

    /// Close the live connection, if any (waits for an in-flight request, which
    /// the caller interrupted first).
    fn teardown(&self, slot: &Slot) {
        let client = lock(&slot.client).take();
        if let Some(client) = client {
            self.close_client(client);
        }
    }

    /// [`Self::teardown`] after an interrupt. A request blocked writing to a
    /// server that stopped reading its input cannot notice the interrupt, so
    /// the process tree is killed when the connection stays busy.
    fn teardown_interrupted(&self, slot: &Slot, interrupter: Option<&Interrupter>) {
        let deadline = Instant::now() + BUSY_GRACE;
        let client = loop {
            match slot.client.try_lock() {
                Ok(mut guard) => break guard.take(),
                Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner().take(),
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    if let Some(process) = interrupter.and_then(Interrupter::process) {
                        let _ = process.kill_tree();
                    }
                    break lock(&slot.client).take();
                }
                Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(10)),
            }
        };
        if let Some(client) = client {
            self.close_client(client);
        }
    }

    /// Stop `id` (a no-op when it is not running).
    pub fn stop(&self, id: &str) -> ServerState {
        let Some(slot) = self.existing(id) else {
            return ServerState::default();
        };
        let (was_active, interrupter) = {
            let mut state = lock(&slot.state);
            state.generation += 1;
            let interrupter = state.interrupter.take();
            if let Some(interrupter) = &interrupter {
                interrupter.interrupt();
            }
            (state.status != McpStatus::Stopped, interrupter)
        };
        let _lifecycle = lock_lifecycle(&slot, interrupter.as_ref());
        self.teardown_interrupted(&slot, interrupter.as_ref());
        let mut state = lock(&slot.state);
        let generation = state.generation;
        *state = SlotState {
            generation,
            ..SlotState::default()
        };
        if was_active {
            slot.logs.push(LogStream::Client, "stopped");
        }
        state.snapshot()
    }

    /// Stop and start `id`.
    pub fn restart(&self, id: &str, cancel: &CancelToken) -> Result<ServerState, McpError> {
        self.enabled_config(id)?;
        self.stop(id);
        self.start(id, cancel)
    }

    /// Stop every server; `reason` is logged for each one that was active.
    /// Returns how many were active.
    pub fn stop_all(&self, reason: &str) -> usize {
        let slots: Vec<(String, Arc<Slot>)> = lock(&self.slots)
            .iter()
            .map(|(id, slot)| (id.clone(), Arc::clone(slot)))
            .collect();
        let mut stopped = 0;
        for (id, slot) in slots {
            let idle = lock(&slot.state).status == McpStatus::Stopped;
            if idle && lock(&slot.client).is_none() {
                continue;
            }
            slot.logs
                .push(LogStream::Client, &bounded_redacted(reason, 200));
            self.stop(&id);
            stopped += 1;
        }
        stopped
    }

    /// Stop `id` and forget its logs and scratch home (the server was removed).
    pub fn forget(&self, id: &str) {
        self.stop(id);
        lock(&self.slots).remove(id);
        self.host.forget(id);
    }

    fn ensure_running(
        &self,
        config: &ServerConfig,
        cancel: &CancelToken,
    ) -> Result<Arc<Slot>, McpError> {
        let slot = self.slot(&config.id);
        if !slot.is_running() {
            self.start_server(&config.id, cancel, true)?;
        }
        Ok(slot)
    }

    /// Run `body` on the live connection of `id`, starting it on demand and
    /// refreshing the tool list first when the server announced a change.
    fn with_client<T>(
        &self,
        id: &str,
        cancel: &CancelToken,
        body: impl FnOnce(&mut McpClient, &[McpToolSpec]) -> Result<T, McpError>,
    ) -> Result<T, McpError> {
        let config = self.enabled_config(id)?;
        let slot = self.ensure_running(&config, cancel)?;
        let mut guard = wait_for_client(&slot, id, cancel)?;
        let Some(client) = guard.as_mut() else {
            return Err(McpError::new(
                McpErrorKind::Unavailable,
                format!("the MCP server `{id}` was stopped"),
            ));
        };
        if client.take_tools_changed() {
            match client.list_tools(cancel) {
                Ok(listing) => {
                    let mut state = lock(&slot.state);
                    state.tools = listing.tools;
                    state.notes = listing.notes;
                    slot.logs.push(LogStream::Client, "refreshed the tool list");
                }
                Err(error) => return Err(self.record_failure(&slot, client, error)),
            }
        }
        let tools = lock(&slot.state).tools.clone();
        body(client, &tools).map_err(|error| self.record_failure(&slot, client, error))
    }

    /// Mark the server failed when `error` ended its connection.
    fn record_failure(&self, slot: &Slot, client: &McpClient, error: McpError) -> McpError {
        let fatal = !client.is_alive()
            || matches!(
                error.kind,
                McpErrorKind::SessionExpired | McpErrorKind::NeedsAuth
            );
        if fatal {
            // Why the connection ended (say, the server stopped reading its
            // input), rather than how this request gave up on it.
            let reason = client
                .interrupter()
                .failure()
                .unwrap_or_else(|| error.clone());
            let mut state = lock(&slot.state);
            if state.status == McpStatus::Running {
                slot.logs.push(LogStream::Client, &reason.message);
                state.fail(reason);
            }
        }
        error
    }

    /// The tools of `id`, starting it on demand. While a request (such as a long
    /// `tools/call`) holds the connection, the cached list is the answer; a
    /// pending `tools/list_changed` is picked up by the next request.
    pub fn list_tools(&self, id: &str, cancel: &CancelToken) -> Result<Vec<McpToolSpec>, McpError> {
        self.enabled_config(id)?;
        if let Some(slot) = self.existing(id)
            && slot.is_running()
            && matches!(slot.client.try_lock(), Err(TryLockError::WouldBlock))
        {
            let state = lock(&slot.state);
            if state.status == McpStatus::Running {
                return Ok(state.tools.clone());
            }
        }
        self.with_client(id, cancel, |_, tools| Ok(tools.to_vec()))
    }

    /// Call `tool` on `id` (started on demand). Tool-level failures are results
    /// with `is_error`; protocol, transport, and server errors are `Err`.
    pub fn call_tool(
        &self,
        id: &str,
        tool: &str,
        arguments: Value,
        cancel: &CancelToken,
    ) -> Result<McpToolResult, McpError> {
        self.with_client(id, cancel, |client, tools| {
            if !tools.iter().any(|spec| spec.name == tool) {
                return Err(McpError::new(
                    McpErrorKind::InvalidRequest,
                    format!(
                        "the MCP server `{id}` has no tool `{}`",
                        bounded_redacted(tool, 128)
                    ),
                ));
            }
            client.call_tool(tool, arguments, cancel)
        })
    }

    /// Check that `id` still answers (`ping`).
    pub fn ping(&self, id: &str, cancel: &CancelToken) -> Result<(), McpError> {
        self.with_client(id, cancel, |client, _| client.ping(cancel))
    }

    /// The tools of every enabled server that is running now and offered in
    /// `workspace` (never starts anything).
    pub fn running_toolsets(&self, workspace: Option<&str>) -> Result<Vec<McpToolset>, McpError> {
        let listing = self.store.list()?;
        let mut toolsets = Vec::new();
        for config in listing.servers {
            if !config.enabled || !config.applies_to(workspace) {
                continue;
            }
            let Some(slot) = self.existing(&config.id) else {
                continue;
            };
            let state = slot.snapshot();
            if state.status != McpStatus::Running {
                continue;
            }
            toolsets.push(McpToolset {
                server_id: config.id,
                server_name: config.name,
                auto_approve_tools: config.auto_approve_tools,
                tools: state.tools,
            });
        }
        Ok(toolsets)
    }
}

impl Drop for McpSupervisor {
    fn drop(&mut self) {
        self.stop_all("the MCP supervisor is shutting down");
    }
}

/// The lifecycle lock of `slot`, which a start holds until it returns. A start
/// that stays stuck (on a server that stopped reading its input) after the
/// interrupt is ended by killing the server's tree.
fn lock_lifecycle<'a>(
    slot: &'a Slot,
    interrupter: Option<&Interrupter>,
) -> std::sync::MutexGuard<'a, ()> {
    let deadline = Instant::now() + BUSY_GRACE;
    loop {
        match slot.lifecycle.try_lock() {
            Ok(guard) => return guard,
            Err(TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
            Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                if let Some(process) = interrupter.and_then(Interrupter::process) {
                    let _ = process.kill_tree();
                }
                return lock(&slot.lifecycle);
            }
            Err(TryLockError::WouldBlock) => thread::sleep(BUSY_POLL),
        }
    }
}

/// The connection of `slot`, once no other request holds it; gives up when
/// `cancel` fires, so a run's Stop or deadline is not stuck behind a slow call.
fn wait_for_client<'a>(
    slot: &'a Slot,
    id: &str,
    cancel: &CancelToken,
) -> Result<std::sync::MutexGuard<'a, Option<McpClient>>, McpError> {
    loop {
        match slot.client.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => match cancel.state() {
                Some(CancelReason::Cancelled) => {
                    return Err(McpError::new(
                        McpErrorKind::Cancelled,
                        format!("the MCP request was cancelled while `{id}` was busy"),
                    ));
                }
                Some(CancelReason::DeadlineExceeded) => {
                    return Err(McpError::new(
                        McpErrorKind::Timeout,
                        format!("the MCP server `{id}` stayed busy with another request"),
                    ));
                }
                None => thread::sleep(BUSY_POLL),
            },
        }
    }
}

fn stopped_while_starting() -> McpError {
    McpError::new(
        McpErrorKind::Cancelled,
        "the MCP server was stopped while it was starting",
    )
}
