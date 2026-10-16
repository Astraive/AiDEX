import { test } from "node:test";
import assert from "node:assert/strict";
import { AidexError, ErrorKind } from "@aidex/client/errors";
import type { RunStatus } from "@aidex/client/dto/sessions";
import { activeRun, lockedByVault } from "./useSecurityStatus";

test("only pending, running, and waiting-for-approval runs count as active", () => {
  const statuses: Record<RunStatus, boolean> = {
    pending: true,
    running: true,
    waiting_approval: true,
    completed: false,
    failed: false,
    cancelled: false,
    interrupted: false,
  };
  for (const [status, active] of Object.entries(statuses)) assert.equal(activeRun({ status }), active, status);
  assert.equal(activeRun({ status: "unknown" }), false);
});

test("only a vault_locked failure counts as the locked state, not as a read error", () => {
  const failure = (kind: string) => new AidexError({ code: "permission_denied", kind, message: "refused" });
  assert.equal(lockedByVault(failure(ErrorKind.vaultLocked)), true);
  assert.equal(lockedByVault(failure(ErrorKind.daemonUnavailable)), false);
  assert.equal(lockedByVault(failure(ErrorKind.policyDenied)), false);
  assert.equal(lockedByVault(null), false);
  assert.equal(lockedByVault(undefined), false);
});
