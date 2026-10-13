import { test } from "node:test";
import assert from "node:assert/strict";
import type { AppPreferences } from "@aidex/client/dto/preferences";
import type { ProviderModel } from "@aidex/client/dto/models";
import type { SessionConfig } from "@aidex/client/dto/sessions";
import { buildSessionConfig, type SessionConfigInput } from "./sessionConfig";
import { fallbackModelOptions, sessionSettingsErrors } from "./sessionSettings";

const defaults: AppPreferences = {
  accent: "#3b82f6",
  theme: "dark",
  density: "comfortable",
  reduce_motion: false,
  keyboard_hints: true,
  model_id: "",
  workspace_id: "",
  permission_mode: null,
  instructions: "Default instructions.",
  fallback_model: "openai/gpt-default-fallback",
  max_steps: 16,
  max_tokens: 4096,
  shortcuts: {},
};

const session: SessionConfig = {
  title: "Fix the build",
  workspace_id: "workspace-1",
  mode: "agent",
  provider_id: "openai",
  model_id: "gpt-main",
  runtime_id: null,
  permission_mode: "supervised",
  instructions: "Conversation instructions.",
  fallback_model: null,
  max_steps: 32,
  max_tokens: 8192,
};

const input = (overrides?: SessionConfigInput["overrides"]): SessionConfigInput => ({
  session,
  defaults,
  title: "",
  mode: "agent",
  workspaceId: "workspace-1",
  isRegisteredWorkspace: (id) => id === "workspace-1",
  target: { provider_id: "openai", model_id: "gpt-main", runtime_id: null },
  permission: "supervised",
  overrides,
});

test("a conversation keeps its own settings, including an explicit null fallback", () => {
  const config = buildSessionConfig(input());
  assert.equal(config.instructions, "Conversation instructions.");
  assert.equal(config.fallback_model, null);
  assert.equal(config.max_steps, 32);
  assert.equal(config.max_tokens, 8192);
});

test("a new conversation starts from the defaults", () => {
  const config = buildSessionConfig({ ...input(), session: undefined, title: "New" });
  assert.equal(config.title, "New");
  assert.equal(config.instructions, "Default instructions.");
  assert.equal(config.fallback_model, "openai/gpt-default-fallback");
  assert.equal(config.max_steps, 16);
  assert.equal(config.max_tokens, 4096);
});

test("overrides replace the conversation's values and leave the rest", () => {
  const config = buildSessionConfig(
    input({ instructions: "Be brief.", fallback_model: "anthropic/claude-fallback", max_steps: 128 }),
  );
  assert.equal(config.instructions, "Be brief.");
  assert.equal(config.fallback_model, "anthropic/claude-fallback");
  assert.equal(config.max_steps, 128);
  assert.equal(config.max_tokens, 8192);
  // The rest of the configuration is unchanged.
  assert.equal(config.title, "Fix the build");
  assert.equal(config.mode, "agent");
  assert.equal(config.permission_mode, "supervised");
});

test("an explicit null fallback override removes the fallback; an absent one keeps it", () => {
  const withFallback = { ...session, fallback_model: "openai/gpt-fallback" };
  assert.equal(
    buildSessionConfig({ ...input({ fallback_model: null }), session: withFallback }).fallback_model,
    null,
  );
  assert.equal(
    buildSessionConfig({ ...input({ max_tokens: 100 }), session: withFallback }).fallback_model,
    "openai/gpt-fallback",
  );
});

test("overrides outside the daemon's bounds are refused", () => {
  for (const overrides of [
    { max_steps: 0 },
    { max_steps: 129 },
    { max_steps: 1.5 },
    { max_tokens: 0 },
    { max_tokens: 32_769 },
    { instructions: "x".repeat(32 * 1024 + 1) },
    // 32 KiB is counted in UTF-8 bytes: 11,000 three-byte characters are over it.
    { instructions: "€".repeat(11_000) },
    { fallback_model: "runtime:codex/default" },
    { fallback_model: "no-slash" },
  ])
    assert.throws(() => buildSessionConfig(input(overrides)), Error, JSON.stringify(overrides).slice(0, 60));
  // The bounds themselves are accepted.
  const config = buildSessionConfig(
    input({ max_steps: 1, max_tokens: 32_768, instructions: "x".repeat(32 * 1024) }),
  );
  assert.equal(config.max_steps, 1);
  assert.equal(config.max_tokens, 32_768);
});

test("limit fields accept whole numbers written with digits only", () => {
  const draft = { instructions: "", fallback_model: "", max_steps: "16", max_tokens: "4096" };
  assert.deepEqual(sessionSettingsErrors(draft), {});
  for (const text of ["", " ", "-1", "1e2", "2.0", "0x10", "+5"]) {
    assert.ok(sessionSettingsErrors({ ...draft, max_steps: text }).max_steps, `steps ${JSON.stringify(text)}`);
    assert.ok(sessionSettingsErrors({ ...draft, max_tokens: text }).max_tokens, `tokens ${JSON.stringify(text)}`);
  }
});

const model = (provider: string, id: string, toolCalls: "native" | "none", chat = true): ProviderModel =>
  ({
    model: { provider_id: provider, model_id: id },
    display_name: id,
    capabilities: {
      chat,
      streaming: true,
      tool_calls: toolCalls,
      json_schema_output: false,
      vision: false,
      embeddings: false,
    },
  }) as unknown as ProviderModel;

test("fallback choices are native chat models; agents need tool calling", () => {
  const models = [
    model("openai", "gpt-main", "native"),
    model("openai", "gpt-tools", "native"),
    model("local-llama", "gemma", "none"),
    model("openai", "embedder", "none", false),
  ];
  const listed = [
    ...models.map((item) => ({ key: `${item.model.provider_id}/${item.model.model_id}`, name: item.display_name })),
    { key: "runtime:codex/default", name: "Default model" },
    { key: "openai/gone", name: "Gone", unavailable: true },
  ];
  const findModel = (key: string) =>
    models.find((item) => `${item.model.provider_id}/${item.model.model_id}` === key);
  const base = { models: listed, findModel, providerName: () => undefined, primaryKey: "openai/gpt-main" };
  assert.deepEqual(
    fallbackModelOptions(base).map((option) => option.value),
    ["", "openai/gpt-tools", "local-llama/gemma"],
  );
  assert.deepEqual(
    fallbackModelOptions({ ...base, agent: true }).map((option) => option.value),
    ["", "openai/gpt-tools"],
  );
  // A saved fallback that can no longer be offered stays visible instead of silently changing.
  const kept = fallbackModelOptions({ ...base, agent: true, current: "local-llama/gemma" });
  assert.deepEqual(kept.at(-1), { value: "local-llama/gemma", label: "gemma (can't use agent tools)" });
  assert.equal(
    fallbackModelOptions({ ...base, current: "openai/gone" }).at(-1)?.label,
    "gone (not available now)",
  );
});
