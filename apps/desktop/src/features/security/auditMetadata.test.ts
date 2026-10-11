import { test } from "node:test";
import assert from "node:assert/strict";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import { auditMetadata } from "./auditMetadata";
const marker = "SENSITIVE_VALUE_NEVER_IN_AUDIT_VIEW";
const event = (type: string, data: Record<string, unknown>): EventEnvelope => ({
  version: 2,
  cursor: 42,
  occurred_at: "2026-10-11T00:00:00Z",
  scope: { workspace_id: "workspace-a", session_id: "session-a" },
  event: { type, data },
});
test("audit projection cannot export event-body strings or credential-bearing fields", () => {
  for (const type of [
    "policy.decision",
    "secret.stored",
    "secret.deleted",
    "secret.granted",
    "process.started",
    "approval.created",
  ]) {
    const projected = auditMetadata(
      event(type, {
        decision: "allow",
        summary: marker,
        message: marker,
        secret_name: marker,
        consumer: marker,
        command_summary: marker,
        value: marker,
        arguments: { api_key: marker },
      }),
    );
    assert.ok(projected);
    assert.equal(JSON.stringify(projected).includes(marker), false);
    assert.deepEqual(Object.keys(projected), [
      "cursor",
      "time",
      "type",
      "workspace",
      "session",
      "result",
    ]);
  }
});
test("message and live output bodies do not become security audit rows", () => {
  assert.equal(
    auditMetadata(event("agent.message", { content: marker })),
    undefined,
  );
  assert.equal(
    auditMetadata(event("process.output", { chunk: marker })),
    undefined,
  );
  assert.equal(
    auditMetadata(event("policy.decision", { decision: marker }))?.result,
    "Recorded",
  );
});
