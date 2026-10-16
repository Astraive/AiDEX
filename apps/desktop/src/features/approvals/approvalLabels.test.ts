import { test } from "node:test";
import assert from "node:assert/strict";
import { decisionLabels } from "./approvalLabels";

const request = (tool_name: string, run_id: string) => ({ tool_name, run_id });

test("decision buttons start with their visible text and name the tool", () => {
  const labels = decisionLabels(request("shell.run", "run_1"), false);
  assert.ok(labels.deny.startsWith("Deny"));
  assert.ok(labels.approve.startsWith("Approve once"));
  assert.ok(labels.deny.includes("shell.run"));
  assert.ok(labels.approve.includes("shell.run"));
  assert.ok(!labels.approve.includes("run_1"), "the conversation shows one run, so it is not named");
});

test("lists that mix runs name the run, so two requests never share a button name", () => {
  const first = decisionLabels(request("shell.run", "run_1"), true);
  const second = decisionLabels(request("shell.run", "run_2"), true);
  assert.ok(first.approve.includes("run_1"));
  assert.ok(first.deny.includes("run_1"));
  assert.notEqual(first.approve, second.approve);
  assert.notEqual(first.deny, second.deny);
});
