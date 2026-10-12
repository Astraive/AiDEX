# `@aidex/client`

Typed TypeScript access to AiDEX Core for the Tauri apps: wrappers for the desktop and installer Tauri commands, the daemon's JSON-RPC methods proxied through the desktop bridge, stream subscriptions over Tauri events, the error class every command rejects with, and DTO types. TypeScript only; no UI and no React.

## Entry points

| Import | Contents |
| --- | --- |
| `@aidex/client/bridge` | `isTauri()`, `command(name, args)` (rejects with `AidexError`; outside Tauri with kind `bridge_unavailable`), `listenTo(event, handler)` |
| `@aidex/client/errors` | `AidexError { code, kind, rpcCode?, data? }`, `toAidexError`, `ErrorKind` constants, `BridgeErrorPayload`, `ServiceErrorCode` |
| `@aidex/client/desktop` | `aidexStatus`, `driveStatus`, `sandboxCapabilities`, `startDaemon`, `stopDaemon`, `selectDrive(path)` |
| `@aidex/client/installer` | `listDrives`, `inspectDrive(root)`, `initDrive(root)` |
| `@aidex/client/host` | `hostProfile()` (both apps) |
| `@aidex/client/rpc` | `call(method, params)` typed by `RpcMethods`, `callUntyped`, `subscribe(method, params, { onData, onEnd })` |
| `@aidex/client/format` | `formatBytes`, `formatDuration`, `bytesToGb`, `UNKNOWN` |
| `@aidex/client/dto/<name>` | `host`, `drive`, `sandbox`, `daemon`, `status`, `installer`, `execution` (`execution.status`: engine capabilities, session and run summaries) |

`subscribe` attaches its `aidex://notification` and `aidex://subscription-ended` listeners before calling `rpc_subscribe`, so notifications sent before the call returns are delivered in order. It resolves to `{ id, initial, unsubscribe }`.

## Errors

Every command rejects with `BridgeErrorPayload { code, kind, rpc_code?, message, data? }`, normalized to `AidexError`. `code` is the cross-service `aidex_core::ServiceErrorCode`; `kind` is the stable identifier to branch on: a daemon error-table name (`method_not_found`, `invalid_params`, `vault_locked`, `policy_denied`, ... in `protocol/commands/README.md`), a drive error kind (`missing_manifest`, `not_removable`, `migration_required`, ...), or a bridge kind (`bridge_unavailable`, `no_drive`, `daemon_unavailable`, `daemon_not_found`, `subscription_limit`, `invalid_method`). Never branch on `message`.

## DTOs

The DTO files are hand-written mirrors of Rust serde output; each names its canonical Rust type. Until schema-driven generation exists (`protocol/`), change the Rust type and its mirror in the same commit. `EventKind` is deliberately open (`{ type: string } & Record<string, unknown>`) because the event catalog is being expanded.

## Adding a daemon method

Add its params/result types under `src/dto/` and an entry to `RpcMethods` in `src/rpc.ts`; desktop screens then call it with `useRpc`. No Tauri change is needed. A new Tauri command gets a wrapper in `src/desktop.ts` or `src/installer.ts`.

## Checks

```sh
bun run --cwd packages/client check
```
