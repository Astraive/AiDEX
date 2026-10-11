import { test } from "node:test";
import assert from "node:assert/strict";
import { supportsSelection } from "./runtimeSelection";
import type { RuntimeDescriptor } from "@aidex/client/dto/runtimes";

const external: RuntimeDescriptor = {
  id: "codex-work",
  name: "Codex",
  modes: ["agent"],
  permission_modes: ["full_access"],
  supports_resume: true,
  supports_approvals: true,
};
test("an external full-access adapter cannot inherit the native supervised or chat selection", () => {
  assert.equal(supportsSelection(external, "agent", "supervised"), false);
  assert.equal(supportsSelection(external, "agent", "auto"), false);
  assert.equal(supportsSelection(external, "chat", "full_access"), false);
  assert.equal(supportsSelection(external, "plan", "full_access"), false);
  assert.equal(supportsSelection(external, "agent", "full_access"), true);
  assert.equal(supportsSelection(undefined, "agent", "full_access"), false);
});
