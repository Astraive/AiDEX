# Desktop hooks

Reusable React hooks for the desktop app.

| Hook | Purpose |
| --- | --- |
| `useAidexStatus()` | Session drive and daemon status from `AidexProvider` (`../services`), plus `startDaemon`, `stopDaemon`, `selectDrive`, `refresh`, and `connectionKey` (changes when the drive or daemon changes). |
| `useBridgeQuery(load, key, enabled)` | Runs any bridge command when `key` changes: `{ data, error, loading, settled, refetch }`. Stale answers are dropped. Refetches that arrive while a load is in flight are coalesced into one load after it settles, so a read slower than its poll still lands. A failed load keeps no data (a locked vault never leaves decrypted records on screen), and every query reloads at once when the vault locks (`useVaultLocks`), so a lock from outside the app does not leave decrypted records on screen until the next poll. The state rules and the scheduler are in `bridgeQueryState.ts`. |
| `useRpc(method, params, { enabled })` | A typed daemon method (`RpcMethods` in `@aidex/client/rpc`) through `rpc_call`; runs only while the daemon answers and refetches on reconnect. |
| `useSubscription(method, params, onData, { enabled })` | A daemon stream through `rpc_subscribe`; unsubscribes on unmount and resubscribes when the daemon changes. |
| `useSessionUpdates(sessionId, refresh, onEvent?)` | Follows one session's events: `{ active, reconnecting, error }`. See below. |
| `useVaultLocks()` | How many times the vault locked while the workspace was open, from `VaultLockProvider` (`../services`); 0 outside it. See below. |
| `usePoll(poll, intervalMs, { enabled, whileAway })` | Every timer that re-reads through the daemon. It polls only while the user is present (`presence.ts`: the window is visible and had keyboard, pointer, wheel, touch, or focus input in the last minute) and polls once at once when they come back. `whileAway` keeps a visible window polling without input; use it only for reads tied to work AiDEX Core already keeps the vault awake for (an executing run, not one waiting for an approval) or for reads that do not touch the vault. See below. |
| `useCommandShortcut`, `useModifierKeyLabel` | Ctrl/⌘ shortcuts and the platform's command-key label. |
| `useKeyChord(chord, handler, { enabled })` | Window-wide chords such as `Mod+Shift+M` (parsed by `lib/keyChord.ts`; `Mod` is ⌘ on Apple platforms, Ctrl elsewhere). |

Errors are `AidexError` (`@aidex/client/errors`): branch on `kind` (`bridge_unavailable`, `no_drive`, `daemon_unavailable`, `method_not_found`, ...), never on message text.

## Session updates

`useSessionUpdates` subscribes to `events.subscribe` with the filter `{session_id}`, starting at `events.latest`. The saved session stays authoritative: `refresh` re-reads it after every (re)connect and after durable events (one read per 150 ms burst), and `onEvent` receives each of the session's events in order, including the ephemeral `agent.delta`, `agent.run_delta` and `process.output`.

The resume cursor follows the rules in `protocol/events/README.md`, implemented as pure functions in `sessionEvents.ts`:

- A durable event advances it; a durable event at or before it is a duplicate and is dropped.
- An `events.watermark` advances it to `max(cursor, watermark)` and never moves it back.
- Ephemeral events never advance it, and are never deduplicated by their cursor.
- Events of another session are ignored.

After the stream drops, the hook resubscribes after its cursor, so missed durable events are replayed once; ephemeral output sent meanwhile is lost (`reconnecting` is true until then). An `events.error` with `cursor_expired`, or a subscribe refused with `cursor_expired` or `cursor_ahead`, makes it re-read the snapshot and start again from `events.latest`. Reconnects back off from 1 s to 30 s; a stream that stayed up for 10 s resets the backoff, and a new drive or daemon starts over.

## Vault locks

`VaultLockProvider` (`../services`, mounted around the workspace) follows the drive-wide `vault.locked` event with one `events.subscribe` stream filtered to that kind, starting at `events.latest` and resuming with the same cursor rules (`vaultLocks.ts`; expired history counts as a lock). A session's stream never carries it, and a screen that is not polling (the user is away, or its session stream is up) would otherwise keep what it decrypted on screen after the idle auto-lock or `aidex vault lock`. Each lock bumps `useVaultLocks()`, and `useBridgeQuery` (so `useRpc` too) reloads every query: the reads fail with `vault_locked` and drop their data, and the pages' own handling of that refusal closes what they keep beside it (an open checkpoint, the selected run's transcript, live output). A view that reads outside `useBridgeQuery` follows `useVaultLocks()` itself: the artifact viewer (`features/chat/artifacts/artifactPreview.ts`) hides what it read before the latest lock in the render that sees it.

## Polling and the vault's idle lock

Most screen reads decrypt records. AiDEX Core does not count the background reads screens poll as vault use (`vault.status`, `vault.secret.list`, `conversation.list|get`, `run.get`, `approval.list`, `execution.status`, `checkpoint.list`, `history.list`, and the runtime status reads leave the idle period running), but every other read does, and an unattended window has no reason to keep reading decrypted records. So polls go through `usePoll`, which stops them while the window is hidden or the user is away, and the idle lock can happen while a run waits for an approval. Event-driven re-reads (`useSessionUpdates`) need no gate: they follow work that is happening.

Tests (`*.test.ts`, `node:test`) run with `bun test src/hooks`.
