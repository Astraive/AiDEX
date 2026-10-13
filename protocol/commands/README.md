# Command contracts: daemon IPC (contract version 2)

Local frontends (the `aidex` CLI today; desktop and mobile bridges later) reach
backend services through `aidexd` using JSON-RPC 2.0. The Rust types in
`crates/aidexd` (`rpc`, `errors`, `handshake`, `methods::*`) are canonical for
this transport until the method DTOs move into the generated `aidex-core`
contract. Sensitive operations must retain broker and approval semantics from
the security specification; possession of the endpoint capability is not an
authorization decision.

## Discovery

`<drive>/.aidex/runtime/daemon.json` (owner-only 0600 on Unix):

```json
{"version": 2, "pid": 1234, "port": 49731, "token": "<capability>"}
```

`token` is the 256-bit OS-random capability as 64 hex characters. On Windows it
is stored DPAPI-protected (current-user scope, base64). A `version` other than 2
is reported as incompatible; such an endpoint is removed by `aidex daemon start`
only when its port is unreachable.

## Framing

- TCP to `127.0.0.1:<port>` only. One persistent connection per client.
- UTF-8, one JSON-RPC 2.0 object per line, terminated by `\n` (a trailing `\r`
  is tolerated; blank lines are ignored).
- At most 1 MiB (1,048,576 bytes) per line excluding the newline. A larger line
  is answered with `message_too_large` and the connection is closed. The daemon
  never emits larger lines; an oversized result becomes a `message_too_large` error.
- Batches are not supported (`invalid_request`). Members outside `jsonrpc`,
  `method`, `params`, `id`, `result`, and `error` are rejected. `params` must be
  an object or array when present. Ids are strings or integers.
- Requests on a connection are handled in order. Notifications from the client
  (no `id`) get no reply.

## Handshake

The first message must be this request, within 5 seconds and at most 4 KiB:

```json
{"jsonrpc":"2.0","id":0,"method":"aidex.handshake",
 "params":{"contract_version":2,"token":"<capability>","client":{"name":"aidex-cli","version":"0.1.0"}}}
```

`client.name` is 1-64 characters of `[A-Za-z0-9._-]`; `client.version` is 1-64
printable ASCII characters. They identify, they do not authenticate. Success:

```json
{"jsonrpc":"2.0","id":0,"result":{"contract_version":2,"server":{"name":"aidexd","version":"0.1.0"},"connection_id":7}}
```

Any other first message gets `handshake_required`; an unsupported version gets
`incompatible_version` with `data: {"supported": [2], "requested": N}` (checked
before the capability); a wrong capability gets `unauthorized` (constant-time
comparison); malformed params get `invalid_params`. Each failure is answered
once and the connection is closed. A second `aidex.handshake` on an
authenticated connection is `invalid_request`.

## Method naming

- Methods are `<domain>.<verb>`: lowercase ASCII segments of letters, digits,
  and `_`, at least two segments, at most 128 bytes (`daemon.status`,
  `events.replay`, `models.list_local`).
- `aidex.*` is reserved for the transport.
- Streams are `<domain>.subscribe` and emit `<domain>.event` notifications.
- Params and results are JSON objects with closed (`deny_unknown_fields`)
  schemas; absent params mean `{}`.

## Streams

A subscribe call returns `{"subscription": "sub-3", "data": <method result>}`.
The response is always written before the first notification, and each
notification is:

```json
{"jsonrpc":"2.0","method":"events.event","params":{"subscription":"sub-3","data":{...}}}
```

The client ends a stream with `aidex.unsubscribe` `{"subscription": "sub-3"}`
(request or notification). When the daemon ends one it sends
`aidex.unsubscribed` `{"subscription": "sub-3", "reason": "completed"}`. At most
64 subscriptions per connection.

## Backpressure and closing

Each connection has a bounded outbound queue (1024 messages / 16 MiB). A client
that stops reading is disconnected as `slow_consumer` rather than slowing other
clients or growing daemon memory; resume from your own cursor after
reconnecting. Before the daemon closes a connection it attempts

```json
{"jsonrpc":"2.0","method":"aidex.closing","params":{"code":-32014,"message":"daemon is stopping"}}
```

(`shutting_down` on stop or drive removal, `slow_consumer` on overflow). At
most 64 concurrent connections are accepted; extras receive one `server_busy`
error with `id: null` and are closed.

## Error codes

Codes are stable within contract version 2; new codes are appended, never
renumbered. Messages are for humans and never contain secrets.

| Code | Name | Meaning | `ServiceErrorCode` | CLI exit |
| --- | --- | --- | --- | --- |
| -32700 | `parse_error` | Line is not valid UTF-8 JSON | `invalid_argument` | 1 |
| -32600 | `invalid_request` | Not an acceptable JSON-RPC 2.0 message | `invalid_argument` | 1 |
| -32601 | `method_not_found` | No such method (daemon older than client?) | `not_found` | 10 |
| -32602 | `invalid_params` | Params rejected | `invalid_argument` | 2 |
| -32603 | `internal` | Daemon failure; maps `internal` | `internal` | 1 |
| -32001 | `invalid_argument` | Domain rejected an argument | `invalid_argument` | 2 |
| -32002 | `not_found` | Entity or history not found (e.g. expired event cursor, `data.reason = "cursor_expired"`) | `not_found` | 8 |
| -32003 | `permission_denied` | Not permitted | `permission_denied` | 6 |
| -32004 | `conflict` | State conflict | `conflict` | 1 |
| -32005 | `incompatible_version` | Contract or data version mismatch | `incompatible_version` | 10 |
| -32006 | `unavailable` | Service temporarily unavailable | `unavailable` | 3 |
| -32010 | `unauthorized` | Handshake capability rejected | `permission_denied` | 5 |
| -32011 | `handshake_required` | First message was not the handshake, or it timed out | `permission_denied` | 5 |
| -32012 | `message_too_large` | Line over 1 MiB (handshake: 4 KiB) | `invalid_argument` | 1 |
| -32013 | `server_busy` | Connection or subscription limit reached | `unavailable` | 3 |
| -32014 | `shutting_down` | Daemon stopping or drive removed | `unavailable` | 3 |
| -32015 | `slow_consumer` | Outbound queue overflowed; connection dropped | `unavailable` | 3 |
| -32020 | `vault_locked` | Vault locked, or wrong passphrase/recovery key (`data.reason`: `locked`, `wrong_passphrase`, `wrong_recovery_key`) | `permission_denied` | 5 |
| -32021 | `policy_denied` | Reserved for broker/policy: denied by policy | `permission_denied` | 6 |
| -32022 | `model_unavailable` | Reserved for models/providers: nothing usable | `unavailable` | 7 |
| -32023 | `cancelled` | Operation cancelled or interrupted | `unavailable` | 9 |

Domain errors carrying an `aidex_core::ServiceErrorCode` map to -32001..-32006
(and `internal` to -32603).

## Methods (contract version 2)

| Method | Params | Result |
| --- | --- | --- |
| `daemon.status` | none | `{state: "ready"\|"stopping", pid, contract_version, version, drive_root, uptime_ms}` |
| `daemon.ping` | none | `{pong: true}` |
| `daemon.stop` | none | `{state: "stopping"}`; then every connection is closed and the endpoint removed |
| `events.replay` | `{after?, filter?, limit?}` (`after` default 0; `limit` 1-256, default 256) | `{events: [EventEnvelope...], next_cursor, has_more}`: matching events strictly after `after`, ascending; pages are also cut to fit the message limit |
| `events.latest` | none | `{latest_cursor, oldest_cursor}` (`oldest_cursor` is `null` when nothing is retained) |
| `events.subscribe` (stream) | `{after?, filter?}` | `{subscription, data: {after, latest_cursor}}`, then ordered `events.event` and `events.watermark` notifications |
| `vault.status` | none | `{state: "missing"\|"locked"\|"unlocked"\|"unavailable", vault_id?, secrets?, has_recovery_key?, unlocked_by?, unlocked_at?, idle_lock_secs?, reason?, message?}` |
| `vault.init` | `{passphrase, recovery?: bool}` | `{state: "unlocked", vault_id, recovery_key?}`; the recovery key is returned only here, once |
| `vault.unlock` | `{passphrase}` or `{recovery_key}` | `vault.status` result |
| `vault.lock` | none | `{state, was_unlocked, revoked_handles}`; active native runs are cancelled first, and the lock cleanup (uploads in flight aborted, `vault.locked` recorded) has run when it returns |
| `vault.secret.set` | `{name, value, consumers?: ["provider:<id>"\|"mcp:<id>"\|"process:<id>"\|"git:<host>"], kind?}` | `SecretMeta {secret_ref, name, kind, created_at, updated_at, policy: {allowed_consumers}}` (never the value) |
| `vault.secret.list` | none | `{secrets: [SecretMeta...]}` sorted by name |
| `vault.secret.delete` | `{name}` | `{deleted: SecretMeta}` |
| `vault.passphrase.change` | `{old_passphrase?, new_passphrase}` (`old_passphrase` may be omitted only after a recovery-key unlock) | `{changed: true}` |
| `model.list` | none (absent, `null`, `[]`, `{}`), or `{refresh?: bool, provider_id?}` | `{models: [ModelDescriptor & {input?: {images: bool, pdf: bool}}...], failures: [{provider_id, kind, message}], default_model: ModelRef\|null, network: "online"\|"offline"}`; every enabled provider, reusing cached listings. `refresh` drops cached listings first; `provider_id` lists only that provider (an unknown or disabled one is a `failures` entry, not an error). `input` says whether the model reads image and PDF attachments through its provider account; it is computed on every listing and never persisted |
| `model.providers` | none | `{providers: [ProviderEntry...]}` ordered by id, disabled accounts included; opens no connection |
| `model.route` | `{task, privacy?, required_context?, needs_tools?, needs_json_schema?, secret_heavy?, model?: "provider/model"}` | `{decision: {task, privacy, primary, fallbacks, allow_remote, preferred_role, reasons, rejected: [{model, reason, ..., explanation}]}, model: ModelDescriptor, network, failures}` |
| `model.use` | `{provider_id, model_id}` | `{default_model, model: ModelDescriptor}` (persisted; the provider must list the model) |
| `model.default` | none | `{default_model: ModelRef\|null}` |
| `provider.add` | `{id, kind?: "openai-compatible", base_url, secret_name?, locality?: "remote", discover_models?, models?: [ModelConfig...], display_name?, preset_id?, enabled?: true}` | `ProviderEntry` |
| `provider.update` | as `provider.add` | `ProviderEntry`; the provider configuration is replaced as a whole. Of the account metadata, an absent field is kept and `null` clears it |
| `provider.patch` | `{id, display_name?: string\|null, enabled?}` | `ProviderEntry`; omitted fields stay, `null` clears the name. Runtime providers are `conflict` |
| `provider.presets` | none | `{presets: [ProviderPreset...]}` in presentation order |
| `provider.open_link` | `{preset_id, link: "api_key"\|"docs"}` | `{opened: true}`; opens the HTTPS link stored in that preset in the system browser. There is no URL parameter |
| `provider.remove` | `{id}` | `{removed: id}` (clears the default model if it pointed there; removing a ChatGPT account also signs it out, best effort) |
| `provider.health` | `{id}` | `{provider_id, status: "ready"\|"auth_required"\|"unavailable"\|"refused", error?, message?, latency_ms?, checked_at}` (a disabled account is `refused` without a connection) |
| `model.preferences.get` | none | `{favorite_models: [key...], hidden_models: [key...]}` (empty lists when never saved) |
| `model.preferences.set` | `{favorite_models?, hidden_models?}` | the saved record; an omitted (or `null`) list stays as saved |
| `runtime.catalog` | none | `[DriverInfo...]`: `{driver, name, protocol, command, package, modes, permission_modes, limitations, auth_methods}` for each supported driver |
| `runtime.list` | none | `{runtimes: [RuntimeConfig...]}` |
| `runtime.add` | `RuntimeConfig` | `RuntimeConfig` (`conflict` if the id exists) |
| `runtime.update`, `runtime.config.save` | `RuntimeConfig` | `RuntimeConfig` (`update` requires an existing id; `config.save` adds or replaces) |
| `runtime.remove` | `{id}` | `{removed: bool}` |
| `runtime.config.get` | `{id}` | `RuntimeConfig` |
| `runtime.health` | `{id}` | `{id, installed, protocol_ready, authenticated, status: "ready"\|"binary_missing"\|"auth_required"\|"installed_auth_unverified"\|"protocol_ready_auth_unverified", limitations}` (never starts a model turn) |
| `runtime.models` | `{id}` | `{models: [{id, name}]}` (drivers with a model-list protocol: codex, pi, omp, muse) |
| `runtime.install` | `{driver, source?}` | `{driver, executable, verified_sha256}` (only components declared by the bundle's verified manifest) |
| `runtime.auth.start` | `{id}` | as `auth.start`, for a Codex runtime that shares a ChatGPT account |
| `runtime.auth.status` | `{id, flow_id?}` | `AuthStatus` of that runtime's account |
| `runtime.auth.cancel` | `{id, flow_id}` | `AuthStatus` |
| `network.offline` | `{enabled}` | `{offline, network}` (persisted on the drive) |
| `network.status` | none | `{offline, network}` |
| `web.settings.get` | none | `WebSettingsView` (defaults when `private_config/web.toml` does not exist; reading never creates it) |
| `web.settings.set` | `{enabled?, provider?: "brave"\|"tavily"\|"exa"\|"searxng"\|"duckduckgo", endpoint?: string\|null, secret_name?: string\|null, max_results?: 1-10, fetch_enabled?}` | `WebSettingsView`; omitted fields stay, `null` (or `""`) clears `endpoint`/`secret_name`, and changing `provider` clears `secret_name` unless the same patch names one (a key never follows a provider change). Unknown fields (a key value) are `invalid_params`; a key is stored only with `vault.secret.set` for `process:web` |
| `web.providers` | none | `{providers: [{id, display_name, icon, description, requires_key, accepts_key, requires_endpoint, endpoint, key_url, docs_url}...]}` in presentation order; `icon` is the brand icon key (`brave`, `tavily`, `exa`, `searxng`, `duckduckgo`) |
| `web.test` | `{query, max_results?: 1-10, settings?: <web.settings.set patch>}` | `{provider, results: [SearchResult...], elapsed_ms}`; runs one search with the stored settings (plus the unsaved `settings` patch, which is not stored), whether or not `enabled` is on |
| `web.open` | `{url}` | `{opened: true}`; opens a result link in the system browser after strict validation (absolute http(s), a host, no credentials, whitespace, or control characters, at most 4096 bytes); the system opener accepts only `https://` |
| `tool.list` | none | `{tools: [ToolDescriptor...]}` (`{id, version, description, input_schema, output_schema, risk: {action, base_risk, reads_workspace, writes_workspace, spawns_process, network, destructive, remote_write}, capabilities}`) |
| `tool.invoke` | `{session, workspace?, tool, input}` (exact session id; `workspace` defaults to the session's) | `BrokerOutcome` as the local user (`origin: user`) |
| `tool.continue` | `{approval_id}` | `BrokerOutcome` of the approved call (re-planned; single use) |
| `broker.approval.list` | `{session?, workspace?, state?, limit?}` (`state`: pending\|approved\|denied\|expired\|revoked\|cancelled; `limit` 1-500) | `{approvals: [ApprovalRecord...]}`, newest first |
| `broker.approval.show` | `{id}` | `ApprovalRecord` |
| `broker.approval.resolve` | `{id, decision: "approve_once"\|"approve_session"\|"deny"}` | `ApprovalRecord` |
| `broker.approval.revoke` | `{id, reason?}` | `ApprovalRecord` (state `revoked`) |
| `policy.show` | none | `PolicyStatus` |
| `policy.profile.set` | `{profile: "safe"\|"balanced"\|"autonomous"\|"offline_secure"}` (`offline-secure` accepted) | `PolicyStatus` (persisted; `policy.changed`) |
| `policy.reload` | none | `PolicyStatus` (policy documents re-read; `policy.changed`) |
| `policy.explain` | `{tool, input?, session?, workspace?, origin?: "agent"\|"user"}` (default workspace: the selected one; default origin `agent`) | `Explanation` (dry run: nothing executes, no approval, no event) |
| `emergency.stop` | `{reason?, lock_vault?}` | `EmergencyReport {stopped_at, reason, cancelled_approvals, revoked_approvals, cancelled_calls, terminated_processes, failed_terminations, revoked_handles, vault_locked}`; `lock_vault: true` locks the vault as `vault.lock` does (native runs cancelled, uploads in flight aborted, `vault.locked` recorded) |
| `emergency.resume` | none | `{resumed}` (`false` when no stop was in effect) |
| `mcp.list` | `{workspace_id?}` (id or name: only servers offered there) | `{servers: [McpServer...], invalid: [{id, error}], tools_omitted}`; never starts a server. `tools_omitted` is `true` when tool lists were dropped to fit one message. Enabled, trusted plugins' `.mcp.json` servers appear as DISABLED entries (`source: "plugin"`, `source_owner`, id `<plugin>-<server>`); they are added and removed as plugins are enabled, disabled, uninstalled, or lose their trust |
| `mcp.add` | `McpServerSpec` | `McpServer` (`conflict` if the id exists; `extension.enabled` when added enabled) |
| `mcp.update` | `McpServerSpec` | `McpServer`; replaces the configuration (keeping its source) and stops the server so the next start uses it |
| `mcp.remove` | `{id}` | `{removed: bool}` (the server is stopped and its logs dropped) |
| `mcp.set_enabled` | `{id, enabled}` | `McpServer`; disabling stops it; `extension.enabled`/`extension.disabled` (`extension_id: "mcp:<id>"`, kind `mcp`) when it changes |
| `mcp.start` | `{id}` | `McpServer` with status `running`; launches (stdio) or connects (http), runs `initialize`, and lists tools. A disabled server is `conflict` (`data.reason = "disabled"`). For a plugin's server, `mcp.start`, `mcp.restart`, and `mcp.tools` re-verify the plugin first, so a plugin whose files changed loses its servers (`not_found`) instead of starting |
| `mcp.stop` | `{id}` | `McpServer` (status `stopped`); interrupts a request in flight |
| `mcp.restart` | `{id}` | `McpServer` |
| `mcp.tools` | `{id}` (starts the server on demand) | `{id, tools: [{name, description, input_schema}], total, truncated, notes}`; tools that do not fit one message are left out (`truncated`) |
| `mcp.logs` | `{id, limit?}` (`limit` 1-400, default 200) | `{id, lines: [{at, stream: "stderr"\|"server"\|"client", text}], dropped}` (newest last; scrubbed of the server's secret values and redacted) |
| `mcp.import` | `{workspace_id?, dry_run?}` (default: the selected workspace) | `{workspace_id, path: ".mcp.json", dry_run, servers: [{status: "added"\|"new"\|"exists"\|"invalid", original_name, config?, error?, dropped_env, dropped_headers, warnings}]}`; reads the workspace's `.mcp.json` and saves each valid entry as a DISABLED server offered only in that workspace (`new`: `dry_run` would add it; `exists`: an id that is already configured is left unchanged). Environment and header values are never imported |

`ModelDescriptor` is `{model: {provider_id, model_id}, display_name, locality: "local"|"remote", context_window, max_output_tokens, capabilities: {chat, streaming, tool_calls: "native"|"structured"|"none", json_schema_output, vision, embeddings}, roles, cost, min_ram_bytes, source: "configured"|"discovered"|"catalog"|"assumed"}`. No model method accepts or returns a credential value: `provider.add` takes a vault secret *name* (unknown fields such as `api_key` are invalid params). Provider failures carry `data.kind` (`ProviderErrorKind`: `offline_refused` and `policy_denied` → -32021 `policy_denied`; `unauthorized` and `credential_unavailable` → -32020 `vault_locked`; `cancelled` → -32023; `invalid_request` and `context_overflow` → -32001; `model_not_found` → -32002; `unavailable`, `rate_limited`, `timeout`, `protocol`, `http` → -32022 `model_unavailable`). A failed route is -32022 (or -32002 for an unknown override) with `data: {kind: "no_route", reasons, rejected, failures}`.

`ProviderEntry` is `{descriptor: {id, kind, base_url, locality, auth: {type: "none"|"vault_secret", secret_name?}}, origin: "configured"|"runtime", display_name, preset_id, enabled, discover_models, declared_models: [ModelConfig...], catalog: {fetched_at, model_count, error: ProviderFailure|null}|null}`; `catalog` is the last listing this daemon recorded (kept in memory). Account metadata (`display_name` 1-64 characters, `preset_id`, `enabled`) is stored in `private_config/provider-accounts.toml`, never in `providers.toml`, so an older daemon still loads `providers.toml` after `update.rollback`. A disabled account stays configured but is skipped by `model.list`, cannot be the default model, and is refused when a conversation runs on it (`unavailable`, "configured provider is unavailable"); disabling the account of the default model clears the default. `ProviderPreset` is `{id, display_name, kind, base_url, discover_models, tool_calls, json_schema_output, authentication, oauth_supported, icon, category: "cloud"|"gateway"|"local"|"enterprise"|"custom", description, auth_methods: ["api_key"|"oauth_chatgpt"|"aws_sigv4"|"none"...], docs_url, api_key_url, key_placeholder}`. `provider.open_link` resolves the URL only from the built-in preset table: an unknown preset or link is `invalid_params`, a preset without that link is `not_found`, offline mode is `policy_denied` with `data.reason = "offline_refused"`, and a browser that cannot be started is `unavailable` (`data.reason = "browser_unavailable"`; only Windows has an opener). The browser is started with fixed command text and the URL as environment data.

Model preferences are a vault record separate from `settings.*` (vault secret `aidex-model-preferences`, so the vault must be unlocked; vault errors map as for `vault.*`). A key is `provider/model` or `runtime:<runtime_id>/<model>`: 3-512 bytes, containing `/`, without control characters. Duplicates are dropped keeping the first occurrence; at most 64 favorites and 512 hidden keys are kept. Invalid keys or lists are `invalid_params` and change nothing.

`RuntimeConfig` is `{id, driver, executable, argv, profile, enabled, auth: {kind: "none"}|{kind: "vault", secret_name, env_name}|{kind: "codex_oauth", provider_id}}`: literal values and vault secret *names* only. External runtimes keep their configuration sealed with a vault key, so every `runtime.*` method except `runtime.catalog` needs an unlocked vault (`vault_locked`); if the runtime service could not start, they are `unavailable` with `data.reason = "runtime_unavailable"` and the daemon still serves everything else. Runtime errors carry `data.reason` (`vault_locked`, `invalid_config`, `not_found`, `conflict`, `runtime_busy`, `offline_refused`, `auth_required`, `installer_unavailable`, `integrity_failed`, ...). Conversations run on a runtime when their configuration names `runtime_id`; they record the placeholder `provider_id: "runtime"`, and `model_id: "default"` lets the runtime use its own configured default model. The daemon loads the enabled runtimes into the native engine after `vault.init`/`vault.unlock`, on `runtime.list`, and before `conversation.create`, `message.send`, or `agent.start` of a runtime conversation whose runtime it does not have yet.

`McpServerSpec` is `{id, name?, transport: "stdio"|"http", command?, args?, env_allowlist?, url?, headers_from_secrets?: [{header, secret_name}], env_from_secrets?: [{env, secret_name}], enabled?: false, workspaces?: [workspace_id], auto_approve_tools?: [tool]}` (closed: unknown fields are `invalid_params`). Ids are `[a-z0-9][a-z0-9_-]{0,63}` and name the vault consumer `mcp:<id>`; secret values are stored with `vault.secret.set` and never accepted here. A stdio server needs `command`; an http server needs an `https://` URL (`http://` only on loopback) without user info, query, or fragment. Command lines and URLs that look like they contain a credential are refused (`invalid_config`). `McpServer` is `{id, name, transport, endpoint (the redacted command line or the URL origin), enabled, status: "stopped"|"starting"|"running"|"failed"|"needs_auth", error?: {reason, message}, tool_count, tools: [{name, description}], source: "drive"|"workspace"|"plugin", source_owner?, secret_names, config: {version, id, name, transport, command?, args, env_allowlist, url?, enabled, workspaces, auto_approve_tools, headers_from_secrets?, env_from_secrets?, source?}, server?: {name, version, protocol_version, tools}, started_at?}`. Stdio servers run in the sandbox with a cleared environment (the sandbox basics plus `env_allowlist`) and their `env_from_secrets` values; every remote request passes the offline switch (loopback servers excepted), the emergency stop, and a `network.request` policy decision (approval verdicts refuse). The daemon stops every server on shutdown, vault lock, and emergency stop. MCP errors carry `data.reason`: `invalid_config`/`invalid_request` (-32001), `not_found`, `conflict`/`disabled` (-32004), `vault_locked`, `policy_denied`, `cancelled`, and `unavailable` with `needs_auth`, `offline_refused`, `launch_failed`, `protocol`, `output_limit`, `server_error`, `disconnected`, `session_expired`, `timeout`, `io`, or `unavailable`; if the MCP service could not start, every `mcp.*` method is `unavailable` with `data.reason = "mcp_unavailable"`.

`BrokerOutcome` is tagged by `outcome`: `{outcome: "completed", call_id, output: {summary, content, diff, changes, warnings}, permit: {issuer: {kind: policy|approval|read_only|development, ...}, sandbox}}`, `{outcome: "pending_approval", call_id, approval_id, card: ApprovalCard}`, `{outcome: "denied", call_id, reason, rule}`, or `{outcome: "failed", call_id, failure: {reason, code, message}}`. A denial or a tool failure is a result, not an error. `ApprovalRecord` is `{approval_id, session_id, workspace_id, call_id, tool_id, state: {status: pending|approved|denied|expired|revoked|cancelled, scope?, by?: {kind: local_user|remote_device, device_id?}, at?, reason?}, card, intent_fingerprint, consumed_at?, grant?: {expires_at, active}}`; `ApprovalCard` is `{approval_id, requester: {origin: {kind: agent|user|remote|skill|adapter, ...}, agent_id?}, tool_id, action, summary, effect, workspace_id, workspace_name, session_id, risk, scope_options: [once|session], created_at, expires_at, rule, reason, sandbox, targets, command?, secrets?: [{secret_name, consumer}], diff_preview?: {files: [{path, kind, additions, deletions, unified?, binary, truncated}], files_omitted, additions, deletions}, session_grant?, warnings?}` (no secret values; command lines and diff text are redacted). `PolicyStatus` is `{settings: {profile, privacy, host_trust, network_mode}, effective_profile, effective_privacy, effective_network_mode, policies_dir?, document_rules, document_error?, hard_rules: [{id, verdict, description}], emergency_stop?}`. Broker errors carry `data.reason` (`emergency_stop` → -32021 `policy_denied`; `unknown_tool`, `approval_not_found`, `invocation_lost` → -32002; `approval_pending`, `already_resolved`, `already_consumed`, `intent_changed` → -32004 `conflict`; `approval_denied`, `approval_expired`, `approval_revoked`, `approval_cancelled`, `unauthorized` → -32003; `scope_not_offered`, `invalid_request` → -32001; workspace reasons as for `workspace.*`). The daemon expires due approvals in the background, `session.cancel` also cancels the session's running calls and approvals, and daemon stop terminates managed processes.

Vault errors carry `data.reason`, the stable `aidex_vault::VaultErrorKind` name (e.g. `vault_not_found`, `vault_exists`, `locked`, `wrong_passphrase`, `corrupt`, `tampered`, `missing_header`, `unsupported_version`, `forbidden_consumer`, `secret_not_found`, `conflict`). Wrong credentials and a locked vault use `vault_locked`; other vault errors use their `ServiceErrorCode` mapping. An error that ended the unlocked session (`conflict`: `vault.json` changed behind the daemon, or `io` from an uncertain commit of the vault's own header) also carries `data.session_dropped: true`: the vault is now locked and must be unlocked again (this applies to `vault.secret.*`, `vault.passphrase.change`, `settings.*`, and `model.preferences.*`). Every end of an unlocked session (`vault.lock`, `emergency.stop` with `lock_vault`, the idle auto-lock, or a dropped session) cancels native runs, aborts attachment uploads in flight, stops MCP servers, and records `vault.locked` once, before the next unlock completes. The one exception: `vault.unlock` while the vault is unlocked but `vault.json` changed on disk replaces the session without any of this, because the vault never locks. Parameters of `vault.*` methods may contain passphrases or secret values: they are only accepted over this authenticated local connection and are never echoed in error messages (invalid-parameter errors name at most a missing field). Secret handles are issued and resolved inside the daemon for the broker; no IPC method returns a secret value.
| `workspace.add` | `{path, name?}` (absolute host directory) | `Workspace` |
| `workspace.create_drive` | `{name}` | `Workspace` (kind `drive`) |
| `workspace.list` | none | `{workspaces: [Workspace...], selected: id\|null}` |
| `workspace.get` | `{id}` (id or name) | `Workspace` |
| `workspace.select` | `{id_or_name}` | `Workspace` (selection persisted on the drive) |
| `workspace.selected` | none | `{workspace: Workspace\|null}` |
| `workspace.remove` | `{id, delete_data?}` (exact id) | `{workspace, deleted_data}`; host data is never deleted |
| `workspace.discover` | `{path}` | `{selected_root, enclosing_repository, nested_repositories, vcs, scan_truncated}` |
| `workspace.ephemeral.create` | `{source}` (id or name) | `{workspace, files, bytes, skipped}` |
| `workspace.ephemeral.review` | `{id}` | `{workspace_id, source_id, changes, conflicts, unsupported, digest, total_changes, total_conflicts, truncated}` |
| `workspace.ephemeral.apply` | `{id, paths?, review_digest?}` | `{applied, skipped_conflicts, failed}` |
| `workspace.ephemeral.discard` | `{id}` | `{workspace}` |
| `session.create` | `{workspace_id, agent_id?, title}` (exact registered workspace id; `agent_id` defaults to `aidex`) | `Session` (state `created`; nothing is started) |
| `session.list` | `{workspace_id?, state?, limit?}` (`limit` 1-500, default 50) | `{sessions: [Session...]}`, newest first |
| `session.show` | `{id}` | `{session, messages, tool_calls, open_tool_calls, artifacts, checkpoints, approvals, latest_checkpoint, event_gaps}` |
| `session.messages` | `{id, after?, limit?}` (`limit` 1-500, default 100) | `{messages: [Message...], next_after, has_more}`; pages are cut to fit one message |
| `session.checkpoints` | `{id}` | `{checkpoints: [CheckpointMeta...], total}` (newest 500, oldest first) |
| `session.resume_plan` | `{id}` | `ResumePlanView` (read-only) |
| `session.resume` | `{id}` | `ResumePlanView` with `adopted: true`; the session is adopted on this host and left `paused` |
| `session.cancel` | `{id}` | `Session` (state `cancelled`) |
| `session.branch` | `{id, checkpoint_id}` | `Session` (the new child) |
| `artifact.list` | `{session_id, offset?, limit?}` (native conversation id; `limit` 1-100, default 100) | `{artifacts: [ArtifactMeta...], total_artifacts, offset, next_offset, truncated}`, newest first |
| `artifact.get` | `{id}` | `ArtifactMeta` |
| `artifact.read` | `{id, offset?, limit?}` (byte `offset`, default 0; `limit` default and maximum 65536, larger values are clamped) | `{id, encoding: "utf8"\|"base64", content, offset, next_offset, total_bytes, sha256}`; `utf8` pages start and end on character boundaries (an `offset` inside a character is `invalid_params`), and `next_offset` is `null` on the last page |
| `artifact.delete` | `{id}` | `{deleted: bool}` (`false` when no such artifact exists); the encrypted bytes are removed too |
| `artifact.plan.update` | `{id, status: "active"\|"superseded"\|"completed"}` | `ArtifactMeta`; only `plan` artifacts (`conflict` otherwise); making a plan `active` supersedes the conversation's other active plan |
| `conversation.fork` | `{source_id, configuration: SessionConfig, title?, command_id?}` (`command_id` 1-128 bytes) | the new conversation (the `conversation.get` view); see below |
| `attachment.upload.begin` | `{session_id, name, size, sha256, mime?, source?: "paste"\|"file"}` (`size` 1 B-16 MiB; `sha256` of the whole file, hex; `source` defaults to `file`, and `workspace` is refused) | `{upload_id, chunk_bytes}` (`chunk_bytes` = 262144); `source` is the client's claim: `paste` shows a text attachment to models as the user's own words (`trust=user`), so send it only for text the user pasted |
| `attachment.upload.chunk` | `{upload_id, offset, data}` (`data` standard base64; every chunk but the last exactly `chunk_bytes`) | `{received}`; `offset` must equal `received`, and a chunk resent at an earlier chunk offset with the same length is acknowledged without writing |
| `attachment.upload.commit` | `{upload_id}` | `Attachment` (`staged`) |
| `attachment.upload.abort` | `{upload_id}` | `{aborted: bool}`; works while the vault is locked |
| `attachment.from_workspace` | `{session_id, workspace_id, path}` (project-relative `path`) | `Attachment` (`staged`, source `workspace`) |
| `attachment.list` | `{session_id, offset?, limit?}` (`limit` 1-200, default 200) | `{attachments: [Attachment...], total_attachments, offset, next_offset, truncated}`, staged and sent, newest first |
| `attachment.get` | `{id}` | `Attachment` |
| `attachment.read` | `{id, offset?, limit?}` (byte `offset`, default 0; pages of at most 512 KiB for `base64` and 128 KiB for `utf8`, larger limits are clamped) | `{id, encoding: "utf8"\|"base64", content, offset, next_offset, total_bytes, sha256}`; `utf8` (text attachment) pages start and end on character boundaries |
| `attachment.delete` | `{id}` | `{deleted: bool}` (`false` when no such attachment exists); staged attachments only |
| `attachment.exclude` | `{id, excluded?: bool}` (default `true`) | `Attachment`; a sent attachment stays in the transcript but is no longer sent to models (`excluded: false` restores it) |
| `context.index` | `{workspace_id?}` (id or name; default: the selected workspace) | `{job_id, workspace_id, already_running}`; one background job per workspace, incremental, resumes a cancelled build |
| `context.index.cancel` | `{workspace_id?}` | `{workspace_id, cancelled}` (committed batches are kept) |
| `context.status` | `{workspace_id?}` | `{workspace_id, index: IndexSummary, job: IndexJobStatus\|null}` |
| `context.search` | `{workspace_id?, query, mode?: "lexical"\|"filename"\|"symbol", path_prefix?, limit?}` (`limit` 1-200, default 20) | `{workspace_id, index_state, hits: [{path, line_range: {start, end}, snippet, score, language, symbol?: {name, kind}}]}` |
| `context.pack` | `{workspace_id?, task, budget, session_id?, paths?, rules?, memory?, search?}` (`budget` 1-100000; `rules`/`memory`/`search` default true) | `ContextPack {id, nonce, workspace_id, budget_tokens, total_tokens, items: [{source, text, tokens, score, trust}], dropped: [{source, reason, tokens?}], dropped_unlisted, terms, index_state}` |
| `memory.list` | `{scope?: "user"\|"workspace"\|"session", workspace_id?, session_id?}` (default scope `workspace`) | `{scope, entries: [MemoryEntry...]}` |
| `memory.add` | `{scope?, workspace_id?, session_id?, text, tags?}` | `MemoryEntry` (source `user`) |
| `memory.edit` | `{scope?, workspace_id?, session_id?, id, text?, tags?}` | `MemoryEntry` |
| `memory.remove` | `{scope?, workspace_id?, session_id?, id}` | the removed `MemoryEntry` |
| `rules.show` | `{workspace_id?}` | `{workspace_id, rules: {source, project: {package_manager, tests, lint, build}, never_modify, require_approval, instructions: [{path, text, bytes, truncated, redacted, sha256}]}}` |
| `skill.list` | `{workspace_id?}` (id or name; without it only drive and plugin skills) | `{skills: [SkillSummary...], issues: [{scope, location, error}...], workspace_id: id\|null, truncated}` sorted by name; cut to fit one message (`truncated`) |
| `skill.show` | `{id, workspace_id?}` (`id` is `<scope>:<name>`) | `SkillSummary` fields plus `body?` (redacted instructions; absent when the skill has an error), `front_matter: [{key, value: string\|boolean\|string[]}...]`, and `body_redacted?: true` (present only when credential-like text was hidden in the body, description, or version, so saving them back would lose it) |
| `skill.set_enabled` | `{id, workspace_id?, enabled}` (project and repo skills need `workspace_id`) | `SkillSummary`; records `extension.enabled`/`extension.disabled` when the state changes |
| `skill.save` | `{name, description, body, version?}` | `SkillSummary` of the drive skill written to `skills/<name>/SKILL.md` (created or replaced; a replaced skill keeps every other front matter key as written, and the save is refused with `invalid_skill` when the kept keys would not read back unchanged) |
| `skill.delete` | `{name}` (drive skills only) | `{deleted: SkillSummary}` |
| `plugin.list` | none | `{plugins: [PluginSummary...], unregistered: [name...]}` (sorted by name; every plugin is re-hashed) |
| `plugin.show` | `{name}` | `PluginDetail` (summary plus manifest, contents, and tree hash for the review dialog); cut to fit one message (see below) |
| `plugin.install` | `{source_path, replace?}` (absolute folder or `.zip` on this computer) | `PluginDetail` plus `replaced`; installed disabled; cut to fit one message (see below) |
| `plugin.trust` | `{name, tree_sha256, enable?}` (the exact hash the user reviewed) | `PluginSummary` |
| `plugin.set_enabled` | `{name, enabled}` | `PluginSummary` (enabling needs trust in the current tree hash) |
| `plugin.uninstall` | `{name}` | `{name, removed_files, was_enabled}` |
| `update.status` | none | `{target, current, previous, integrity: IntegrityReport, journal, pending, versions, running_dir, restart_required, daemon_version, recovery, recovery_error}` |
| `update.check` | `{source, allow_downgrade?, allow_unsigned?}` (`source` absolute: archive, unpacked bundle, or release folder) | `{source, package, checksum: verified\|absent\|not_applicable, candidate: {version, target, manifest_sha256, created_at, git_commit, signature, sbom}, current, previous, action: install\|upgrade\|repair\|downgrade_to_previous\|up_to_date}` (nothing changes) |
| `update.apply` | `{source, allow_downgrade?, allow_unsigned?}` | `{update_id, action, checksum, candidate, from, to, self_test, launchers, pruned, warnings, restart_required}` |
| `update.rollback` | `{allow_unsigned?}` or none | `{update_id, from, to, signature, launchers, warnings, restart_required}` |

`Workspace` is `{id, name, kind: {type: host|drive|ephemeral, source?}, root: {type: host|drive, path}, created_at, last_opened_at, path, availability: {state: available|unavailable, reason?}, repository: {vcs, root, is_workspace_root}|null}`. Workspace errors carry `data.reason` (`invalid_path`, `escapes_root`, `symlink_leaf`, `root_unavailable`, `workspace_not_found`, `path_not_found`, `forbidden_location`, `conflict`, `review_changed`, `not_a_change`, `wrong_kind`, `ownership_marker`, `too_large`, ...). A review lists at most 1000 changes and is shrunk to fit one message (`truncated`); `digest` always covers every change, and passing it to `workspace.ephemeral.apply` makes the apply fail with `conflict`/`review_changed` if anything changed since.

`Session` is `{id, workspace_id, agent_id, title, state: {kind: created|running|waiting_approval|paused|completed|failed|cancelled|interrupted, code?, message?, reason?, recoverable?}, parent_session_id, branched_from, model, created_at, updated_at, host_fingerprint: {id, os, arch}, last_event_cursor, budget: {input_tokens, output_tokens, tool_calls, wall_ms}, resumed_at}`. `ResumePlanView` is `{session, checkpoint: CheckpointMeta|null, checkpoint_host, recovered_from_corruption, corrupt_checkpoints, messages_since_checkpoint, messages_after, unrecoverable: [{kind: process|tool_call|approval|model_state, subject, detail}], unrecoverable_total, different_host, host_changes, event_gaps, event_gaps_total, resumable, blocked_reason, adopted, continuation_available, continuation}`; `continuation_available` is `false` until the native harness is wired, and nothing is started on its behalf. Session errors carry `data.reason` (`session_not_found`, `checkpoint_not_found`, `invalid_transition`, `session_closed`, `checkpoint_corrupt`, `no_valid_checkpoint`, `invalid_argument`, ...); `session.resume` of a session that is not interrupted or paused is `conflict` with `data.reason = "not_resumable"` and `blocked_reason`. The daemon runs session recovery before it serves: sessions found `running`/`waiting_approval` become `interrupted`, running tool calls `lost`, and unprovable events explicit gaps.

`SkillSummary` is `{id: "<scope>:<name>", name, display_name, description, scope: "project"|"repo"|"drive"|"plugin", enabled, user_invocable, model_invocable, version?, location, size_bytes, plugin?, error?}`; `location` is drive-relative (`skills/<dir>/SKILL.md`) or workspace-relative (`.claude/skills/<dir>/SKILL.md`). Skills are directories holding `SKILL.md` (or `skill.md`) with YAML front matter (`name`, `description` required; `version`, `user-invocable`, `disable-model-invocation`, `allowed-tools` optional) found, in precedence order (the first skill of a name wins), in the workspace's `.aidex/skills/` (scope `project`), `.agents/skills/`, `.claude/skills/`, `.codex/skills/` (scope `repo`), the drive's `skills/` (scope `drive`), and enabled plugins (scope `plugin`). Symbolic links and junctions are refused, bodies over 64 KiB are listed with an `error`, and all text is credential-redacted (skill text is still untrusted data). Enablement is stored in `private_config/skills.toml`: drive and plugin skills are enabled by default, project and repo skills disabled until enabled for that workspace; a skill with an `error` cannot be enabled. `skill.save` validates the draft and refuses credential-like text; project, repo, and plugin skills are never written or deleted. Skill errors carry `data.reason` (`invalid_skill_id`, `skill_not_found`, `invalid_skill`, `credential_detected`, `workspace_required`, `skill_conflict`, `invalid_state_file`, `unsupported_state_version`, `unsafe_path`, `io`, plus workspace reasons); `skill_unavailable` (an unknown, disabled, or invalid skill named for invocation) is `invalid_params`. A change whose event could not be recorded is `internal_error` with `data: {reason: "event_not_recorded", committed: true}`.

Native conversations: `message.send` and `agent.start` take `{session_id, content, command_id, parent_run_id?, skills?: ["<scope>:<name>"...], web_search?: bool, attachments?: ["att_<ULID>"...]}`. `content` is stored exactly as typed; `skills` (at most 8) are injected for that run only as delimited system messages, and unknown, disabled, or not user-invocable ids are `invalid_params` with `data.reason = "skill_unavailable"` (nothing is started). `web_search: true` turns on the web tools for that run in Chat mode (Agent and Plan modes follow the web settings) and is `unavailable` with `data.reason = "web_unavailable"` while web search is off or not configured; neither option is stored in the conversation configuration, and a command id reused with different options is a `conflict`. `conversation.get` and session-scoped `history.list` messages carry an optional `skills: [id...]` on the user message that invoked them, and an optional `attachments: [AttachmentRef...]` (`{id, name, kind, mime, size, pages?, width?, height?, excluded?}`, in the order they were sent) on the user message they were sent with; the attachments table, not the run record, is the authority for which message an attachment belongs to. Runs carry the optional `message_index` of their user message and the `mode`, `permission_mode` and `workspace_id` of the conversation when the run began (absent for runs recorded before these fields existed). Native runs offer harness tools with stable names the desktop renders specially: `web_search` (result JSON `{query, provider, results: [{title, url, snippet, published?}]}`), `web_fetch` (`{url, final_url, status, content_type, title, text_truncated, truncated, artifact?: {id, name, kind: "web_page", size}, artifact_error?, text}`; pages over 64 KiB are saved as `web_page` artifacts and `text` holds the first 64 KiB), `skill_load`, `artifact_create` and `artifact_update` (`{artifact: ArtifactMeta}`), `plan_propose` (`{title, artifact: ArtifactMeta}`, the new active plan), and `mcp__<server>__<tool>`. Tool result messages hold these results as produced; the harness wraps untrusted ones (web, MCP, skills) as delimited data only in the model's prompt. Approvals of `web_fetch` and MCP tools carry `preview: {tool: "web.fetch", url, host}` or `{tool: "mcp.call", server, server_name, server_tool}`.

`ArtifactMeta` is `{id: "art_<ULID>", session_id, run_id, name, kind: "document"|"code"|"data"|"diagram"|"plan"|"web_page"|"other", mime, language, size, sha256, encoding: "utf8"|"base64", plan_status: "active"|"superseded"|"completed"|null, created_at, updated_at}` (`run_id` and `language` may be `null`; `plan_status` is set exactly for `plan` artifacts; `sha256` is of the plaintext; times are Unix milliseconds). Artifacts belong to native conversations and live in the encrypted native store: names, metadata, and bytes (at most 16 MiB; above 1 MiB in encrypted blob files) are sealed with the vault-held record key, so every `artifact.*` method needs an unlocked vault (`vault_locked` otherwise). There is no RPC that creates artifacts: the native harness produces them, and deleting a conversation deletes its artifacts. Unknown artifacts and conversations are `not_found`; malformed parameters, an `offset` beyond `total_bytes`, and a zero `limit` are `invalid_params`; an artifact whose encrypted bytes fail authentication is `conflict`.

Chat attachments: `Attachment` is `{id: "att_<ULID>", session_id, name, kind: "image"|"pdf"|"text"|"unknown", mime, size, sha256, pages?, width?, height?, animated, state: "staged"|"sent", source?: "paste"|"file"|"workspace", created_at, run_id?, message_index?, position?, excluded}` (`sha256` is of the plaintext; `run_id`, `message_index` and the 0-based `position` within the message are set once sent; `unknown` is a kind a newer daemon wrote, listed but never sent). Bytes reach the daemon only as chunked uploads from the client or from a registered project through its confined root: `attachment.from_workspace` follows links only inside the root, refuses credential-looking names for both the requested path and the link target, reads regular files only (at most 16 MiB), and sends no bytes over IPC. The daemon reads no other host path. The kind is decided from the bytes, never from the client's MIME type: PNG, JPEG, GIF or WebP images (at most 3,750,000 bytes and 8000 pixels per side), PDFs (at most 16 MiB and 100 pages; encrypted PDFs and a PDF object stream that declares more than 1,000,000 objects are refused), and UTF-8 text without NUL bytes (at most 1 MiB; private key material is refused); `mime` only names the type of text. Names are 1-255 bytes, the final path component the client sent, without control or Unicode format characters, and are display-only. Names, metadata and bytes are sealed with the vault-held record key in their own table and blob directory (`<db stem>-attachments/`, swept separately from artifacts), so every `attachment.*` method except `attachment.upload.abort` needs an unlocked vault (`vault_locked`). `vault.lock`, `emergency.stop` with `lock_vault`, the idle auto-lock, and a session the vault drops (`data.session_dropped`) end every upload in flight: later chunks are `vault_locked` and partial files are deleted. Limits: at most 8 uploads and 64 MiB of declared bytes in flight per daemon; an upload with no chunk for 10 minutes is dropped; at most 40 staged attachments per conversation (uploads in flight included) and 256 MiB staged across the drive; staged attachments older than 24 hours are deleted when the daemon starts and, on a running daemon, by the next `attachment.upload.begin`, `attachment.from_workspace` or `attachment.list` (at most every 5 minutes); a message sends at most 20 attachments and 32 MiB. A message sent with attachments and blank `content` stores the content `[Attached files]`, so no daemon ever sends a blank user message; clients hide exactly that text on a message that has `attachments`. Sent attachments live and die with their conversation: `attachment.delete` refuses them (`attachment_sent`) and `conversation.delete` deletes them. Refusals are `invalid_params` with `data.reason`: `too_large`, `too_many_attachments`, `bad_offset`, `hash_mismatch`, `unsupported_type`, `sensitive_content`, `too_many_pages`, `attachment_sent`, `attachment_unavailable`, `invalid_name`, or `invalid_attachment`; `busy` (uploads in flight) is `unavailable`. Project files that cannot be read carry the workspace reasons (`path_not_found`, `invalid_path`, `escapes_root`, ...). Error messages never contain a name, a path, or content, and attachments record no events.

For `message.send` and `agent.start`, `attachments` are staged ids of the conversation, sent in order with the new user message; `content` may then be blank. Unknown, foreign, repeated, or already sent ids, more than 20 ids, and attachments with `parent_run_id` are `attachment_unavailable`. Images or PDFs the conversation's model cannot read, any attachments for a conversation that runs on an external runtime, and a message whose attachments cannot fit the model's declared or reported context window on its own are `attachments_unsupported`. A conversation too large for its external runtime is `context_too_large_for_runtime`. All are `invalid_params`, and nothing is stored. A retry with the same `command_id` and arguments returns the original run. Text attachments reach models only between nonce delimiters (`source=attachment`): uploads with `source: "file"` as `trust=untrusted` data, uploads with `source: "paste"` as `trust=user` (the user's own words), and `attachment.from_workspace` files as `trust=project`, which models follow as task guidance unless it conflicts with the user's request, policy, or approvals; no attachment grants permissions. Images and PDFs go as content parts to models whose `model.list` `input` allows them, through the same egress checks as every model request.

`conversation.fork` creates a conversation with `configuration` (validated like `conversation.create`, including its workspace; `title` replaces the configuration's title, and a blank title becomes "New conversation") whose messages are a copy of `source_id`'s history. Message indices are kept: assistant tool calls that never got a result are removed from their message, and only trailing empty assistant placeholders are dropped. Sent attachments are re-sealed into the new conversation with new ids on the same messages (at most 256 MiB, else `invalid_params` with `data.reason = "too_large"`). Runs, approvals, checkpoints, artifacts, staged attachments, and the external runtime thread are not copied. The result is the new conversation's `conversation.get` view with `forked_from: {session_id, message_count}`. A fork is refused (`conflict`) while the source has an active run. A retry with the same `command_id` and arguments returns the conversation the first call created; the same `command_id` with other arguments is `conflict`.

`IndexSummary` is `{workspace_id, state: empty|building|partial|complete, schema_version, files, chunks, symbols, vectors, bytes, runs, last_run_at, last_complete_at, truncated, repository: {vcs, is_workspace_root, branch, head, detached, remotes}|null, recovery: {kind: corrupt|schema_reset, reason, moved_to, at}|null}`; `IndexJobStatus` is `{job_id, workspace_id, state: running|completed|cancelled|failed, progress: {phase, total, processed, changed, removed}, started_at, finished_at, report, error, event_error}`. Index jobs record `task.created` (role `context.index`, `task_id` = job id), `task.started`, then `task.completed`, `task.cancelled`, or `task.failed`, scoped to the workspace; a job's `state` stays `running` until its terminal event is recorded (or `event_error` is set). Pack item sources are `{type: file, path, lines}`, `{type: rule}`, `{type: memory, scope, id}`, `{type: summary, index}`, or `{type: project_instructions, path}`; `trust` is `user`, `project`, or `untrusted`; drop reasons are `budget`, `secret_file`, `secret_like`, `stale`, `not_indexed`, `unreadable`. `MemoryEntry` is `{id, text, tags, source: {kind: user}|{kind: agent, session_id}, created_at, updated_at}`. Context and memory errors carry `data.reason` (`no_workspace_selected`, `invalid_query`, `invalid_request`, `index_busy`, `invalid_rules`, `secret_like`, `invalid_text`, `invalid_tag`, `memory_not_found`, `corrupt_memory`, plus workspace reasons).

Plugins are portable bundles of skills and MCP servers, never native code that AiDEX runs (`aidex-plugins`). A plugin is a folder with `aidex-plugin.json` or `.claude-plugin/plugin.json` (`{name, version, description, author?: {name, url?}, homepage?, license?, permissions?: {network?: [domain], secrets?: [name], shell?: bool}}`; other manifest keys are reported in `contents.ignored_fields` and never used), `skills/<dir>/SKILL.md`, and an optional `.mcp.json`. `plugin.install` copies the folder, or extracts the `.zip` (at most 48 MiB; a single top-level folder is accepted), into `plugins/<name>/` on the drive: at most 32 MiB in 4096 files, no symbolic links, junctions, or special files, no absolute, traversing, unportable, or case-colliding names, and no executables (by extension or by PE, ELF, or Mach-O content) unless `.mcp.json` names that exact file as a server command (`${CLAUDE_PLUGIN_ROOT}/...`). Its provenance goes to the new file `private_config/plugins.toml` (closed schema, version 1: `[[plugins]] {name, version, tree_sha256, installed_at, source, trusted_sha256?, enabled}`). The tree hash is SHA-256 over `"aidex-plugin-tree-v1\n"` and, for each regular file sorted by its `/`-separated path, `<path>\0<size>\0<sha256 hex>\n`, so a folder and a `.zip` of the same files hash alike. Plugins install disabled and untrusted; `plugin.trust` records the exact `tree_sha256` the user reviewed and fails with `hash_mismatch` if the files no longer hash to it, and `plugin.set_enabled {enabled: true}` needs that trust. Every read re-hashes the files: when they differ from the recorded tree (or are missing or invalid), trust is revoked and the plugin disabled (`extension.disabled`, kind `plugin`); an unreadable tree contributes nothing without losing its trust (its MCP servers are stopped and removed, and come back disabled once the files verify again). Only enabled plugins whose current tree is trusted contribute `plugin`-scope skills and disabled MCP server entries. `PluginSummary` is `{name, version, description?, integrity: "intact"|"modified"|"missing"|"invalid", tree_sha256 (current, null when missing or invalid), installed_sha256 (recorded), trusted_sha256, trusted, enabled, installed_at, source, location (drive-relative), skill_count, mcp_server_count, error?}`; `PluginDetail` adds `manifest_file`, `manifest`, and `contents: {skills: [{dir, path, name?, description?}], skills_truncated, mcp_servers: [{name, transport: "stdio"|"http"|"sse", command?, args, url?, env_keys, header_keys, bundled_command?}], ignored_fields, unsupported_components, file_count, total_bytes, readme?, readme_truncated}`. The review finds a skill's file as skill discovery does (`SKILL.md`, else `skill.md`, else another letter case of it) and skips hidden directories. `plugin.show` and `plugin.install` results are cut to fit one message: the README is dropped first (`readme_truncated`), then skills from the end (`skills_truncated`), and as a last resort `contents` is null and `error` is set; `skill_count` stays the real count. Environment and header *values* in `.mcp.json` are never read or returned; commands, arguments, URLs, and manifest text are redacted, and control characters in shown plugin text (other than the README's line breaks and tabs) become U+FFFD. Plugin errors carry `data.reason` (`invalid_request`, `invalid_manifest`, `unsafe_content`, `executable_refused`, `too_large` → -32001; `plugin_not_found` → -32002; `not_trusted` → -32003; `plugin_exists`, `hash_mismatch`, `integrity_failed` → -32004; `invalid_config` → -32001; `io` → -32603); enabling or disabling records `extension.enabled`/`extension.disabled` (kind `plugin`) and a failed append is `internal_error` with `data: {reason: "event_not_recorded", committed: true}`.

Update errors carry `data.reason` (stable: `unsigned`, `malformed_signature`, `signature_manifest_mismatch`, `unknown_key`, `revoked_key`, `key_not_valid`, `bad_signature`, `tampered`, `missing_critical_file`, `unsafe_archive`, `checksum_mismatch`, `checksum_missing`, `archive_too_large`, `wrong_target`, `downgrade`, `downgrade_not_previous`, `not_newer`, `invalid_version`, `incompatible_drive_schema`, `host_extraction`, `self_test_failed`, `busy`, `recovery_required`, `not_installed`, `no_previous`, `source_not_found`, `no_package`, ...) and `data.integrity` (`true` for authenticity and integrity refusals, which are `conflict`; version and platform refusals are `incompatible_version`). The switch takes effect on the next launch: the daemon never replaces its running binaries (`restart_required`). Interrupted updates are recovered when the daemon starts, before it serves; `update.status.recovery` reports what happened. Updates record `update.staged`, `update.verified`, `update.switched`, `update.rolled_back`, and `update.failed`.

Workspace and vault methods record their events in the drive log after the change commits: `workspace.added` (add, create_drive), `workspace.selected`, `workspace.removed`, `workspace.ephemeral_created|ephemeral_applied|ephemeral_discarded` (each scoped to the workspace named in the event), and drive-wide `vault.unlocked` (init, unlock). If that append fails the change is not undone and the method returns `internal_error` with `data: {reason: "event_not_recorded", committed: true}`. `vault.locked` is recorded once for every end of an unlocked session (`vault.lock`, `emergency.stop` with `lock_vault`, the idle auto-lock, or a dropped session), best effort: the session has already ended, so a failed append is not reported. An unlock that replaces a session whose `vault.json` changed on disk records only `vault.unlocked`: the vault never locked.

`WebSettingsView` is `{version: 1, enabled, provider, endpoint: string|null, secret_name: string|null, max_results, fetch_enabled, configured, problem?, credential_consumer: "process:web"}`: the closed-schema `private_config/web.toml` (secret *names* only) plus `configured` (`false` with a `problem` when SearXNG has no endpoint or a key provider has no `secret_name`). `SearchResult` is `{title, url, snippet, published?, source: provider}` with plain-text titles and snippets and absolute http(s) URLs. A search first asks the egress gate: the offline switch (`policy_denied`, `data.reason = "offline_refused"`), an emergency stop, then a `network.request` intent for the provider host under the active profile, privacy mode, network mode, and policy documents (`policy_denied`, `data.reason = "policy_denied"`; an approval verdict is refused, since a synchronous request cannot wait). Each decision is recorded as `policy.decision`. Only then is the key released, through a 30-second vault handle for `process:web` after a `secret.use` evaluation (`secret.granted`/`secret.revoked`); a locked vault is `vault_locked`. Web errors carry `data.reason`: `invalid_request`, `invalid_url`, `invalid_settings`, `not_configured`, `target_refused`, `offline_refused`, `policy_denied`, `vault_locked`, `secret_not_found`, `secret_denied`, `secret_invalid`, `credential_unavailable`, `unauthorized` (the provider refused the key), `rate_limited` (HTTP 429, or a DuckDuckGo bot check), `http_error`, `unsupported_content_type`, `too_many_redirects`, `protocol`, `timeout`, `cancelled`, `unavailable`, or `io`; messages name hosts and providers, never a URL query, a header, a response body, or a key. `web.open` errors are `invalid_params` (`invalid_url`), `invalid_argument` (`https_required`), `policy_denied` (`offline_refused`, checked before anything is launched), or `unavailable` (`browser_unavailable`).

Event envelopes, filters, and the catalog are specified in
`protocol/events/README.md`. `events.subscribe` delivers persisted history after
`after` first, then live events, in cursor order without duplicates or gaps; each
`events.event` notification's `data` is one envelope. An `events.watermark`
notification's `data` is `{cursor: N}`: all durable events through `N` were sent
earlier or filtered out. Acknowledge it only after processing all preceding
events, and resume with the same filter. Watermarks never overtake matching
events; they may coalesce multiple nonmatches and are not periodic heartbeats.
The initial `latest_cursor` is metadata, not an acknowledgement. Ephemeral
events do not advance acknowledgements. If the stream cannot continue without
a gap (retention removed undelivered history) the daemon sends one `events.error`
notification `{reason: "cursor_expired"|"internal", message, oldest_available?}`
and ends the stream. A client that stops reading is dropped as `slow_consumer`;
it reconnects after its last processed durable event or watermark.
An expired cursor is `not_found` with `data: {reason: "cursor_expired", requested,
oldest_available}`; a cursor the log never assigned is `invalid_params` with
`data.reason = "cursor_ahead"`; an unknown filter kind is `invalid_params`.
