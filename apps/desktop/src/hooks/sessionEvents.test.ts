import assert from "node:assert/strict";
import { test } from "node:test";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import { cursorUnusable, reconnectDelay, runDelta, sessionStreamStep, type SessionStreamStep } from "./sessionEvents";

const SESSION = "conversation-a";

const envelope = (
  type: string,
  cursor: number,
  data: Record<string, unknown> = {},
  session = SESSION,
): EventEnvelope => ({
  version: 2,
  cursor,
  scope: { workspace_id: "workspace-a", session_id: session },
  occurred_at: "2026-10-13T00:00:00Z",
  event: { type, data },
});

const step = (data: unknown, cursor: number, method = "events.event"): SessionStreamStep =>
  sessionStreamStep(method, data, SESSION, cursor);

/** Feed notifications in order, as `useSessionUpdates` does, and return the resume cursor. */
function follow(cursor: number, notifications: [method: string, data: unknown][]) {
  const delivered: string[] = [];
  for (const [method, data] of notifications) {
    const next = sessionStreamStep(method, data, SESSION, cursor);
    if (next.kind === "event") delivered.push(next.envelope.event.type);
    if (next.kind === "event" || next.kind === "watermark") cursor = next.cursor;
  }
  return { cursor, delivered };
}

test("ephemeral events are delivered but never advance the cursor", () => {
  for (const type of ["agent.delta", "agent.run_delta", "process.output"]) {
    const result = step(envelope(type, 57, { agent_id: "run-1", text: "partial" }), 41);
    assert.equal(result.kind, "event", type);
    assert.equal(result.kind === "event" && result.durable, false, type);
    assert.equal(result.kind === "event" && result.cursor, 41, `${type} keeps the durable cursor`);
  }
  // Equal-cursor deltas keep streaming after the durable event they follow.
  const { cursor, delivered } = follow(41, [
    ["events.event", envelope("task.started", 42)],
    ["events.event", envelope("agent.delta", 42, { text: "a" })],
    ["events.event", envelope("agent.delta", 42, { text: "b" })],
  ]);
  assert.equal(cursor, 42);
  assert.deepEqual(delivered, ["task.started", "agent.delta", "agent.delta"]);
});

test("a watermark advances the cursor but never regresses it", () => {
  assert.deepEqual(step({ cursor: 90 }, 41, "events.watermark"), { kind: "watermark", cursor: 90 });
  assert.deepEqual(step({ cursor: 30 }, 41, "events.watermark"), { kind: "watermark", cursor: 41 });
  assert.equal(step({ cursor: -1 }, 41, "events.watermark").kind, "ignored");
  assert.equal(step({ cursor: Number.NaN }, 41, "events.watermark").kind, "ignored");
  assert.equal(step(null, 41, "events.watermark").kind, "ignored");
  const { cursor } = follow(41, [
    ["events.event", envelope("task.started", 45)],
    ["events.watermark", { cursor: 60 }],
    ["events.event", envelope("process.output", 60, { process_id: "p", stream: "stdout", chunk: "x" })],
  ]);
  assert.equal(cursor, 60, "the ephemeral event after the watermark does not move it");
});

test("replayed duplicates and older durable events are dropped", () => {
  const { cursor, delivered } = follow(41, [
    ["events.event", envelope("task.completed", 42)],
    ["events.event", envelope("task.completed", 42)],
    ["events.event", envelope("task.started", 40)],
    ["events.watermark", { cursor: 50 }],
    ["events.event", envelope("tool.requested", 50)],
    ["events.event", envelope("tool.started", 51)],
  ]);
  assert.equal(cursor, 51);
  assert.deepEqual(delivered, ["task.completed", "tool.started"]);
});

test("another session's events and malformed notifications are ignored", () => {
  assert.equal(step(envelope("task.completed", 50, {}, "conversation-b"), 41).kind, "ignored");
  const foreignDelta = envelope("agent.run_delta", 41, { agent_id: "run", text: "x" }, "conversation-b");
  assert.equal(step(foreignDelta, 41).kind, "ignored");
  assert.equal(step({ ...envelope("task.completed", 50), scope: {} }, 41).kind, "ignored");
  assert.equal(step(envelope("task.completed", Number.NaN), 41).kind, "ignored");
  assert.equal(step({ ...envelope("task.completed", 50), event: { data: {} } }, 41).kind, "ignored");
  assert.equal(step(null, 41).kind, "ignored");
  assert.equal(step(envelope("task.completed", 50), 41, "events.unknown").kind, "ignored");
});

test("stream errors say whether the history this follower missed has expired", () => {
  const expired = step({ reason: "cursor_expired", message: "gone", oldest_available: 70 }, 41, "events.error");
  assert.deepEqual(expired, { kind: "error", reason: "cursor_expired", message: "gone", expired: true });
  const internal = step({ reason: "internal", message: "failed" }, 41, "events.error");
  assert.equal(internal.kind === "error" && internal.expired, false);
  assert.equal(step(null, 41, "events.error").kind, "error");
  assert.equal(cursorUnusable({ reason: "cursor_expired", requested: 3, oldest_available: 9 }), true);
  assert.equal(cursorUnusable({ reason: "cursor_ahead" }), true);
  assert.equal(cursorUnusable({ reason: "busy" }), false);
  assert.equal(cursorUnusable(undefined), false);
});

test("run_delta is routed by agent_id", () => {
  const root = runDelta(envelope("agent.run_delta", 41, { agent_id: "run-root", text: "root text" }));
  const child = runDelta(envelope("agent.run_delta", 41, { agent_id: "run-child", text: "child text" }));
  assert.deepEqual(root, { runId: "run-root", text: "root text" });
  assert.deepEqual(child, { runId: "run-child", text: "child text" });
  assert.equal(runDelta(envelope("agent.delta", 41, { text: "conversation text" })), undefined);
  assert.equal(runDelta(envelope("agent.run_delta", 41, { text: "no run" })), undefined);
  assert.equal(runDelta(envelope("agent.run_delta", 41, { agent_id: "", text: "x" })), undefined);
});

test("reconnects back off from one second to thirty", () => {
  assert.deepEqual([0, 1, 2, 3, 4, 5, 6, 20].map(reconnectDelay), [1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
});
