import { test } from "node:test";
import assert from "node:assert/strict";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import { captureProcessEvent } from "./executionOutput";
const event = (type: string, data: Record<string, unknown>): EventEnvelope => ({
  version: 2,
  cursor: 8,
  scope: { session_id: "session-a" },
  occurred_at: "2026-10-11T00:00:00Z",
  event: { type, data },
});
test("interleaved process output preserves stderr and a failed exit independently", () => {
  let traces = captureProcessEvent(
    {},
    event("process.started", {
      process_id: "test-process",
      command_summary: "test command",
    }),
  );
  traces = captureProcessEvent(
    traces,
    event("process.output", {
      process_id: "test-process",
      stream: "stdout",
      chunk: "running tests\n",
    }),
  );
  traces = captureProcessEvent(
    traces,
    event("process.output", {
      process_id: "other-process",
      stream: "stdout",
      chunk: "other output",
    }),
  );
  traces = captureProcessEvent(
    traces,
    event("process.output", {
      process_id: "test-process",
      stream: "stderr",
      chunk: "assertion failed\n",
    }),
  );
  traces = captureProcessEvent(
    traces,
    event("process.exited", { process_id: "test-process", exit_code: 1 }),
  );
  assert.equal(traces["test-process"].stdout, "running tests\n");
  assert.equal(traces["test-process"].stderr, "assertion failed\n");
  assert.equal(traces["test-process"].exitCode, 1);
  assert.equal(traces["other-process"].ended, false);
});
