import { test } from "node:test";
import assert from "node:assert/strict";
import type { AppPreferences } from "@aidex/client/dto/preferences";
import {
  nativeDefaultsChanged,
  nativeDefaultsDraft,
  nativeDefaultsPatch,
  validateNativeDefaults,
  type NativeDefaultsDraft,
} from "./nativeDefaults";

const settings: AppPreferences = {
  accent: "#3b82f6",
  theme: "dark",
  density: "comfortable",
  reduce_motion: false,
  keyboard_hints: true,
  model_id: "openai/gpt-main",
  workspace_id: "",
  permission_mode: null,
  instructions: "",
  fallback_model: null,
  max_steps: 16,
  max_tokens: 4096,
  shortcuts: {},
};

const draft = (patch: Partial<NativeDefaultsDraft> = {}): NativeDefaultsDraft => ({
  ...nativeDefaultsDraft(settings),
  ...patch,
});

test("the saved defaults load as an unchanged, valid draft", () => {
  const loaded = nativeDefaultsDraft(settings);
  assert.equal(loaded.permission_mode, "supervised");
  assert.equal(loaded.fallback_model, "");
  assert.equal(loaded.max_steps, "16");
  assert.deepEqual(validateNativeDefaults(loaded), {});
  assert.equal(nativeDefaultsChanged(loaded, settings), false);
  assert.equal(nativeDefaultsChanged({ ...loaded, max_tokens: "8192" }, settings), true);
});

test("limits follow SessionConfig::validate: 1-128 steps and 1-32768 tokens", () => {
  for (const max_steps of ["1", "128"]) assert.equal(validateNativeDefaults(draft({ max_steps })).max_steps, undefined);
  for (const max_steps of ["0", "129", "", "1.5", "-3"])
    assert.ok(validateNativeDefaults(draft({ max_steps })).max_steps, max_steps);
  for (const max_tokens of ["1", "32768"])
    assert.equal(validateNativeDefaults(draft({ max_tokens })).max_tokens, undefined);
  for (const max_tokens of ["0", "32769", "abc"])
    assert.ok(validateNativeDefaults(draft({ max_tokens })).max_tokens, max_tokens);
});

test("instructions are limited to 32 KiB of UTF-8", () => {
  assert.equal(validateNativeDefaults(draft({ instructions: "a".repeat(32 * 1024) })).instructions, undefined);
  assert.ok(validateNativeDefaults(draft({ instructions: "a".repeat(32 * 1024 + 1) })).instructions);
  // 16,384 two-byte characters are exactly 32 KiB; one more is over.
  assert.equal(validateNativeDefaults(draft({ instructions: "é".repeat(16_384) })).instructions, undefined);
  assert.ok(validateNativeDefaults(draft({ instructions: "é".repeat(16_385) })).instructions);
});

test("the default may be native or a runtime; the fallback must be native", () => {
  assert.equal(validateNativeDefaults(draft({ model_id: "runtime:codex/default" })).model_id, undefined);
  assert.equal(validateNativeDefaults(draft({ model_id: "" })).model_id, undefined);
  assert.ok(validateNativeDefaults(draft({ model_id: "no-slash" })).model_id);
  assert.ok(validateNativeDefaults(draft({ model_id: `openai/${"m".repeat(512)}` })).model_id);
  assert.equal(validateNativeDefaults(draft({ fallback_model: "anthropic/claude" })).fallback_model, undefined);
  assert.ok(validateNativeDefaults(draft({ fallback_model: "runtime:codex/default" })).fallback_model);
});

test("the patch carries numbers and a null fallback, and refuses invalid drafts", () => {
  assert.deepEqual(
    nativeDefaultsPatch(
      draft({ permission_mode: "auto_accept_edits", instructions: "Be brief.", max_steps: "64", max_tokens: "2048" }),
    ),
    {
      model_id: "openai/gpt-main",
      permission_mode: "auto_accept_edits",
      instructions: "Be brief.",
      fallback_model: null,
      max_steps: 64,
      max_tokens: 2048,
    },
  );
  assert.throws(() => nativeDefaultsPatch(draft({ max_steps: "200" })), /1 to 128/);
});
