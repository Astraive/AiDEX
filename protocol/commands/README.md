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
| `vault.lock` | none | `{state, was_unlocked, revoked_handles}` |
| `vault.secret.set` | `{name, value, consumers?: ["provider:<id>"\|"mcp:<id>"\|"process:<id>"\|"git:<host>"], kind?}` | `SecretMeta {secret_ref, name, kind, created_at, updated_at, policy: {allowed_consumers}}` (never the value) |
| `vault.secret.list` | none | `{secrets: [SecretMeta...]}` sorted by name |
| `vault.secret.delete` | `{name}` | `{deleted: SecretMeta}` |
| `vault.passphrase.change` | `{old_passphrase?, new_passphrase}` (`old_passphrase` may be omitted only after a recovery-key unlock) | `{changed: true}` |

Vault errors carry `data.reason`, the stable `aidex_vault::VaultErrorKind` name (e.g. `vault_not_found`, `vault_exists`, `locked`, `wrong_passphrase`, `corrupt`, `tampered`, `missing_header`, `unsupported_version`, `forbidden_consumer`, `secret_not_found`, `conflict`). Wrong credentials and a locked vault use `vault_locked`; other vault errors use their `ServiceErrorCode` mapping. Parameters of `vault.*` methods may contain passphrases or secret values: they are only accepted over this authenticated local connection and are never echoed in error messages (invalid-parameter errors name at most a missing field). Secret handles are issued and resolved inside the daemon for the broker; no IPC method returns a secret value.
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

`Workspace` is `{id, name, kind: {type: host|drive|ephemeral, source?}, root: {type: host|drive, path}, created_at, last_opened_at, path, availability: {state: available|unavailable, reason?}, repository: {vcs, root, is_workspace_root}|null}`. Workspace errors carry `data.reason` (`invalid_path`, `escapes_root`, `symlink_leaf`, `root_unavailable`, `workspace_not_found`, `path_not_found`, `forbidden_location`, `conflict`, `review_changed`, `not_a_change`, `wrong_kind`, `ownership_marker`, `too_large`, ...). A review lists at most 1000 changes and is shrunk to fit one message (`truncated`); `digest` always covers every change, and passing it to `workspace.ephemeral.apply` makes the apply fail with `conflict`/`review_changed` if anything changed since.

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
