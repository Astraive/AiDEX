# `aidexd`

The local AiDEX daemon: it composes backend services for one drive and exposes
them as JSON-RPC 2.0 methods over an authenticated loopback connection (IPC
contract version 2). The crate also ships the blocking client the `aidex` CLI
uses. Wire format, handshake, error codes, and naming rules:
[`protocol/commands/README.md`](../../protocol/commands/README.md).

## Layout

| Module | Responsibility |
| --- | --- |
| `endpoint` | `daemon.json` endpoint file, `Capability` (redacted `Debug`, constant-time `matches`), DPAPI/0600 protection, symlink-checked `.aidex/runtime` |
| `rpc`, `errors` | JSON-RPC message model, params helpers, the stable `ErrorCode` table, `RpcError` |
| `handshake` | `aidex.handshake` params/result and server-side validation |
| `router` | `Router`, `CallContext`, `ClientInfo` |
| `stream` | `NotificationSink` for subscription methods |
| `transport` (private) | listener, per-connection reader/writer threads, bounded outbox, shutdown |
| `services` | `Services`, built once and shared with handlers as `Arc<Services>` |
| `methods/<domain>` | JSON-RPC adapters: `daemon.*` (status/ping/stop), `events.*` (replay/latest/subscribe), `vault.*` (status/init/unlock/lock/secret.set/secret.list/secret.delete/passphrase.change), `workspace.*` (registry, discovery, ephemeral review/apply/discard over `aidex-workspace`) |
| `lifecycle` | `Daemon`, `DaemonHandle`, `serve`, `launch`, `stop`, stale-endpoint recovery |
| `client` | `Client`, `Subscription`, `ClientError` |

## API

```rust
// Server side (inside aidexd)
router.register("domain.verb", |ctx: &CallContext, params: Value| -> Result<Value, RpcError> { .. })?;
router.register_stream("domain.subscribe", |ctx, params, sink: NotificationSink| -> Result<Value, RpcError> {
    std::thread::spawn(move || while sink.notify("domain.event", json!({..})).is_ok() { .. });
    Ok(json!({ "cursor": 0 }))          // becomes the subscribe result's `data`
})?;

// Client side (CLI, tests, other frontends)
let mut client = Client::connect(drive_root, "aidex-cli")?;
let status: DaemonStatus = client.call_as("daemon.status", Value::Null)?;
let mut events = client.subscribe("domain.subscribe", json!({}))?;
while let Some(notification) = events.next_timeout(Duration::from_secs(1))? { .. }
```

`ClientError` is `Unavailable | Unauthorized | Incompatible { message, supported } |
Rpc { code, message, data } | Io | Protocol`; `rpc_kind()` maps `Rpc` codes back
to `ErrorCode`. Every error type has a stable `code() -> aidex_core::ServiceErrorCode`.

Embedding and tests: `Daemon::with_options(root, DaemonOptions { transport, poll_interval, drive_present })`,
`daemon.router_mut()` for extra methods, `daemon.start()` → `DaemonHandle`
(`endpoint()`, `stop()`, `wait()`; dropping it stops the daemon), and
`Client::connect_endpoint(handle.endpoint(), ClientInfo::new(..))` to skip the
endpoint file. `Router::call` and `CallContext::detached` unit-test plain
handlers without sockets.

## Adding a domain service

1. **Service** — in `services.rs`, add a private field (e.g. `sessions: SessionStore`),
   open it in `Services::open` mapping failures with
   `ServicesError::new("sessions", error.code(), error.to_string())`, and add an
   accessor `pub fn sessions(&self) -> &SessionStore`. Locate drive files through
   `services.paths()` (`aidex_drive::DrivePaths`; the daemon only runs on an initialized
   schema-v2 drive) and create directories with `paths().ensure_dir(..)`.
2. **Methods** — create `methods/sessions.rs` with typed `#[serde(deny_unknown_fields)]`
   params/results, `pub(crate) fn register(router: &mut Router, services: &Arc<Services>) -> Result<(), RouterError>`
   that clones the `Arc` into each handler, decodes params with `rpc::parse_params`,
   maps domain errors with `RpcError::service(error.code(), error.to_string())`
   (or a specific `ErrorCode` such as `PolicyDenied`, `VaultLocked`), and returns
   `rpc::to_result(&dto)`. Streams use `register_stream("sessions.subscribe", ..)`
   and push `sessions.event` notifications.
3. **Wire it** — add `pub mod sessions;` and `sessions::register(router, services)?;`
   to `methods/mod.rs`.
4. **CLI** — create `crates/aidex-cli/src/commands/sessions.rs` with a clap
   `Subcommand` enum and `pub fn run(command, output: Output) -> Result<Exit, CliError>`
   that calls `commands::daemon::connect(drive)?` and `client.call_as(..)`; add
   `pub mod sessions;` plus a dispatch arm in `commands/mod.rs`, a variant in
   `cli::Commands`, and its label in `Commands::label`.
5. **Tests** — unit-test handlers with `Router::call`; test wire behaviour with an
   in-process `Daemon` (see `tests/common`).

## Guarantees

- Binds IPv4 loopback only; a 256-bit OS-random capability is required in the
  first message, compared in constant time, protected at rest (DPAPI on
  Windows, 0600 on Unix), never logged, and redacted from `Debug`.
- One daemon per drive (`create_new` endpoint); stale endpoints are removed
  only when provably unreachable; other drive data is never touched.
- Bounded everything: 1 MiB lines, 64 connections, 1024 queued messages /
  16 MiB per connection, 64 subscriptions per connection, 5 s handshake.
  Slow consumers are disconnected instead of buffering without limit.
- The daemon stops (closing connections and removing its endpoint) on
  `daemon.stop` or when the drive disappears.

## Limitations

- Methods: `daemon.status|ping|stop`, `events.replay|latest|subscribe`, `workspace.*`,
  and `vault.*` exist; every other domain is future work.
- The event log is `DrivePaths::sessions()/events.sqlite3` (100 000 events, 90
  days). PR #40's `.aidex/runtime/events.sqlite` is superseded: it is imported once,
  read-only, into an empty new log and then left untouched (safe to delete).
  The daemon records `daemon.started` once its endpoint is published and
  `daemon.stopping` before it closes connections (best effort; live subscribers
  may only see it on replay). Each `events.subscribe` stream uses one thread.
  The stream sends envelopes as `events.event` and consumed durable progress as
  `events.watermark {cursor}`. Watermarks follow all matching events they cover,
  including filtered history and live queue catch-up; clients acknowledge them
  after processing preceding events. They are not events or periodic heartbeats,
  and ephemeral events and the initial `latest_cursor` do not acknowledge progress.
- `Services::vault()` returns the drive's `Arc<aidex_vault::VaultService>` (vault in
  `DrivePaths::vault()`, default 30 min idle auto-lock). A damaged vault does not stop
  the daemon; `vault.status` reports it as `unavailable`. Secret handles are issued and
  resolved through `VaultService` in-process (for the broker), never over IPC.
- DTOs for daemon methods live in this crate (`methods::*`), not in the
  generated `aidex-core` contract schema yet.
- No per-client authorization beyond the bearer capability; any process of the
  same OS user that can read the endpoint file can connect.
- See `src/transport/README.md` and `src/lifecycle/README.md` for transport and
  lifecycle specifics.
