import assert from "node:assert/strict";
import { test } from "node:test";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import { followVaultLocks, VAULT_LOCK_FILTER } from "./vaultLocks";

const driveWide = (type: string, cursor: number): EventEnvelope => ({
  version: 2,
  cursor,
  scope: {},
  occurred_at: "2026-10-15T00:00:00Z",
  event: { type, data: { vault_id: "vault-a" } },
});

test("the stream asks for vault.locked only", () => {
  assert.deepEqual(VAULT_LOCK_FILTER, { kinds: ["vault.locked"] });
});

test("a vault.locked event after the cursor is a lock and moves the cursor", () => {
  assert.deepEqual(followVaultLocks("events.event", driveWide("vault.locked", 12), 10), { cursor: 12, locked: true });
});

test("a replayed lock at or before the cursor, and other events, are not", () => {
  assert.deepEqual(followVaultLocks("events.event", driveWide("vault.locked", 10), 10), { cursor: 10, locked: false });
  assert.deepEqual(followVaultLocks("events.event", driveWide("vault.unlocked", 11), 10), { cursor: 10, locked: false });
  assert.deepEqual(followVaultLocks("events.event", { cursor: 11 }, 10), { cursor: 10, locked: false });
});

test("a watermark moves the cursor and never back", () => {
  assert.deepEqual(followVaultLocks("events.watermark", { cursor: 40 }, 10), { cursor: 40, locked: false });
  assert.deepEqual(followVaultLocks("events.watermark", { cursor: 5 }, 10), { cursor: 10, locked: false });
});

test("expired history may have held a lock: start again from the latest cursor and count one", () => {
  assert.deepEqual(
    followVaultLocks("events.error", { reason: "cursor_expired", message: "gone", oldest_available: 50 }, 10),
    { cursor: undefined, locked: true },
  );
  assert.deepEqual(followVaultLocks("events.error", { reason: "internal", message: "stopped" }, 10), {
    cursor: 10,
    locked: false,
  });
});
