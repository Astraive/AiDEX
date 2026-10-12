# Desktop hooks

Reusable React hooks for the desktop app.

| Hook | Purpose |
| --- | --- |
| `useAidexStatus()` | Session drive and daemon status from `AidexProvider` (`../services`), plus `startDaemon`, `stopDaemon`, `selectDrive`, `refresh`, and `connectionKey` (changes when the drive or daemon changes). |
| `useBridgeQuery(load, key, enabled)` | Runs any bridge command when `key` changes: `{ data, error, loading, settled, refetch }`. Stale answers are dropped. |
| `useRpc(method, params, { enabled })` | A typed daemon method (`RpcMethods` in `@aidex/client/rpc`) through `rpc_call`; runs only while the daemon answers and refetches on reconnect. |
| `useSubscription(method, params, onData, { enabled })` | A daemon stream through `rpc_subscribe`; unsubscribes on unmount and resubscribes when the daemon changes. |
| `useCommandShortcut`, `useModifierKeyLabel` | Ctrl/⌘ shortcuts and the platform's command-key label. |
| `useKeyChord(chord, handler, { enabled })` | Window-wide chords such as `Mod+Shift+M` (parsed by `lib/keyChord.ts`; `Mod` is ⌘ on Apple platforms, Ctrl elsewhere). |

Errors are `AidexError` (`@aidex/client/errors`): branch on `kind` (`bridge_unavailable`, `no_drive`, `daemon_unavailable`, `method_not_found`, ...), never on message text.
