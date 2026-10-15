// The drive-wide `vault.locked` events the workspace follows (`VaultLockProvider`, `../services`), as
// pure functions so the rules are tested without a daemon. `vault.locked` has no session scope, so no
// session stream ever carries it.
import type { EventFilter } from "@aidex/client/dto/daemon";
import { streamStep } from "./sessionEvents";

export const VAULT_LOCKED = "vault.locked";

/** The subscription filter: only `vault.locked` (watermarks still move the cursor past the rest). */
export const VAULT_LOCK_FILTER: EventFilter = { kinds: [VAULT_LOCKED] };

/**
 * Where the follower resumes after one notification (`undefined`: start again from
 * `events.latest`), and whether the vault locked or may have locked unseen: a `vault.locked` event,
 * or history that expired before the follower read it.
 */
export type VaultLockUpdate = { cursor: number | undefined; locked: boolean };

/** Apply one `events.subscribe` notification of the vault-lock stream to a follower at `cursor`. */
export function followVaultLocks(method: string, data: unknown, cursor: number): VaultLockUpdate {
  const step = streamStep(method, data, cursor, (envelope) => envelope.event.type === VAULT_LOCKED);
  switch (step.kind) {
    case "event":
      return { cursor: step.cursor, locked: true };
    case "watermark":
      return { cursor: step.cursor, locked: false };
    case "error":
      return step.expired ? { cursor: undefined, locked: true } : { cursor, locked: false };
    default:
      return { cursor, locked: false };
  }
}
