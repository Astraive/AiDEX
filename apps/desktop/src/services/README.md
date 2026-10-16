# Desktop services

AiDEX Core integration for the desktop UI. Everything reaches Rust through `@aidex/client` (`packages/client`), which wraps the Tauri commands in `../../src-tauri/src/commands/`.

- `AidexProvider.tsx` wraps the whole app (launch flow included). It loads `aidex_status` on start and every 10 s, and exposes `startDaemon`, `stopDaemon`, `selectDrive`, and `refresh`, plus `connectionKey`, which changes when the drive or daemon changes so data hooks refetch. Read it with `useAidexStatus()` (`../hooks`).
- `VaultLockProvider.tsx` wraps the signed-in workspace (`../app/Workspace.tsx`). It follows the drive-wide `vault.locked` event with one filtered `events.subscribe` stream and counts the locks it sees; `useBridgeQuery` (`../hooks`) reloads every query when the count changes, so a lock from outside the app (the idle auto-lock, `aidex vault lock`) clears decrypted results at once. Read the count with `useVaultLocks()`.

Outside the Tauri window (for example `next dev` in a browser) every command rejects with `AidexError` kind `bridge_unavailable`; screens show that state and fall back to their labelled sample data. Nothing here mocks Core behaviour.

## Wiring a daemon-backed screen

1. Backend: register the method in `crates/aidexd/src/methods/<domain>.rs` (see `crates/aidexd/README.md`). No Tauri change is needed; `rpc_call` proxies any `<domain>.<verb>`.
2. DTO: mirror the params and result in `packages/client/src/dto/<domain>.ts` (name the canonical Rust type) and add the method to `RpcMethods` in `packages/client/src/rpc.ts`.
3. Screen: `const query = useRpc("<domain>.<verb>", params)` gives `{ data, error, loading, settled, refetch }`. Render `<CoreStatusNotice requires="daemon" />` (`../features/system`) for the no-drive, daemon-stopped, and incompatible states, and branch on `query.error?.kind` (for example `method_not_found` while a backend is not installed). Streams use `useSubscription("<domain>.subscribe", params, onData)`.
4. Remove the screen's `sample.ts` data and its page from `samplePages` in `../lib/routes.ts`.

A command that is not a daemon method (host facts, drive inspection) is added on the Rust side as described in `../../src-tauri/src/commands/README.md`, wrapped in `packages/client/src/desktop.ts`, and read with `useBridgeQuery`.
