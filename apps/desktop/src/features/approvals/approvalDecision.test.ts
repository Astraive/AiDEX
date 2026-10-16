import { test } from "node:test";
import assert from "node:assert/strict";
import type { Approval } from "@aidex/client/dto/sessions";
import { approvable, approvalDecision } from "./approvalDecision";

const request: Approval = {
  id: "approval-1",
  run_id: "run-1",
  tool_name: "write_file",
  arguments: { path: "src/main.rs" },
  arguments_bytes: 64,
  arguments_truncated: false,
  fingerprint: "fingerprint-1",
  status: "pending",
  preview: null,
  preview_truncated: false,
  runtime_id: null,
  runtime_request_id: null,
  runtime_ref_omitted: true,
};

test("a decision carries the exact id, fingerprint, and command id", () => {
  assert.deepEqual(approvalDecision(request, true, "command-a"), {
    id: "approval-1",
    fingerprint: "fingerprint-1",
    approved: true,
    command_id: "command-a",
  });
  assert.deepEqual(
    approvalDecision({ ...request, runtime_id: "codex-work", runtime_request_id: "vendor-request" }, false, "command-b"),
    { id: "approval-1", fingerprint: "fingerprint-1", approved: false, command_id: "command-b" },
  );
});

test("requests that are no longer pending cannot be answered", () => {
  for (const status of ["approved", "denied", "expired"] as const) {
    assert.throws(() => approvalDecision({ ...request, status }, true, "command"));
    assert.throws(() => approvalDecision({ ...request, status }, false, "command"));
    assert.equal(approvable({ ...request, status }), false);
  }
});

test("truncated requests can be denied but not approved", () => {
  for (const truncated of [{ arguments_truncated: true }, { preview_truncated: true }]) {
    const incomplete = { ...request, ...truncated };
    assert.equal(approvable(incomplete), false);
    assert.throws(() => approvalDecision(incomplete, true, "command"));
    assert.equal(approvalDecision(incomplete, false, "command").approved, false);
  }
  assert.equal(approvable(request), true);
});

test("a decision without the request identity or a command id is refused", () => {
  assert.throws(() => approvalDecision({ ...request, fingerprint: "" }, false, "command"));
  assert.throws(() => approvalDecision({ ...request, id: "" }, false, "command"));
  assert.throws(() => approvalDecision(request, false, ""));
});
