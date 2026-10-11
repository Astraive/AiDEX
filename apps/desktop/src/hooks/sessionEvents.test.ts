import { test } from "node:test";
import assert from "node:assert/strict";
import { sessionEvent } from "./sessionEvents";

const event = (type: string, cursor: number, session = "conversation-a") => ({
  version: 2,
  cursor,
  scope: { session_id: session },
  occurred_at: "2026-10-11T00:00:00Z",
  event: { type, data: { agent_id: "child-run", text: "delta" } },
});
test("root and child text keep streaming after their last durable task event", () => {
  const root = sessionEvent(
    event("agent.delta", 41),
    "events.event",
    "conversation-a",
    41,
  );
  const child = sessionEvent(
    event("agent.run_delta", 41),
    "events.event",
    "conversation-a",
    root?.cursor,
  );
  assert.equal(root?.envelope.event.type, "agent.delta");
  assert.equal(child?.envelope.event.data?.agent_id, "child-run");
  assert.equal(child?.cursor, 41);
  assert.ok(
    sessionEvent(
      event("agent.run_delta", 41),
      "events.event",
      "conversation-a",
      child?.cursor,
    ),
  );
});
test("recovery accepts new durable state and discards replayed duplicates", () => {
  const update = sessionEvent(
    event("task.completed", 42),
    "events.event",
    "conversation-a",
    41,
  );
  assert.equal(update?.cursor, 42);
  assert.equal(
    sessionEvent(
      event("task.completed", 42),
      "events.event",
      "conversation-a",
      update?.cursor,
    ),
    undefined,
  );
  assert.equal(
    sessionEvent(
      event("task.started", 40),
      "events.event",
      "conversation-a",
      update?.cursor,
    ),
    undefined,
  );
});
test("another session and malformed notifications cannot affect the active conversation", () => {
  assert.equal(
    sessionEvent(
      event("agent.run_delta", 41, "conversation-b"),
      "events.event",
      "conversation-a",
      41,
    ),
    undefined,
  );
  assert.equal(
    sessionEvent(
      event("task.completed", NaN),
      "events.event",
      "conversation-a",
      41,
    ),
    undefined,
  );
  assert.equal(
    sessionEvent(null, "events.event", "conversation-a", 41),
    undefined,
  );
});
