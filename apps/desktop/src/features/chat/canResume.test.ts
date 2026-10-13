import { test } from "node:test";
import assert from "node:assert/strict";
import type { RunStatus } from "@aidex/client/dto/sessions";
import { canResume } from "./canResume";

const run = (status: RunStatus, has_runtime_ref = false) => ({ status, has_runtime_ref });
const resumable = { supports_resume: true };

test("only an interrupted run offers Resume", () => {
  assert.equal(canResume(run("interrupted"), false, undefined), true);
  for (const status of ["pending", "running", "waiting_approval", "completed", "failed", "cancelled"] as const)
    assert.equal(canResume(run(status, true), false, resumable), false, status);
  assert.equal(canResume(undefined, false, resumable), false);
});

test("an external run needs a resumable runtime and a saved protocol reference", () => {
  assert.equal(canResume(run("interrupted", true), true, resumable), true);
  assert.equal(canResume(run("interrupted", false), true, resumable), false);
  assert.equal(canResume(run("interrupted", true), true, { supports_resume: false }), false);
  // A runtime the engine does not report (removed, or not loaded yet) cannot resume.
  assert.equal(canResume(run("interrupted", true), true, undefined), false);
  assert.equal(canResume(run("cancelled", true), true, resumable), false);
});
