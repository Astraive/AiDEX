import { test } from "node:test";
import assert from "node:assert/strict";
import type { CheckReport } from "@aidex/client/dto/updates";
import {
  absoluteUpdateSource,
  applySelectionReason,
  sameCandidate,
} from "./updateSelection";
const checked: CheckReport = {
  source: { type: "local_file", path: "C:/packages/aidex.zip" },
  package: "C:/packages/aidex.zip",
  checksum: "verified",
  candidate: {
    version: "0.2.0",
    target: "windows-x64",
    manifest_sha256: "digest-a",
    created_at: "2026-10-11T00:00:00Z",
    git_commit: null,
    signature: {
      status: "verified",
      key_id: "release-key",
      key_name: "Release",
      key_source: "release",
    },
    sbom: true,
  },
  current: null,
  previous: null,
  action: "install",
};
test("manual update sources require host-absolute paths and never imply an online catalog", () => {
  assert.equal(
    absoluteUpdateSource("C:/packages/aidex.zip", "windows-x64"),
    true,
  );
  assert.equal(
    absoluteUpdateSource("\\\\server\\share\\aidex.zip", "windows-x64"),
    true,
  );
  assert.equal(absoluteUpdateSource("C:relative.zip", "windows-x64"), false);
  assert.equal(
    absoluteUpdateSource("https://vendor/aidex.zip", "windows-x64"),
    false,
  );
  assert.equal(
    absoluteUpdateSource("/packages/aidex.tar.gz", "linux-x64"),
    true,
  );
});
test("unsigned, foreign-target, pending, and already-active updates cannot be applied through the normal UI", () => {
  assert.equal(applySelectionReason(checked, "windows-x64", false), undefined);
  assert.ok(
    applySelectionReason(
      {
        ...checked,
        candidate: {
          ...checked.candidate,
          signature: { status: "unsigned_allowed" },
        },
      },
      "windows-x64",
      false,
    ),
  );
  assert.ok(applySelectionReason(checked, "linux-x64", false));
  assert.ok(applySelectionReason(checked, "windows-x64", true));
  assert.ok(
    applySelectionReason(
      { ...checked, action: "up_to_date" },
      "windows-x64",
      false,
    ),
  );
});
test("a source recheck with another signed manifest requires fresh review", () => {
  assert.equal(sameCandidate(checked, { ...checked }), true);
  assert.equal(
    sameCandidate(checked, {
      ...checked,
      candidate: { ...checked.candidate, manifest_sha256: "digest-b" },
    }),
    false,
  );
});
