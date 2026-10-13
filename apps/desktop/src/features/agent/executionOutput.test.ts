import assert from "node:assert/strict";
import { test } from "node:test";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import {
  PROCESS_LIMIT,
  RUN_LIMIT,
  RUN_TEXT_LIMIT,
  STREAM_LIMIT,
  appendRunText,
  captureProcessEvent,
  markDisconnected,
  processStatus,
  tail,
  type ProcessTraces,
} from "./executionOutput";

const event = (type: string, data: Record<string, unknown>): EventEnvelope => ({
  version: 2,
  cursor: 8,
  scope: { workspace_id: "workspace-a", session_id: "session-a" },
  occurred_at: "2026-10-13T00:00:00Z",
  event: { type, data },
});
const output = (process_id: string, stream: string, chunk: string) =>
  event("process.output", { process_id, stream, chunk });
const apply = (traces: ProcessTraces, ...events: EventEnvelope[]) => events.reduce(captureProcessEvent, traces);
const trace = (traces: ProcessTraces, id: string) => traces.find((candidate) => candidate.id === id);

test("interleaved stdout and stderr stay separate and keep the exit code", () => {
  const traces = apply(
    [],
    event("process.started", { process_id: "test", command_summary: "cargo test", sandbox_level: "brokered" }),
    output("test", "stdout", "running 3 tests\n"),
    output("other", "stdout", "other output"),
    output("test", "stderr", "assertion failed\n"),
    output("test", "stdout", "test result: FAILED\n"),
    event("process.exited", { process_id: "test", exit_code: 1, signal: null }),
  );
  const cargo = trace(traces, "test");
  assert.equal(cargo?.label, "cargo test");
  assert.equal(cargo?.stdout, "running 3 tests\ntest result: FAILED\n");
  assert.equal(cargo?.stderr, "assertion failed\n");
  assert.equal(cargo?.ended, true);
  assert.equal(cargo?.exitCode, 1);
  assert.deepEqual(cargo && processStatus(cargo), { label: "Exited with code 1", tone: "danger" });
  assert.equal(trace(traces, "other")?.ended, false);
  assert.equal(trace(traces, "other")?.label, "other", "the id stands in until process.started arrives");
});

test("a signal exit and an exit without status read correctly", () => {
  const traces = apply(
    [],
    event("process.exited", { process_id: "killed", exit_code: null, signal: 9 }),
    event("process.exited", { process_id: "unknown", exit_code: null, signal: null }),
  );
  assert.equal(processStatus(trace(traces, "killed")!).label, "Stopped by signal 9");
  assert.equal(processStatus(trace(traces, "unknown")!).label, "Exited without a status code");
});

test("each stream keeps only its latest 64 KiB", () => {
  const chunk = "x".repeat(40 * 1024);
  const traces = apply(
    [],
    output("big", "stdout", chunk),
    output("big", "stdout", `${chunk}end`),
    output("big", "stderr", "e"),
  );
  const big = trace(traces, "big")!;
  assert.equal(big.stdout.length, STREAM_LIMIT);
  assert.ok(big.stdout.endsWith("xend"));
  assert.equal(big.stderr, "e");
});

test("a tail never starts inside a surrogate pair", () => {
  assert.equal(tail("ab😀cd", 4), "😀cd");
  assert.equal(tail("ab😀cd", 3), "cd");
  assert.equal(tail("short", 10), "short");
});

test("at most eight processes are kept, ended ones dropped first", () => {
  let traces: ProcessTraces = apply(
    [],
    output("running-0", "stdout", "a"),
    event("process.exited", { process_id: "ended-0", exit_code: 0 }),
  );
  for (let index = 1; traces.length < PROCESS_LIMIT; index += 1)
    traces = apply(traces, output(`running-${index}`, "stdout", "a"));
  assert.ok(trace(traces, "ended-0"), "eight fit");
  traces = apply(traces, output("newest", "stdout", "a"));
  assert.equal(traces.length, PROCESS_LIMIT);
  assert.equal(trace(traces, "ended-0"), undefined, "the ended process goes first");
  assert.ok(trace(traces, "running-0"));
  traces = apply(traces, output("newer", "stdout", "a"));
  assert.equal(traces.length, PROCESS_LIMIT);
  assert.equal(trace(traces, "running-0"), undefined, "then the least recently active one");
  assert.ok(trace(traces, "newer"));
});

test("unrelated and malformed events leave the traces unchanged", () => {
  const traces = apply([], output("p", "stdout", "a"));
  assert.equal(captureProcessEvent(traces, event("task.started", { task_id: "t" })), traces);
  assert.equal(captureProcessEvent(traces, output("p", "stdin", "x")), traces);
  assert.equal(captureProcessEvent(traces, event("process.output", { process_id: "p", stream: "stdout" })), traces);
  assert.equal(captureProcessEvent(traces, event("process.output", { stream: "stdout", chunk: "x" })), traces);
});

test("a disconnect marks running processes uncertain until their next event", () => {
  let traces = apply(
    [],
    output("running", "stdout", "a"),
    event("process.exited", { process_id: "done", exit_code: 0 }),
  );
  traces = markDisconnected(traces);
  assert.equal(trace(traces, "running")?.uncertain, true);
  assert.equal(trace(traces, "running")?.gap, true);
  assert.equal(trace(traces, "done")?.uncertain, false);
  assert.equal(processStatus(trace(traces, "running")!).tone, "warning");
  traces = apply(traces, output("running", "stdout", "b"));
  assert.equal(trace(traces, "running")?.uncertain, false);
  assert.equal(trace(traces, "running")?.gap, true, "the missed output stays missing");
  const ended = apply([], event("process.exited", { process_id: "x", exit_code: 0 }));
  assert.equal(markDisconnected(ended), ended, "nothing running, nothing to mark");
});

test("run text is appended per run and bounded", () => {
  let texts = appendRunText([], "root", "Hello");
  texts = appendRunText(texts, "child", "Child ");
  texts = appendRunText(texts, "root", " world");
  texts = appendRunText(texts, "child", "done");
  assert.deepEqual(texts, [
    { runId: "root", text: "Hello world" },
    { runId: "child", text: "Child done" },
  ]);
  texts = appendRunText(texts, "root", "y".repeat(RUN_TEXT_LIMIT));
  assert.equal(texts.find((entry) => entry.runId === "root")?.text.length, RUN_TEXT_LIMIT);
  for (let index = 0; index < RUN_LIMIT; index += 1) texts = appendRunText(texts, `run-${index}`, "x");
  assert.equal(texts.length, RUN_LIMIT);
  assert.equal(texts.find((entry) => entry.runId === "child"), undefined);
});
