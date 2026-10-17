import { test } from "node:test";
import assert from "node:assert/strict";
import { AidexError } from "@aidex/client/errors";
import {
  consumerList,
  MANAGED_MESSAGE,
  managedSecret,
  MAX_CONSUMERS,
  secretPolicyError,
  secretWriteError,
  validConsumer,
  validSecretName,
} from "./secretPolicy";

test("secrets may target exact provider, MCP, process, and Git consumers", () => {
  for (const scope of [
    "provider:openai_work",
    "mcp:github",
    "process:runtime-codex-work",
    "process:web",
    "git:github.com",
    "git:git.example.org:2222",
  ])
    assert.equal(validConsumer(scope), true, scope);
  assert.deepEqual(consumerList("mcp:github\nprocess:runtime-codex-work, mcp:github\n\n"), [
    "mcp:github",
    "process:runtime-codex-work",
  ]);
  assert.equal(secretPolicyError("9.runtime-key", consumerList("mcp:github")), undefined);
  assert.equal(secretPolicyError("key", []), undefined);
});

test("wildcards, agents, prompts, shells, and malformed targets cannot receive secrets", () => {
  for (const scope of [
    "provider:*",
    "provider:",
    ":openai",
    "openai",
    "env:OPENAI_API_KEY",
    "model:gpt",
    "prompt:system",
    "shell:bash",
    "agent:native",
    "session:abc",
    "log:audit",
    "mcp:../github",
    "process:runtime-Upper",
    "git:",
    "git:Github.com",
    "git:a..b",
    "git:-host.com",
    "git:host:0",
    "git:host:65536",
    "git:host:22:33",
  ])
    assert.equal(validConsumer(scope), false, scope);
  assert.ok(secretPolicyError("key", ["agent:native"]));
  assert.ok(
    secretPolicyError(
      "key",
      Array.from({ length: MAX_CONSUMERS + 1 }, (_, index) => `mcp:server${index}`),
    ),
  );
});

test("secret names follow the vault's pattern", () => {
  for (const name of ["OPENAI_API_KEY", "runtime-codex.key", "9key", "a".repeat(128)]) assert.equal(validSecretName(name), true, name);
  for (const name of ["", "_key", ".key", "key with space", "key/slash", "a".repeat(129)])
    assert.equal(validSecretName(name), false, name);
  assert.ok(secretPolicyError("-key", []));
});

test("records AiDEX manages cannot be replaced or deleted from the vault screen", () => {
  for (const name of [
    "aidex-preferences",
    "aidex-model-preferences",
    "aidex-appearance",
    "native-record-encryption-key",
    "external-runtime-encryption-key",
    "oauth.openai.profile",
    "oauth.hosts",
  ])
    assert.equal(managedSecret({ name }), true, name);
  assert.equal(managedSecret({ name: "runtime-codex-work-key" }), false);
  assert.equal(managedSecret({ name: "OPENAI_API_KEY" }), false);
  assert.equal(managedSecret({ name: "future-managed-key" }, ["future-managed-key"]), true);
});

test("a managed_secret refusal from set or delete explains the managed record", () => {
  const refusal = (data: unknown) =>
    new AidexError({ code: "permission_denied", kind: "policy_denied", message: "daemon message", data });
  assert.equal(secretWriteError(refusal({ reason: "managed_secret" })), MANAGED_MESSAGE);
  assert.equal(secretWriteError(refusal({ reason: "secret_not_found" })), "daemon message");
  assert.equal(secretWriteError(refusal(undefined)), "daemon message");
  assert.equal(secretWriteError(refusal("managed_secret")), "daemon message");
  assert.equal(secretWriteError(refusal(null)), "daemon message");
});
