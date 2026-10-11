import { test } from "node:test";
import assert from "node:assert/strict";
import {
  consumerList,
  managedSecret,
  secretPolicyError,
  validConsumer,
} from "./secretPolicy";
test("user credentials may target exact provider, MCP, runtime process and Git consumers", () => {
  for (const scope of [
    "provider:openai_work",
    "mcp:github",
    "process:runtime-codex-work",
    "git:git.example.org:2222",
  ])
    assert.equal(validConsumer(scope), true, scope);
  assert.deepEqual(
    consumerList("mcp:github\nprocess:runtime-codex-work, mcp:github"),
    ["mcp:github", "process:runtime-codex-work"],
  );
  assert.equal(
    secretPolicyError("9.runtime-key", consumerList("mcp:github")),
    undefined,
  );
});
test("wildcards, model/prompt/shell recipients and invalid hosts cannot receive vault values", () => {
  for (const scope of [
    "provider:*",
    "env:OPENAI_API_KEY",
    "model:gpt",
    "prompt:system",
    "shell:bash",
    "agent:native",
    "log:audit",
    "mcp:../github",
    "process:runtime-Upper",
    "git:Github.com",
    "git:a..b",
    "git:host:0",
    "git:host:65536",
  ])
    assert.equal(validConsumer(scope), false, scope);
  assert.equal(secretPolicyError("key", []), undefined);
  assert.ok(
    secretPolicyError(
      "key",
      Array.from({ length: 33 }, (_, index) => `mcp:server${index}`),
    ),
  );
});
test("ordinary credential rotation cannot overwrite service encryption or account records", () => {
  for (const name of [
    "native-record-encryption-key",
    "external-runtime-encryption-key",
    "aidex-preferences",
    "oauth.openai.profile",
    "oauth.hosts",
  ])
    assert.equal(managedSecret({ name }), true);
  assert.equal(managedSecret({ name: "runtime-codex-work-key" }), false);
});
