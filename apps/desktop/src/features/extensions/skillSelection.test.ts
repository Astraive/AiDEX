import { test } from "node:test";
import assert from "node:assert/strict";
import type { SkillSummary } from "@aidex/client/dto/extensions";
import { skillSelectionReason } from "./skillEligibility";
const skill: SkillSummary = {
  id: "review",
  name: "review",
  description: "Review changes",
  enabled: true,
  trusted: true,
  user_invocable: true,
  model_invocable: false,
  workspace_ids: ["project-a"],
  profiles: ["supervised"],
  digest: "fixture",
  plugin_id: null,
};
test("explicit selection permits a manual-only skill in its granted project", () => {
  assert.equal(
    skillSelectionReason(skill, {
      workspaceId: "project-a",
      profile: "supervised",
    }),
    undefined,
  );
});
test("a selected skill cannot cross into projectless chat or another profile/workspace", () => {
  assert.equal(
    skillSelectionReason(skill, { workspaceId: "", profile: "supervised" }),
    "Choose a project first",
  );
  assert.equal(
    skillSelectionReason(skill, {
      workspaceId: "project-b",
      profile: "supervised",
    }),
    "Workspace is not granted",
  );
  assert.equal(
    skillSelectionReason(skill, {
      workspaceId: "project-a",
      profile: "full_access",
    }),
    "Permission profile is not granted",
  );
});
test("explicit selection never bypasses disabled, untrusted or non-invocable metadata", () => {
  assert.equal(skillSelectionReason({ ...skill, enabled: false }), "Disabled");
  assert.equal(skillSelectionReason({ ...skill, trusted: false }), "Untrusted");
  assert.equal(
    skillSelectionReason({ ...skill, user_invocable: false }),
    "Manual invocation unavailable",
  );
});
