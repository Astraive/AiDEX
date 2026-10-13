import { test } from "node:test";
import assert from "node:assert/strict";
import type { CheckReport } from "@aidex/client/dto/updates";
import { absoluteUpdateSource, applySelectionReason, sameCandidate } from "./updateSelection";

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
    signature: { status: "verified", key_id: "release-key", key_name: "Release", key_source: "release" },
    sbom: true,
  },
  current: null,
  previous: null,
  action: "install",
};

test("manual update sources must be absolute paths on this host", () => {
  assert.equal(absoluteUpdateSource("C:/packages/aidex.zip", "windows-x64"), true);
  assert.equal(absoluteUpdateSource("D:\\releases", "windows-x64"), true);
  assert.equal(absoluteUpdateSource("/packages/aidex.tar.gz", "linux-x64"), true);
  assert.equal(absoluteUpdateSource("/Volumes/AIDEX/release", "macos-arm64"), true);

  assert.equal(absoluteUpdateSource("", "windows-x64"), false);
  assert.equal(absoluteUpdateSource("C:relative.zip", "windows-x64"), false);
  assert.equal(absoluteUpdateSource("packages\\aidex.zip", "windows-x64"), false);
  assert.equal(absoluteUpdateSource("/packages/aidex.zip", "windows-x64"), false);
  assert.equal(absoluteUpdateSource("C:/packages/aidex.zip", "linux-x64"), false);
  assert.equal(absoluteUpdateSource("./aidex.tar.gz", "linux-x64"), false);
  assert.equal(absoluteUpdateSource("https://vendor.example/aidex.zip", "windows-x64"), false);
  assert.equal(absoluteUpdateSource("file:///packages/aidex.tar.gz", "linux-x64"), false);
  assert.equal(absoluteUpdateSource("/packages/aidex\n.tar.gz", "linux-x64"), false);
});

test("network shares and device paths are not local update sources", () => {
  for (const source of [
    "\\\\server\\share\\aidex.zip",
    "//server/share/aidex.zip",
    "\\\\?\\C:\\packages\\aidex.zip",
    "\\\\?\\UNC\\server\\share\\aidex.zip",
    "\\\\.\\pipe\\aidex",
    "\\\\.\\PhysicalDrive0",
  ])
    assert.equal(absoluteUpdateSource(source, "windows-x64"), false, source);
});

test("unsigned, foreign-target, pending, and already-selected packages cannot be applied", () => {
  assert.equal(applySelectionReason(checked, "windows-x64", false), undefined);
  assert.ok(applySelectionReason(undefined, "windows-x64", false));
  assert.ok(applySelectionReason(checked, undefined, false));
  assert.ok(
    applySelectionReason(
      { ...checked, candidate: { ...checked.candidate, signature: { status: "unsigned_allowed" } } },
      "windows-x64",
      false,
    ),
  );
  assert.ok(applySelectionReason(checked, "linux-x64", false));
  assert.ok(applySelectionReason(checked, "windows-x64", true));
  assert.ok(applySelectionReason({ ...checked, action: "up_to_date" }, "windows-x64", false));
});

test("a re-check that finds another package, manifest, or signer needs a fresh review", () => {
  assert.equal(sameCandidate(checked, { ...checked }), true);
  assert.equal(
    sameCandidate(checked, { ...checked, candidate: { ...checked.candidate, manifest_sha256: "digest-b" } }),
    false,
  );
  assert.equal(sameCandidate(checked, { ...checked, package: "C:/packages/aidex-0.3.0.zip" }), false);
  assert.equal(sameCandidate(checked, { ...checked, candidate: { ...checked.candidate, version: "0.2.1" } }), false);
  assert.equal(
    sameCandidate(checked, {
      ...checked,
      candidate: {
        ...checked.candidate,
        signature: { status: "verified", key_id: "other-key", key_name: "Other", key_source: "explicit" },
      },
    }),
    false,
  );
  assert.equal(
    sameCandidate(checked, { ...checked, candidate: { ...checked.candidate, signature: { status: "unsigned_allowed" } } }),
    false,
  );
});
