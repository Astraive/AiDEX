import { test } from "node:test";
import assert from "node:assert/strict";
import type { CheckReport, JournalSummary } from "@aidex/client/dto/updates";
import {
  PROGRESS_POLL_MS,
  SLOW_PROGRESS_POLL_MS,
  UNCHANGED_READS_BEFORE_SLOWING,
  absoluteUpdateSource,
  applySelectionReason,
  countUnchangedReads,
  followedOutcome,
  journalProgressKey,
  progressPollInterval,
  reviewReducer,
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

test("the status is polled only while the update journal can still change", () => {
  const poll = (input: Partial<Parameters<typeof progressPollInterval>[0]>) =>
    progressPollInterval({ busy: false, pending: false, recoveryError: false, unchangedReads: 0, ...input });
  assert.equal(poll({ busy: true }), PROGRESS_POLL_MS, "an operation of this page is running");
  assert.equal(poll({ pending: true }), PROGRESS_POLL_MS, "an unfinished update, e.g. one the CLI runs");
  assert.equal(
    poll({ pending: true, recoveryError: true }),
    undefined,
    "startup recovery failed: nothing changes until AiDEX Core restarts, so the files are not re-hashed",
  );
  assert.equal(poll({}), undefined);
  assert.equal(poll({ recoveryError: true }), undefined);
  assert.equal(
    poll({ pending: true, unchangedReads: UNCHANGED_READS_BEFORE_SLOWING }),
    SLOW_PROGRESS_POLL_MS,
    "a journal stuck at one step is read less often",
  );
  assert.equal(poll({ busy: true, unchangedReads: UNCHANGED_READS_BEFORE_SLOWING + 5 }), SLOW_PROGRESS_POLL_MS);
});

const journal = (state: JournalSummary["state"], updated_at = "2026-10-14T10:00:00Z"): JournalSummary => ({
  id: "upd_1",
  operation: "apply",
  state,
  started_at: "2026-10-14T10:00:00Z",
  updated_at,
  candidate_version: "0.2.0",
  to: { version: "0.2.0", dir: "versions/0.2.0", manifest_sha256: "digest-a" },
  signer_key_id: "release-key",
  outcome: null,
});

test("reads count as unchanged until the journal moves to another step or write", () => {
  let reads = countUnchangedReads(undefined, journalProgressKey(journal("staging")));
  assert.equal(reads.count, 0);
  reads = countUnchangedReads(reads, journalProgressKey(journal("staging")));
  reads = countUnchangedReads(reads, journalProgressKey(journal("staging")));
  assert.equal(reads.count, 2);
  reads = countUnchangedReads(reads, journalProgressKey(journal("self_test")));
  assert.equal(reads.count, 0);
  reads = countUnchangedReads(reads, journalProgressKey(journal("self_test", "2026-10-14T10:05:00Z")));
  assert.equal(reads.count, 0);
  assert.equal(journalProgressKey(null), "none");
});

test("opening a confirmation drops an error from an earlier operation, one raised in it stays", () => {
  const refused = reviewReducer({}, { type: "error", error: "Enter the absolute path of a package." });
  const opened = reviewReducer(refused, { type: "open", review: "rollback" });
  assert.deepEqual(opened, { review: "rollback", error: undefined });
  const failed = reviewReducer(opened, { type: "error", error: "The previous version changed." });
  assert.deepEqual(failed, { review: "rollback", error: "The previous version changed." });
  const closed = reviewReducer(failed, { type: "close" });
  assert.deepEqual(closed, { review: undefined, error: "The previous version changed." }, "shown on the page");
  assert.equal(reviewReducer(closed, { type: "open", review: "apply" }).error, undefined);
  assert.deepEqual(reviewReducer(failed, { type: "reset" }), {});
});

test("an operation the page stopped waiting for reports the journal it left", () => {
  const since = Date.parse("2026-10-14T09:59:00Z");
  const done = followedOutcome("apply", since, journal("committed"));
  assert.equal(done.tone, "success");
  assert.ok(done.text.includes("0.2.0"));
  const aborted = followedOutcome("apply", since, { ...journal("aborted"), outcome: "self_test_failed" });
  assert.equal(aborted.tone, "danger");
  assert.ok(aborted.text.includes("self test failed"));
  assert.equal(followedOutcome("rollback", since, journal("committed")).tone, "danger", "another operation");
  assert.equal(
    followedOutcome("apply", Date.parse("2026-10-14T11:00:00Z"), journal("committed")).tone,
    "danger",
    "an older journal does not describe this attempt",
  );
  assert.equal(followedOutcome("apply", since, null).tone, "danger");
});
