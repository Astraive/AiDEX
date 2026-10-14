import { test } from "node:test";
import assert from "node:assert/strict";
import type { RuntimeConfig } from "@aidex/client/dto/runtime";
import { applyRuntimeEdit } from "./runtimeEdit";

// The configuration as stored after an account import bound it: the screen's cached copy may
// still show `profile: null` and the old `auth`.
const stored: RuntimeConfig = {
  id: "codex",
  driver: "codex",
  executable: "a",
  argv: ["--old"],
  profile: "codex-p1",
  enabled: true,
  auth: { kind: "none" },
};

test("a Connection save changes only the launch settings and keeps the bound account", () => {
  const saved = applyRuntimeEdit(stored, { executable: "b", argv: [] });
  assert.equal(saved.executable, "b");
  assert.deepEqual(saved.argv, []);
  assert.equal(saved.profile, "codex-p1");
  assert.deepEqual(saved.auth, { kind: "none" });
  assert.equal(saved.enabled, true);
  assert.equal(saved.id, "codex");
});

test("the enable switch changes only `enabled`", () => {
  const saved = applyRuntimeEdit(stored, { enabled: false });
  assert.deepEqual(saved, { ...stored, enabled: false });
});

test("the edit does not share the screen's argument list", () => {
  const argv = ["--flag"];
  const saved = applyRuntimeEdit(stored, { executable: "a", argv });
  argv.push("--later");
  assert.deepEqual(saved.argv, ["--flag"]);
});
