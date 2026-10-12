//! Daemon lifecycle: compose services, publish the endpoint, serve until a
//! stop request or drive removal, then close connections and remove the
//! endpoint. Also launches a detached daemon, stops one, and recovers stale
//! endpoints without touching other drive data.

mod detach;

use crate::{
    client::{Client, ClientError},
    endpoint::{self, Capability, Endpoint, EndpointError},
    methods::{
        self,
        daemon::{DaemonStatus, STATUS, STOP},
    },
    router::{ClientInfo, Router, RouterError},
    rpc::CONTRACT_VERSION,
    services::{Services, ServicesError},
    transport::{Server, TransportConfig},
};
use aidex_core::ServiceErrorCode;
use aidex_events::{DaemonStopReason, EventKind, EventLog, EventScope};
use serde_json::Value;
use std::{
    fmt, io,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

/// How long `launch` waits for a spawned daemon to answer `daemon.status`.
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
/// How long `stop` waits for the daemon to remove its endpoint.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_TIMEOUT: Duration = Duration::from_millis(250);

fn lifecycle_client() -> ClientInfo {
    ClientInfo::new("aidexd-lifecycle", env!("CARGO_PKG_VERSION"))
}

/// Cooperative stop request shared by the listener and `daemon.stop`.
#[derive(Clone, Debug, Default)]
pub struct ShutdownSignal(Arc<AtomicBool>);

impl ShutdownSignal {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_requested(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Reports whether the selected drive is still present.
pub type DriveProbe = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct DaemonOptions {
    pub transport: TransportConfig,
    /// Listener poll interval for stop requests and drive presence.
    pub poll_interval: Duration,
    /// Defaults to `root.is_dir()`; a `false` result stops the daemon.
    pub drive_present: DriveProbe,
    /// Updater trust store and self-test; `None` uses the built-in trust store and
    /// the process self-test (tests inject run-time keys here).
    pub updater: Option<aidex_updater::UpdaterConfig>,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            transport: TransportConfig::default(),
            poll_interval: Duration::from_millis(50),
            drive_present: Arc::new(|root: &Path| root.is_dir()),
            updater: None,
        }
    }
}

impl fmt::Debug for DaemonOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DaemonOptions")
            .field("transport", &self.transport)
            .field("poll_interval", &self.poll_interval)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// `daemon.stop` or [`DaemonHandle::stop`].
    Requested,
    /// The selected drive disappeared.
    DriveRemoved,
}

/// Endpoint recovery outcome before a start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    NoEndpoint,
    RemovedStale,
}

#[derive(Debug)]
pub enum DaemonError {
    DriveUnavailable(PathBuf),
    AlreadyRunning {
        pid: u32,
    },
    /// Something answers on the endpoint's port but cannot be authenticated.
    EndpointBusy(String),
    Endpoint(EndpointError),
    Services(ServicesError),
    Router(RouterError),
    Client(ClientError),
    Io {
        context: &'static str,
        source: io::Error,
    },
    Launch(String),
    NotReady(String),
    StopTimeout(Duration),
}

impl DaemonError {
    pub fn code(&self) -> ServiceErrorCode {
        match self {
            Self::DriveUnavailable(_)
            | Self::Io { .. }
            | Self::Launch(_)
            | Self::NotReady(_)
            | Self::StopTimeout(_) => ServiceErrorCode::Unavailable,
            Self::AlreadyRunning { .. } | Self::EndpointBusy(_) => ServiceErrorCode::Conflict,
            Self::Endpoint(error) => error.code(),
            Self::Services(error) => error.code(),
            Self::Router(_) => ServiceErrorCode::Internal,
            Self::Client(error) => error.code(),
        }
    }
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DriveUnavailable(root) => {
                write!(f, "selected drive is unavailable: {}", root.display())
            }
            Self::AlreadyRunning { pid } => write!(f, "daemon is already running (pid {pid})"),
            Self::EndpointBusy(message) | Self::Launch(message) | Self::NotReady(message) => {
                f.write_str(message)
            }
            Self::Endpoint(error) => error.fmt(f),
            Self::Services(error) => error.fmt(f),
            Self::Router(error) => write!(f, "invalid method registration: {error}"),
            Self::Client(error) => error.fmt(f),
            Self::Io { context, source } => write!(f, "{context}: {source}"),
            Self::StopTimeout(timeout) => write!(
                f,
                "daemon acknowledged stop but did not exit within {} s",
                timeout.as_secs()
            ),
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Endpoint(error) => Some(error),
            Self::Services(error) => Some(error),
            Self::Router(error) => Some(error),
            Self::Client(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// A composed daemon that has not started listening yet.
pub struct Daemon {
    root: PathBuf,
    services: Arc<Services>,
    router: Router,
    options: DaemonOptions,
    shutdown: ShutdownSignal,
}

impl Daemon {
    pub fn new(root: &Path) -> Result<Self, DaemonError> {
        Self::with_options(root, DaemonOptions::default())
    }

    /// Open services on `root` and register every built-in and domain method.
    pub fn with_options(root: &Path, options: DaemonOptions) -> Result<Self, DaemonError> {
        if !root.is_dir() {
            return Err(DaemonError::DriveUnavailable(root.to_path_buf()));
        }
        let shutdown = ShutdownSignal::new();
        let services = Arc::new(
            Services::open_with(root, shutdown.clone(), options.updater.clone())
                .map_err(DaemonError::Services)?,
        );
        let mut router = Router::new();
        methods::register_all(&mut router, &services).map_err(DaemonError::Router)?;
        Ok(Self {
            root: root.to_path_buf(),
            services,
            router,
            options,
            shutdown,
        })
    }

    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    /// Extra registrations, e.g. test-only methods.
    pub fn router_mut(&mut self) -> &mut Router {
        &mut self.router
    }

    /// Bind loopback, publish the endpoint (failing if one exists), and serve
    /// on a background thread.
    pub fn start(self) -> Result<DaemonHandle, DaemonError> {
        let token = Capability::generate().map_err(DaemonError::Endpoint)?;
        let server = Server::bind(
            self.router,
            token.clone(),
            self.options.transport.clone(),
            self.shutdown.clone(),
        )
        .map_err(|source| DaemonError::Io {
            context: "cannot bind daemon endpoint",
            source,
        })?;
        let port = server.port().map_err(|source| DaemonError::Io {
            context: "cannot read daemon endpoint port",
            source,
        })?;
        let endpoint = Endpoint {
            version: CONTRACT_VERSION,
            pid: self.services.pid(),
            port,
            token,
        };
        endpoint::publish(&self.root, &endpoint).map_err(DaemonError::Endpoint)?;
        // This daemon now owns the drive: finish or roll back an interrupted update
        // before serving (outcome and failures are events and `update.status`).
        self.services.recover_updates();
        let events = self.services.events().clone();
        let started = EventKind::DaemonStarted {
            pid: self.services.pid(),
            version: env!("CARGO_PKG_VERSION").into(),
        };
        if let Err(error) = events.append(EventScope::global(), started) {
            let _ = endpoint::remove(&self.root);
            return Err(DaemonError::Services(ServicesError::new(
                "events",
                error.code(),
                error.to_string(),
            )));
        }
        let broker = Arc::clone(self.services.broker());
        let native = self.services.native().clone();
        let inference = self.services.inference().clone();
        drop(self.services);
        let root = self.root.clone();
        let options = self.options;
        let thread = thread::Builder::new()
            .name("aidexd-accept".into())
            .spawn(move || {
                // The saved model is verified and loaded on its own worker;
                // accepting IPC never waits for GGUF hashing or llama.cpp readiness.
                let _ = inference.restore_saved();
                let outcome = server.run(
                    &root,
                    options.poll_interval,
                    &options.drive_present,
                    |outcome| {
                        native.shutdown();
                        let _ = inference.shutdown();
                        record_stopping(&events, outcome);
                        // No tool process outlives the daemon (or the drive).
                        broker.shutdown_processes(matches!(outcome, Ok(StopReason::DriveRemoved)));
                    },
                );
                let _ = inference.shutdown();
                let cleanup = endpoint::remove(&root);
                match (outcome, cleanup) {
                    (Ok(reason), Ok(())) => Ok(reason),
                    // A removed drive cannot be cleaned; recovery handles the leftover file.
                    (Ok(reason), Err(_)) if !root.is_dir() => Ok(reason),
                    (Ok(_), Err(error)) => Err(DaemonError::Endpoint(error)),
                    (Err(source), _) => Err(DaemonError::Io {
                        context: "daemon listener failed",
                        source,
                    }),
                }
            });
        match thread {
            Ok(thread) => Ok(DaemonHandle {
                endpoint,
                root: self.root,
                shutdown: self.shutdown,
                thread: Some(thread),
            }),
            Err(source) => {
                let _ = endpoint::remove(&self.root);
                Err(DaemonError::Io {
                    context: "cannot start daemon listener thread",
                    source,
                })
            }
        }
    }
}

/// Record `daemon.stopping` before connections close, so live subscribers may still
/// see it. Best effort: a removed drive cannot record anything.
fn record_stopping(events: &EventLog, outcome: &io::Result<StopReason>) {
    let reason = match outcome {
        Ok(StopReason::DriveRemoved) => DaemonStopReason::DriveRemoved,
        Ok(StopReason::Requested) | Err(_) => DaemonStopReason::Requested,
    };
    let _ = events.append(EventScope::global(), EventKind::DaemonStopping { reason });
}

/// A running daemon. Dropping it requests shutdown and waits for it.
pub struct DaemonHandle {
    endpoint: Endpoint,
    root: PathBuf,
    shutdown: ShutdownSignal,
    thread: Option<JoinHandle<Result<StopReason, DaemonError>>>,
}

impl DaemonHandle {
    /// The published endpoint, including its capability (keep it in memory only).
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn drive_root(&self) -> &Path {
        &self.root
    }

    pub fn shutdown_signal(&self) -> &ShutdownSignal {
        &self.shutdown
    }

    /// Request a graceful stop; [`Self::wait`] for completion.
    pub fn stop(&self) {
        self.shutdown.request();
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub fn wait(mut self) -> Result<StopReason, DaemonError> {
        match self.thread.take() {
            Some(thread) => thread.join().unwrap_or_else(|_| {
                Err(DaemonError::Launch(
                    "daemon listener thread panicked".into(),
                ))
            }),
            None => Ok(StopReason::Requested),
        }
    }
}

impl Drop for DaemonHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.shutdown.request();
            let _ = thread.join();
        }
    }
}

impl fmt::Debug for DaemonHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DaemonHandle")
            .field("endpoint", &self.endpoint)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

/// Serve in the foreground (the `aidexd run` entry point).
pub fn serve(root: &Path) -> Result<StopReason, DaemonError> {
    let handle = Daemon::new(root)?.start()?;
    println!("ready");
    handle.wait()
}

/// Refuse a live or unverifiable endpoint; remove only one proven unreachable.
pub fn recover_stale_endpoint(root: &Path) -> Result<Recovery, DaemonError> {
    let Some(summary) = endpoint::summarize(root).map_err(|error| {
        DaemonError::EndpointBusy(format!(
            "cannot safely recover daemon endpoint {}: {error}; delete it only if no daemon is running",
            endpoint::state_path(root).display()
        ))
    })?
    else {
        return Ok(Recovery::NoEndpoint);
    };
    if let Some(port) = summary.port.filter(|port| *port != 0) {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        if TcpStream::connect_timeout(&address, PROBE_TIMEOUT).is_ok() {
            if summary.version == CONTRACT_VERSION
                && let Ok(endpoint) = endpoint::read(root)
                && Client::connect_endpoint(&endpoint, lifecycle_client()).is_ok()
            {
                return Err(DaemonError::AlreadyRunning { pid: endpoint.pid });
            }
            let detail = if summary.version == CONTRACT_VERSION {
                "it is responding but did not accept this user's capability".to_owned()
            } else {
                format!(
                    "an IPC contract-v{} daemon appears to be running; stop it with the CLI that started it",
                    summary.version
                )
            };
            return Err(DaemonError::EndpointBusy(format!(
                "daemon endpoint {} is in use: {detail}",
                endpoint::state_path(root).display()
            )));
        }
    }
    endpoint::remove(root).map_err(DaemonError::Endpoint)?;
    Ok(Recovery::RemovedStale)
}

/// Recover a stale endpoint, spawn `<executable> run --drive <root>` detached,
/// and wait until it answers `daemon.status`.
pub fn launch(root: &Path, executable: &Path) -> Result<DaemonStatus, DaemonError> {
    if !root.is_dir() {
        return Err(DaemonError::DriveUnavailable(root.to_path_buf()));
    }
    recover_stale_endpoint(root)?;
    let mut spawned = detach::spawn(executable, root).map_err(|error| {
        DaemonError::Launch(format!("cannot launch `{}`: {error}", executable.display()))
    })?;
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut last_error = String::from("endpoint not published");
    loop {
        if let Some(status) = spawned.exited() {
            return Err(DaemonError::Launch(format!(
                "daemon exited during startup ({status}); run `aidexd run --drive {}` to see why",
                root.display()
            )));
        }
        if endpoint::state_path(root).is_file() {
            match Client::connect_with(root, lifecycle_client())
                .and_then(|mut client| client.call_as::<DaemonStatus>(STATUS, Value::Null))
            {
                Ok(status) => return Ok(status),
                Err(error) => last_error = error.to_string(),
            }
        }
        if Instant::now() >= deadline {
            spawned.kill();
            return Err(DaemonError::NotReady(format!(
                "daemon did not become ready within {} s ({last_error})",
                STARTUP_TIMEOUT.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Ask the daemon to stop and wait until it removes its endpoint.
pub fn stop(root: &Path, timeout: Duration) -> Result<(), DaemonError> {
    let mut client = Client::connect_with(root, lifecycle_client()).map_err(DaemonError::Client)?;
    client
        .call(STOP, Value::Null)
        .map_err(DaemonError::Client)?;
    drop(client);
    let deadline = Instant::now() + timeout;
    while endpoint::state_path(root).exists() {
        if Instant::now() >= deadline {
            return Err(DaemonError::StopTimeout(timeout));
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
