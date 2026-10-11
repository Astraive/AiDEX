import { test } from "node:test";
import assert from "node:assert/strict";
import type { Approval } from "@aidex/client/dto/sessions";
import { approvalDecision } from "./approvalDecision";
const gateway: Approval = {
  id: "approval-gateway",
  run_id: "external-run",
  tool_name: "mcp_call",
  arguments: { server_id: "scoped-server", tool_name: "inspect" },
  arguments_bytes: 64,
  arguments_truncated: false,
  fingerprint: "sealed-bridge-fingerprint",
  status: "pending",
  preview: null,
  preview_truncated: false,
  runtime_id: null,
  runtime_request_id: null,
  runtime_ref_omitted: true,
};
test("SDK gateway approval uses the standard exact decision without inventing native credentials or references", () => {
  assert.deepEqual(approvalDecision(gateway, true, "user-command"), {
    id: "approval-gateway",
    fingerprint: "sealed-bridge-fingerprint",
    approved: true,
    command_id: "user-command",
  });
  assert.deepEqual(
    approvalDecision(
      {
        ...gateway,
        runtime_id: "codex-work",
        runtime_request_id: "native-request",
      },
      false,
      "deny-command",
    ),
    {
      id: "approval-gateway",
      fingerprint: "sealed-bridge-fingerprint",
      approved: false,
      command_id: "deny-command",
    },
  );
});
test("expired or incomplete requests cannot be approved, while incomplete requests can be denied", () => {
  assert.throws(() =>
    approvalDecision({ ...gateway, status: "expired" }, true, "user-command"),
  );
  assert.throws(() =>
    approvalDecision(
      { ...gateway, preview_truncated: true },
      true,
      "user-command",
    ),
  );
  assert.equal(
    approvalDecision(
      { ...gateway, arguments_truncated: true },
      false,
      "deny-command",
    ).approved,
    false,
  );
});
