# `aidexd`

The local AiDEX daemon: it composes backend services for one drive and exposes
them as JSON-RPC 2.0 methods over an authenticated loopback connection (IPC
contract version 2). The crate also ships the blocking client the `aidex` CLI
uses. Wire format, handshake, error codes, and naming rules:
[`protocol/commands/README.md`](../../protocol/commands/README.md).

## Layout

| Module                | Responsibility                                                                                                                                                                                                                                                                 |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `endpoint`            | `daemon.json` endpoint file, `Capability` (redacted `Debug`, constant-time `matches`), DPAPI/0600 protection, symlink-checked `.aidex/runtime`                                                                                                                                 |
| `rpc`, `errors`       | JSON-RPC message model, params helpers, the stable `ErrorCode` table, `RpcError`                                                                                                                                                                                               |
| `handshake`           | `aidex.handshake` params/result and server-side validation                                                                                                                                                                                                                     |
| `router`              | `Router`, `CallContext`, `ClientInfo`                                                                                                                                                                                                                                          |
| `stream`              | `NotificationSink` for subscription methods                                                                                                                                                                                                                                    |
| `transport` (private) | listener, per-connection reader/writer threads, bounded outbox, shutdown                                                                                                                                                                                                       |
| `services`            | `Services`, built once and shared with handlers as `Arc<Services>`                                                                                                                                                                                                             |
| `methods/<domain>`    | JSON-RPC adapters: `daemon.*` (status/ping/stop), `events.*` (replay/latest/subscribe), `vault.*` (status/init/unlock/lock/secret.set/secret.list/secret.delete/passphrase.change), `workspace.*` (registry, discovery, ephemeral review/apply/discard over `aidex-workspace`) |
| `lifecycle`           | `Daemon`, `DaemonHandle`, `serve`, `launch`, `stop`, stale-endpoint recovery                                                                                                                                                                                                   |
| `client`              | `Client`, `Subscription`, `ClientError`                                                                                                                                                                                                                                        |

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

Embedding and tests: `Daemon::with_options(root, DaemonOptions { transport, poll_interval, drive_present, updater })`
(`updater: Option<aidex_updater::UpdaterConfig>` injects a trust store and self-test; `None` uses the built-in trust store),
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
  On Windows, clients check loopback reachability before unlocking the temporary
  DPAPI credential. Moving the drive to another computer therefore lets startup
  replace an unreachable endpoint with that computer's credential, preserving the
  portable vault and history. Reachable peers still require authentication and
  are never replaced on a credential error.
- Bounded everything: 1 MiB lines, 64 connections, 1024 queued messages /
  16 MiB per connection, 64 subscriptions per connection, 5 s handshake.
  Slow consumers are disconnected instead of buffering without limit.
- The daemon stops (closing connections and removing its endpoint) on
  `daemon.stop` or when the drive disappears.
- Updates: right after publishing its endpoint (so this daemon owns the drive)
  and before serving, the daemon runs `Updater::recover`, finishing or rolling
  back an interrupted update; `update.apply`/`update.rollback` switch
  `app/current.json` but never replace the running binaries (the new version
  starts on the next launch). `aidexd --version` prints the version (used by the
  update self-test).

## Limitations

- Methods: `daemon.status|ping|stop`, `events.replay|latest|subscribe`, `workspace.*`,
  `vault.*`, `model.*`, `provider.*`, `network.*`, `runtime.*`, `settings.get|set`,
  `model.preferences.get|set`, `web.settings.get|set`, `web.providers|test|open`,
  and `update.status|check|apply|rollback` exist, plus the broker's
  `tool.list|invoke|continue`, `broker.approval.list|show|resolve|revoke`,
  `policy.show|profile.set|reload|explain`, and `emergency.stop|resume`; additional
  domains are future work.
- `Services::broker()` returns the `Arc<aidex_broker::Broker>` composed over the drive
  (approvals in `private_sessions/approvals.sqlite3`, settings in
  `private_config/policy.toml`, documents in `policies/`, sandbox scratch in
  `tmp/broker/`, the helper `aidex-sandbox-exec` next to the daemon, the drive vault).
  A background thread expires due approvals every 500 ms; `session.cancel` also
  cancels the session's calls and approvals; daemon stop and drive removal terminate
  every managed process. Every IPC client acts as the local user. Remote provider
  calls pass the broker's `ModelEgress` gate (offline switch, emergency stop,
  `model.invoke` policy); vault-backed provider credentials go through it too
  (`secret.use`, audited 30 s handles), while `aidex-auth` account sign-ins resolve
  through `AuthService` (`BrokeredCredentials`).
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
  `services/vault_lock.rs` observes every end of the unlocked session (`vault.lock`,
  emergency stop, idle auto-lock, or a session dropped after a `conflict` or an
  uncertain header commit) and runs the lock cleanup once: native runs cancelled,
  attachment uploads aborted, official runtime logins cancelled, external runtime
  processes terminated, `vault.locked` recorded. `services/mcp.rs` stops MCP
  servers on the same notice (its polling watchdog stays as a backstop). The next
  unlock waits until that cleanup ran, so it never reaches the new session;
  `vault.init`/`vault.unlock` then call `NativeEngine::reconcile_after_unlock` (best
  effort) to settle runs the lock left without a final state as `interrupted`.
  `vault.secret.set|delete` refuse the records AiDEX owns
  (`methods::vault::managed_secret`) with `policy_denied`/`managed_secret`, and
  `vault.secret.list` names them in `managed`. A keep-alive thread
  (`services/keep_alive.rs`) counts executing agent work as vault use: while
  `NativeEngine::has_executing()` (a run's thread works on it, native or
  external), it calls `VaultService::keep_alive` at least four times per idle
  period (at most once a second), so the idle auto-lock follows the end of the
  work. It reads in-memory engine state only, never the vault file. Work that
  waits for a person does not count: a run waiting for an approval (native, or
  an external runtime paused with its vendor process) and a pending official
  login. When the user walks away the idle auto-lock happens as usual, the lock
  cleanup cancels that work, and the next unlock settles the waiting run as
  `interrupted`, which `run.resume` continues as before. A native parent
  waiting for its active children still executes, so a child waiting for an
  approval keeps the vault awake through it until that wait ends (at most ten
  minutes). The one
  exception: `vault.unlock` while a session is open whose `vault.json` changed on
  disk replaces that session without a notice or cleanup, because the vault never
  locks. Errors that dropped the session carry `data.session_dropped: true`.
- DTOs for daemon methods live in this crate (`methods::*`), not in the
  generated `aidex-core` contract schema yet.
- No per-client authorization beyond the bearer capability; any process of the
  same OS user that can read the endpoint file can connect.
- See `src/transport/README.md` and `src/lifecycle/README.md` for transport and
  lifecycle specifics.

## Provider/model methods

`Services::models()` exposes `ModelServices` with a shared `ProviderRegistry`,
model router, offline switch and vault credential resolver. Model chat stays an
internal `ModelProvider` API; desktop messages use the separate `conversation.*`
and `agent.*` services rather than a raw provider-chat RPC.

| Method                  | Parameters                                                                                                   | Result                                                                                                                                         |
| ----------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `provider.presets`      | none                                                                                                         | `{presets: [ProviderPreset]}`; stable IDs, protocol kind, endpoint defaults, capabilities, icon, category, auth methods, and console/docs links |
| `provider.add`          | `{id, kind?, base_url, secret_name?, locality?, discover_models?, models?, display_name?, preset_id?, enabled?}` | `ProviderEntry`                                                                                                                                |
| `provider.update`       | same full replacement configuration as add                                                                   | `ProviderEntry`; validates/persists before replacing the instance; absent metadata fields are kept, `null` clears them                         |
| `provider.patch`        | `{id, display_name?: string\|null, enabled?}`                                                                | `ProviderEntry`; renames or enables/disables a configured account                                                                              |
| `provider.open_link`    | `{preset_id, link: "api_key"\|"docs"}`                                                                       | `{opened: true}`; opens only the HTTPS link stored in that preset                                                                              |
| `provider.remove`       | `{id}`                                                                                                       | `{removed}`; clears a default pointing at this account and signs a ChatGPT account out (best effort)                                          |
| `provider.health`       | `{id}`                                                                                                       | `{provider_id, status, error?, message?, latency_ms?, checked_at}`                                                                             |
| `model.providers`       | none                                                                                                         | `{providers: [ProviderEntry]}`, disabled accounts included, with account metadata and the last catalog status                                  |
| `model.list`            | none, or `{refresh?, provider_id?}`                                                                          | `{models, failures, default_model, network}`; `refresh` drops cached listings, `provider_id` lists one provider                                |
| `model.route`           | `{task, privacy?, required_context?, needs_tools?, needs_json_schema?, secret_heavy?, model?}`               | `{decision, model, network, failures}`                                                                                                         |
| `model.use`             | `{provider_id, model_id}`                                                                                    | `{default_model, model}`; validates then persists                                                                                              |
| `model.default`         | none                                                                                                         | `{default_model}`                                                                                                                              |
| `model.preferences.get` | none                                                                                                         | `{favorite_models, hidden_models}`                                                                                                             |
| `model.preferences.set` | `{favorite_models?, hidden_models?}`                                                                         | the saved record; omitted lists are unchanged                                                                                                  |
| `network.offline`       | `{enabled}`                                                                                                  | `{offline, network}`; on also cancels runtime runs and logins                                                                                  |
| `network.status`        | none                                                                                                         | `{offline, network}`                                                                                                                           |

Provider kinds are `openai-compatible`, `openai-responses`, `anthropic`, `gemini`,
`azure-openai`, and `bedrock`. Runtime local providers also use `llama-cpp` or
`fake`, which cannot be configured through `provider.add`. `secret_name` names a
vault secret with consumer `provider:<id>`; keys are set through `vault.secret.set`.
Credential handles are short-lived, resolved and revoked per call. Health reports
`auth_required` for missing/locked/rejected credentials. API-key provider presets
retain `oauth_supported: false` (only `openai` offers ChatGPT sign-in); supported
ChatGPT account acquisition is owned by the shared `aidex-auth` service and exposed
through `auth.*`. Its encrypted accounts register OpenAI Responses provider
instances after identity verification. A new ChatGPT account records the `openai`
preset; a reconnect reasserts only the kind, endpoint, and credential name, keeping
the account's declared models, discovery flag, and account metadata.

Account metadata (display name, preset id, enabled flag) lives in the registry's
`private_config/provider-accounts.toml` sidecar so `providers.toml` keeps the
schema an older daemon loads after `update.rollback`. Disabled accounts stay in
`model.providers` but are skipped by `model.list`, cannot hold the default model,
and are refused when a conversation runs on them. `provider.open_link` resolves its
URL from the preset table only and opens it through `aidex_auth::open_in_browser`
(network gate first, HTTPS only, fixed command text with the URL as environment
data); handler tests exercise `open_link_url` and the offline refusal, never a
browser.

Model picker favorites and hidden models are a separate vault record
(`aidex_preferences::ModelPreferencesService`, secret `aidex-model-preferences`),
registered next to `settings.*` in `methods/settings.rs`; `AppPreferences` is
unchanged so an older daemon still reads `aidex-preferences`.

### Web search

`Services::web()` returns `WebServices` (`services/web.rs`): the drive's
`private_config/web.toml` (`aidex-web`, a new closed-schema file, so older
daemons are unaffected) and `WebClient`s whose egress check refuses when the
offline switch is on or an emergency stop is in effect, then evaluates a
`network.request` intent for the destination host under the broker's settings
and policy documents (approval verdicts refuse; every decision is a
`policy.decision` event). Provider keys are released only after that check,
through a `secret.use` evaluation and a 30-second vault handle for
`process:web` (`secret.granted`/`secret.revoked`). `methods/web.rs` serves
`web.settings.get|set`, `web.providers`, `web.test` (one search with the stored
or an unsaved patched configuration), and `web.open` (strict URL validation,
then `aidex_auth::open_in_browser`). Handler tests use a loopback SearXNG
fixture and never open a browser. Harness tools (`web.search`, `web.fetch`)
are not wired yet.

### External runtimes

`Services::runtime()` returns the `aidex_adapters::RuntimeManager` (Codex, Claude
Code, Cursor, Grok, OpenCode, Antigravity, Pi, Oh My Pi, Muse, and ACP drivers) or
why it failed to open; a failure never stops the daemon, and `runtime.*` then
reports `unavailable` (`data.reason = "runtime_unavailable"`). `methods/runtime.rs`
serves `runtime.catalog|list|add|update|config.save|remove|config.get|health|models|install`,
`runtime.auth.start|status|cancel`, and the managed accounts
`runtime.account.status`, `runtime.profile.import`, and
`runtime.login.start|status|cancel|logout` (Codex only; errors carry the adapter's
code as `data.reason`, and results never carry credential bytes, file contents, or
CLI output). Import, login start, and logout run like configuration changes
(`conflict` while an external run is active); `runtime.login.status` of a
connected login re-syncs the engine best effort, so it never fails because a run
is active. The manager's gate (`services/runtime.rs`) admits vendor processes and
logins only while the drive is online, the vault is unlocked, and no emergency
stop is in effect; adapters stop running processes and logins within about 25 ms
once it closes. `emergency.stop` also cancels every run, pending login, and vendor
process; `network.offline {enabled: true}` cancels the active runs of runtime
conversations and pending logins; daemon shutdown calls `RuntimeManager::shutdown`.
While the gate is closed the account methods start no official CLI:
`runtime.account.status` and the managed-account path of `runtime.health`
answer from the status the CLI last confirmed for the runtime's profile (kept
in memory by `methods/runtime.rs`, never credential text), and import, login
start, and logout are refused with `offline_refused`.
The configuration is sealed with a vault
key, so the native engine only learns the configured runtimes through
`sync_runtimes`, which loads the enabled executors when the vault is unlocked (a
no-op while it is locked, and on a drive that never saved a runtime
configuration, so unlocking never creates the runtime encryption key). It runs after `vault.init`/`vault.unlock`, on
`runtime.list`, and before `conversation.create`, `message.send`, `agent.start`,
`run.resume`, and `approval.respond` of a runtime conversation whose runtime the
engine does not have yet (when an active external run refuses the swap, just that
runtime is added); adding, updating, or removing a runtime swaps the engine's
executors directly. `agent.configuration.set` that changes the runtime, project,
mode, permission mode, or instructions drops the conversation's and its runs'
`runtime_ref`, and a run's `has_runtime_ref` reports only a reference
`run.resume` can continue (`aidex_native_sessions::is_resumable_reference`). Runtime
conversations record `provider_id: "runtime"`; `model_id: "default"`
(`aidex_native_sessions::RUNTIME_DEFAULT_MODEL`) makes the harness send no model so
the runtime uses its own default.

### Standalone desktop conversations

`conversation.create` with omitted or null `workspace_id` creates a project-free chat, even when `workspace.select` has a current project. Explicit references must resolve to registered, available workspace handles. `agent.configuration.set` performs the same validation before persistence and preserves message history. Plan/Agent execution requires a selected project; ordinary Chat does not expose agent tools.

### Chat attachments and forks

`methods/attachments.rs` adapts the `aidex-native-sessions` attachment store: chunked uploads (`attachment.upload.begin|chunk|commit|abort`), `attachment.from_workspace`, `attachment.list|get|read|delete|exclude`. The daemon never reads a host path for an attachment except a registered project's file, resolved through its `ConfinedRoot`: the requested path and the resolved link target must both pass the credential-name checks (`aidex_tools::looks_sensitive`, `aidex_context::exclude::is_secret_path`), the opened handle must be a regular file of at most 16 MiB, and the store reads it through a bounded, zeroizing reader. Upload chunks are decoded from base64 into zeroizing buffers and sealed by the store as they arrive; nothing keeps the record key between calls. Every lock (`vault.lock`, `emergency.stop {lock_vault: true}`, the idle auto-lock, or a dropped session) aborts every upload in flight (deleting its partial file) through the vault lock cleanup, and a chunk or commit after any lock is `vault_locked`. Store refusals (`<reason>: <detail>`) become `invalid_params` with `data.reason` (`busy` is `unavailable`); messages never carry a name, a path, or content, and no attachment event is recorded.

`message.send` and `agent.start` pass `attachments` to the harness in `RunOptions`; the daemon itself refuses more than 20 ids and attachments with `parent_run_id` (`attachment_unavailable`), and maps the harness's `attachments_unsupported` and `context_too_large_for_runtime` refusals to `invalid_params`. `conversation.get` and session `history.list` add `attachments` to user messages from `SessionStore::message_attachments` (the attachments table is authoritative; `Run::attachments` is only a cache), and runs show the `mode`, `permission_mode` and `workspace_id` they began with. `conversation.fork` (`methods/native_fork.rs`) validates the new configuration like `conversation.create`, then calls `SessionStore::fork` and returns the new conversation's bounded view. `model.list` entries carry `input: {images, pdf}` from `ProviderRegistry::model_input`, computed per listing and never persisted.
