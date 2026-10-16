import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import { auditKindLabels, auditKinds, auditMetadata, mergeAuditRow, type AuditMetadata } from "./auditMetadata";

// The daemon's event catalog and filter limit, read from the Rust sources so the audit filter cannot
// drift from them unnoticed (an unknown kind makes `events.replay` and `events.subscribe` refuse it).
const crate = (path: string) => readFileSync(new URL(`../../../../../crates/events/src/${path}`, import.meta.url), "utf8");
const catalogBlock = /pub const EVENT_NAMES: &\[&str\] = &\[([\s\S]*?)\];/.exec(crate("catalog/mod.rs"))?.[1] ?? "";
const EVENT_NAMES = [...catalogBlock.matchAll(/"([^"]+)"/g)].map((match) => match[1]);
const MAX_FILTER_KINDS = Number(/pub const MAX_FILTER_KINDS: usize = (\d+);/.exec(crate("filter.rs"))?.[1]);

const marker = "SENSITIVE_VALUE_NEVER_IN_AUDIT_VIEW";
const event = (type: string, data: Record<string, unknown>, cursor = 42): EventEnvelope => ({
  version: 2,
  cursor,
  occurred_at: "2026-10-11T00:00:00Z",
  scope: { workspace_id: "workspace-a", session_id: "session-a" },
  event: { type, data },
});

test("every audited kind exists in the daemon's event catalog and fits one filter", () => {
  assert.ok(EVENT_NAMES.length > 50, "the EVENT_NAMES list was found in crates/events");
  assert.ok(MAX_FILTER_KINDS > 0, "MAX_FILTER_KINDS was found in crates/events");
  for (const kind of auditKinds) assert.ok(EVENT_NAMES.includes(kind), `${kind} is not in EVENT_NAMES`);
  assert.ok(auditKinds.length <= MAX_FILTER_KINDS);
  assert.equal(new Set(auditKinds).size, auditKinds.length);
  for (const kind of auditKinds) assert.match(kind, /^[a-z0-9_]+(\.[a-z0-9_]+)*$/);
  assert.deepEqual(Object.keys(auditKindLabels).sort(), [...auditKinds].sort());
});

test("audit rows never carry event bodies, commands, reasons, or secret names", () => {
  for (const kind of auditKinds) {
    const projected = auditMetadata(
      event(kind, {
        decision: "allow",
        summary: marker,
        message: marker,
        reason: marker,
        detail: marker,
        rule: marker,
        secret_name: marker,
        consumer: marker,
        command_summary: marker,
        code: marker,
        value: marker,
        arguments: { api_key: marker },
      }),
    );
    assert.ok(projected, kind);
    assert.equal(JSON.stringify(projected).includes(marker), false, kind);
    assert.deepEqual(Object.keys(projected), ["cursor", "time", "type", "workspace", "session", "result"]);
  }
});

test("message, output, and unknown events do not become audit rows", () => {
  assert.equal(auditMetadata(event("agent.message", { content: marker })), undefined);
  assert.equal(auditMetadata(event("agent.delta", { text: marker })), undefined);
  assert.equal(auditMetadata(event("process.output", { chunk: marker })), undefined);
  assert.equal(auditMetadata(event("secret.stored", { secret_name: marker })), undefined);
  assert.equal(auditMetadata({ ...event("policy.decision", {}), cursor: Number.NaN }), undefined);
});

test("results come from a closed set: verdicts, exit status, and enforcement", () => {
  assert.equal(auditMetadata(event("policy.decision", { decision: "require_approval" }))?.result, "Needs approval");
  assert.equal(auditMetadata(event("policy.decision", { decision: marker }))?.result, "Recorded");
  assert.equal(auditMetadata(event("process.exited", { exit_code: 3, signal: null }))?.result, "Exit 3");
  assert.equal(auditMetadata(event("process.exited", { exit_code: null, signal: 9 }))?.result, "Signal 9");
  assert.equal(auditMetadata(event("process.exited", { exit_code: marker }))?.result, "Exited");
  assert.equal(auditMetadata(event("sandbox.negotiated", { level: "brokered", enforced: true }))?.result, "Enforced");
  assert.equal(auditMetadata(event("sandbox.negotiated", { level: "restricted", enforced: false }))?.result, "Not enforced");
  assert.equal(auditMetadata(event("approval.denied", { reason: marker }))?.result, "Denied");
  const driveWide = auditMetadata({ ...event("vault.locked", { vault_id: "v" }), scope: {} });
  assert.equal(driveWide?.workspace, "Drive");
  assert.equal(driveWide?.session, "—");
});

test("rows merge newest first, once per cursor, up to the limit", () => {
  const row = (cursor: number): AuditMetadata => auditMetadata(event("vault.locked", {}, cursor))!;
  let rows: AuditMetadata[] = [];
  for (const cursor of [3, 1, 2, 2, 5]) rows = mergeAuditRow(rows, row(cursor), 3);
  assert.deepEqual(
    rows.map((item) => item.cursor),
    [5, 3, 2],
  );
});
