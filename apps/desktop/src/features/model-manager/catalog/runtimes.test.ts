import { test } from "node:test";
import assert from "node:assert/strict";
import type { RuntimeHealth } from "@aidex/client/dto/runtime";
import { runtimeHealthKind, runtimePickerStatus } from "./runtimes";

const health = (status: string): RuntimeHealth => ({
  id: "codex",
  installed: true,
  protocol_ready: false,
  authenticated: status === "authenticated_cached",
  status,
  limitations: [],
});

test("managed account statuses that runtime.health can return map onto known states", () => {
  assert.equal(runtimeHealthKind("ready"), "ready");
  assert.equal(runtimeHealthKind("binary_missing"), "binary_missing");
  assert.equal(runtimeHealthKind("auth_required"), "auth_required");
  assert.equal(runtimeHealthKind("signed_out"), "auth_required");
  assert.equal(runtimeHealthKind("auth_pending"), "auth_pending");
  assert.equal(runtimeHealthKind("authenticated_cached"), "signed_in_unverified");
  assert.equal(runtimeHealthKind("installed_auth_unverified"), "auth_unverified");
  assert.equal(runtimeHealthKind("protocol_ready_auth_unverified"), "auth_unverified");
  assert.equal(runtimeHealthKind("something_new"), "unknown");
});

test("the picker never shows a raw status code", () => {
  for (const status of ["authenticated_cached", "auth_pending", "signed_out", "something_new"]) {
    const shown = runtimePickerStatus({ state: "ready", health: health(status) }, "Codex");
    assert.ok(!shown.statusText?.includes(status), `${status}: ${shown.statusText}`);
  }
  assert.deepEqual(runtimePickerStatus({ state: "ready", health: health("signed_out") }, "Codex"), {
    status: "setup",
  });
  assert.equal(runtimePickerStatus({ state: "ready", health: health("authenticated_cached") }, "Codex").status, "warning");
  assert.equal(runtimePickerStatus({ state: "ready", health: health("ready") }, "Codex").status, "ready");
});
