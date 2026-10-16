import assert from "node:assert/strict";
import { test } from "node:test";
import type { ArtifactData } from "./artifactContent";
import {
  EMPTY_PREVIEW,
  previewFailed,
  previewRead,
  previewStarted,
  shownPreview,
  type PreviewState,
} from "./artifactPreview";

const data: ArtifactData = { encoding: "utf8", text: "decrypted words", totalBytes: 15, complete: true };
const locked = { kind: "vault_locked", message: "the vault is locked" };

test("a read shows its content until the vault locks", () => {
  const ready = previewRead("artifact_a", 0, data);
  assert.deepEqual(shownPreview(ready, "artifact_a", 0), { status: "ready", data });
  // The render that sees the lock already hides it, before any re-read answers.
  assert.deepEqual(shownPreview(ready, "artifact_a", 1), { status: "locked" });
});

test("a re-read after a lock starts locked out, never with the old content", () => {
  const ready = previewRead("artifact_a", 0, data);
  const started = previewStarted(ready, "artifact_a", 1);
  assert.deepEqual(started, { id: "artifact_a", locks: 1, status: "locked" });
  assert.deepEqual(shownPreview(started, "artifact_a", 1), { status: "locked" });
});

test("a read refused as vault_locked keeps nothing; other failures show their message", () => {
  assert.deepEqual(previewFailed("artifact_a", 2, locked), { id: "artifact_a", locks: 2, status: "locked" });
  const missing = previewFailed("artifact_a", 2, { kind: "not_found", message: "gone" });
  assert.deepEqual(shownPreview(missing, "artifact_a", 2), { status: "failed", message: "gone" });
});

test("retries while locked out stay locked out until a read answers after the unlock", () => {
  let state: PreviewState = previewFailed("artifact_a", 1, locked);
  state = previewStarted(state, "artifact_a", 1);
  assert.equal(shownPreview(state, "artifact_a", 1).status, "locked");
  state = previewRead("artifact_a", 1, data);
  assert.deepEqual(shownPreview(state, "artifact_a", 1), { status: "ready", data });
});

test("another artifact starts empty, and nothing held for one shows for another", () => {
  assert.deepEqual(shownPreview(EMPTY_PREVIEW, "artifact_a", 0), { status: "loading" });
  const ready = previewRead("artifact_a", 0, data);
  assert.deepEqual(previewStarted(ready, "artifact_b", 0), { id: "artifact_b", locks: 0, status: "loading" });
  assert.deepEqual(shownPreview(ready, "artifact_b", 0), { status: "loading" });
  // A first read after earlier locks is an ordinary load.
  assert.deepEqual(previewStarted(EMPTY_PREVIEW, "artifact_a", 3), { id: "artifact_a", locks: 3, status: "loading" });
});
