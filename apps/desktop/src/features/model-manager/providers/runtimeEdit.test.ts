import { test } from "node:test";
import assert from "node:assert/strict";
import { AidexError } from "@aidex/client/errors";
import type { RuntimeConfig } from "@aidex/client/dto/runtime";
import { accountChangedMeanwhile, applyRuntimeEdit, updateStoredRuntime, type RuntimeCalls } from "./runtimeEdit";

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

const refused = (reason: string) =>
  new AidexError({ code: "conflict", kind: "conflict", message: "refused", data: { reason } });

/** Calls that serve `reads` in turn and refuse a write with `refusals` in turn. */
function fakeCalls(reads: RuntimeConfig[], refusals: (AidexError | null)[] = []) {
  const writes: RuntimeConfig[] = [];
  let read = 0;
  const calls: RuntimeCalls = {
    get: async () => reads[Math.min(read++, reads.length - 1)],
    update: async (config) => {
      writes.push(config);
      const refusal = refusals.shift();
      if (refusal) throw refusal;
      return config;
    },
  };
  return { calls, writes, reads: () => read };
}

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

test("the wizard's step changes the launch settings and sign-in but never the profile", () => {
  const auth = { kind: "vault", secret_name: "runtime-codex", env_name: "OPENAI_API_KEY" } as const;
  const saved = applyRuntimeEdit(stored, { executable: "c", argv: ["--x"], enabled: true, auth });
  assert.deepEqual(saved, { ...stored, executable: "c", argv: ["--x"], auth });
});

test("the edit does not share the screen's argument list", () => {
  const argv = ["--flag"];
  const saved = applyRuntimeEdit(stored, { executable: "a", argv });
  argv.push("--later");
  assert.deepEqual(saved.argv, ["--flag"]);
});

test("an edit is applied to the configuration read just before the write", async () => {
  // The screen still thinks no account is bound; the read says otherwise.
  const fake = fakeCalls([stored]);
  const saved = await updateStoredRuntime("codex", { executable: "b" }, fake.calls);
  assert.equal(saved.profile, "codex-p1");
  assert.deepEqual(fake.writes, [{ ...stored, executable: "b" }]);
});

test("an account bound between the read and the write is kept by one retry on a fresh read", async () => {
  const before = { ...stored, profile: null };
  const fake = fakeCalls([before, stored], [refused("conflict")]);
  const saved = await updateStoredRuntime("codex", { executable: "b" }, fake.calls);
  assert.equal(fake.reads(), 2);
  assert.equal(fake.writes.length, 2);
  assert.equal(saved.profile, "codex-p1", "the retry keeps the account that was bound meanwhile");
});

test("a second refusal, and any other failure, reach the caller without another write", async () => {
  const twice = fakeCalls([stored], [refused("conflict"), refused("conflict")]);
  await assert.rejects(updateStoredRuntime("codex", { enabled: false }, twice.calls), /refused/);
  assert.equal(twice.writes.length, 2);

  const busy = fakeCalls([stored], [refused("runtime_busy")]);
  await assert.rejects(updateStoredRuntime("codex", { enabled: false }, busy.calls), /refused/);
  assert.equal(busy.writes.length, 1, "a busy runtime is not retried");
});

test("only a profile conflict counts as an account bound meanwhile", () => {
  assert.equal(accountChangedMeanwhile(refused("conflict")), true);
  assert.equal(accountChangedMeanwhile(refused("runtime_busy")), false);
  assert.equal(accountChangedMeanwhile(new Error("conflict")), false);
});
