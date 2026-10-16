# `@aidex/client`

Typed TypeScript access to AiDEX Core for the Tauri apps: wrappers for the desktop and installer Tauri commands, the daemon's JSON-RPC methods proxied through the desktop bridge, stream subscriptions over Tauri events, the error class every command rejects with, and DTO types. TypeScript only; no UI and no React.

## Entry points

| Import | Contents |
| --- | --- |
| `@aidex/client/bridge` | `isTauri()`, `command(name, args)` (rejects with `AidexError`; outside Tauri with kind `bridge_unavailable`), `listenTo(event, handler)` |
| `@aidex/client/errors` | `AidexError { code, kind, rpcCode?, data? }`, `toAidexError`, `ErrorKind` constants, `BridgeErrorPayload`, `ServiceErrorCode` |
| `@aidex/client/desktop` | `aidexStatus`, `driveStatus`, `sandboxCapabilities`, `startDaemon`, `stopDaemon`, `selectDrive(path)`, `callUpdate(method, params)` (`rpc_call_long`: `update.check`, `update.apply`, or `update.rollback` on a connection of its own that waits up to 30 minutes) |
| `@aidex/client/installer` | `listDrives`, `inspectDrive(root)`, `initDrive(root)` |
| `@aidex/client/host` | `hostProfile()` (both apps) |
| `@aidex/client/rpc` | `call(method, params)` typed by `RpcMethods`, `callUntyped`, `subscribe(method, params, { onData, onEnd })` |
| `@aidex/client/format` | `formatBytes`, `formatDuration`, `bytesToGb`, `UNKNOWN` |
| `@aidex/client/dto/<name>` | One module per area: `daemon` (events, stream notifications and watermarks, live delta payloads), `vault` (status and value-free secret metadata), `updates` (`update.*` reports), `sessions` (conversations, runs, approvals, checkpoints and their paged lists), `runtime` (external runtimes and managed accounts), `auth` (browser sign-in flows), `models`, `execution`, `artifacts`, `attachments`, `skills`, `mcp`, `plugins`, `web`, `inference`, `preferences`, `workspace`, `host`, `drive`, `sandbox`, `status`, `installer` |

`subscribe` attaches its `aidex://notification` and `aidex://subscription-ended` listeners before calling `rpc_subscribe`, so notifications sent before the call returns are delivered in order. It resolves to `{ id, initial, unsubscribe }`.

## Errors

Every command rejects with `BridgeErrorPayload { code, kind, rpc_code?, message, data? }`, normalized to `AidexError`. `code` is the cross-service `aidex_core::ServiceErrorCode`; `kind` is the stable identifier to branch on: a daemon error-table name (`method_not_found`, `invalid_params`, `vault_locked`, `policy_denied`, ... in `protocol/commands/README.md`), a drive error kind (`missing_manifest`, `not_removable`, `migration_required`, ...), or a bridge kind (`bridge_unavailable`, `no_drive`, `daemon_unavailable`, `daemon_not_found`, `subscription_limit`, `invalid_method`). Never branch on `message`.

## DTOs

The DTO files are hand-written mirrors of Rust serde output; each names its canonical Rust type. Until schema-driven generation exists (`protocol/`), change the Rust type and its mirror in the same commit. `EventKind` is deliberately open (`{ type: string; data?: Record<string, unknown> }`) because the event catalog is being expanded; `daemon.ts` types the payloads screens read (`AgentDelta`, `AgentRunDelta`, `StreamWatermark`).

`src/contracts.fixtures.ts` holds daemon params and results recorded over JSON-RPC, each checked with `satisfies` against its `RpcMethods` entry or DTO, so a missing or unknown field fails `check`. Re-record the affected fixture when a Rust type changes.

## Adding a daemon method

Add its params/result types under `src/dto/` and an entry to `RpcMethods` in `src/rpc.ts`; desktop screens then call it with `useRpc`. No Tauri change is needed. Prefer a typed entry over `callUntyped`, and add a recorded fixture for a result the desktop depends on. A new Tauri command gets a wrapper in `src/desktop.ts` or `src/installer.ts`.

## Checks

```sh
bun run --cwd packages/client check
```
