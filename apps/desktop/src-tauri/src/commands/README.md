# Tauri command adapters

`mod.rs` holds thin `#[tauri::command]` adapters; every one runs its work on a blocking worker (`tauri::async_runtime::spawn_blocking`) because daemon IPC, PowerShell volume queries, and daemon launches block. Failures are `BridgeError { code, kind, rpc_code?, message, data? }` (`../error.rs`): daemon errors keep the daemon's stable JSON-RPC code, error-table name, message, and data.

| Command | Args | Returns |
| --- | --- | --- |
| `aidex_status` | — | `AidexStatus { app_version, drive: resolved \| invalid \| none, daemon: running \| stopped \| incompatible \| unauthorized \| error \| no_drive }` |
| `drive_status` | — | `DriveReport { root, source, health?, health_error?, volume?, volume_error? }` (never modifies the drive) |
| `host_profile` | — | `aidex_host::HostProfile` |
| `sandbox_capabilities` | — | `aidex_sandbox::BackendCapabilities` (`probe()`) |
| `daemon_start` / `daemon_stop` | — | `AidexStatus` after the change |
| `rpc_call` | `method`, `params?` | the method's JSON result |
| `rpc_call_long` | `method` (`update.check`, `update.apply`, or `update.rollback` only), `params?` | the method's JSON result, from a connection of its own that waits up to 30 minutes (`UPDATE_CALL_TIMEOUT`), so the shared connection keeps serving other calls |
| `rpc_subscribe` | `method`, `params?` | `{ subscription, initial }`; notifications arrive as `aidex://notification` `{ subscription, method, data }`, and a daemon-side end as `aidex://subscription-ended` `{ subscription, reason, error? }` |
| `rpc_unsubscribe` | `subscription` | whether it was live |
| `select_drive` | `path` | `AidexStatus` with the chosen drive (memory only) |

`reports.rs` builds the status shapes without Tauri types and is unit-tested; the TypeScript mirrors are in `packages/client/src/dto/`.

## Adding a command

1. Write the behaviour as a plain function (in `reports.rs`, `../bridge`, or `../lifecycle`) with a unit test.
2. Add the adapter here, its name to `COMMANDS`, to `tauri::generate_handler!` in `../app.rs`, to the `AppManifest` list in `../../build.rs`, and `allow-<name-with-dashes>` to `../../capabilities/default.json` (a unit test checks all four agree).
3. Add a typed wrapper in `packages/client/src/desktop.ts` and the DTO mirror in `packages/client/src/dto/`.

Daemon methods do not need a command: call them with `rpc_call` (`call()` / `useRpc()` on the TypeScript side). The update operations that can run for minutes go through `rpc_call_long` (`callUpdate()` in `apps/desktop/src/features/settings/updateCalls.ts`).
