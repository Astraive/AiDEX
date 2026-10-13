import assert from "node:assert/strict";
import { test } from "node:test";
import {
  CHECKPOINT_PAGE,
  newerWindow,
  newestPageOffset,
  olderWindow,
  shownWindow,
  type CheckpointWindow,
} from "./checkpointPages";

/** Offsets of the checkpoints `window` lists when the conversation holds `total` of them. */
const listed = (window: CheckpointWindow, total: number) => {
  const end = Math.min(total, window.offset + window.limit);
  return Array.from({ length: Math.max(0, end - window.offset) }, (_, index) => window.offset + index);
};

/** What `checkpoint.list` answers about the next page of `window`. */
const nextOffset = (window: CheckpointWindow, total: number) => {
  const end = Math.min(total, window.offset + window.limit);
  return end < total ? end : null;
};

test("the newest page holds the latest checkpoints", () => {
  assert.equal(newestPageOffset(0), 0);
  assert.equal(newestPageOffset(CHECKPOINT_PAGE), 0);
  assert.equal(newestPageOffset(CHECKPOINT_PAGE + 5), 5);
  assert.equal(newestPageOffset(130), 130 - CHECKPOINT_PAGE);
  assert.deepEqual(shownWindow(null, 25), { offset: 5, limit: CHECKPOINT_PAGE });
  assert.deepEqual(shownWindow({ offset: 0, limit: 5 }, 25), { offset: 0, limit: 5 });
});

test("older and newer pages walk back to the newest page without overlap", () => {
  const total = 45;
  const newest = shownWindow(null, total);
  const older = olderWindow(newest);
  assert.deepEqual(older, { offset: 5, limit: 20 });
  const oldest = olderWindow(older);
  assert.deepEqual(oldest, { offset: 0, limit: 5 });
  const seen = [oldest, older, newest].flatMap((window) => listed(window, total));
  assert.deepEqual(seen, Array.from({ length: total }, (_, index) => index));
  assert.deepEqual(newerWindow(total, nextOffset(oldest, total)), older);
  assert.equal(newerWindow(total, nextOffset(older, total)), null);
  assert.equal(newerWindow(total, nextOffset(newest, total)), undefined);
});

test("newer reaches checkpoints added after the conversation was read", () => {
  // conversation.get counted 25, so the newest page was chosen as 5..24; the list now counts 27.
  const shown = shownWindow(null, 25);
  const listTotal = 27;
  assert.equal(nextOffset(shown, listTotal), 25);
  const newer = newerWindow(listTotal, 25);
  assert.deepEqual(newer, { offset: 25, limit: CHECKPOINT_PAGE });
  assert.deepEqual(listed(shownWindow(newer!, 25), listTotal), [25, 26]);
  assert.equal(nextOffset(newer!, listTotal), null);
});

test("pages chosen before checkpoints were added still never overlap", () => {
  // Older from the newest page of 25, then five checkpoints are added.
  const older = olderWindow(shownWindow(null, 25));
  assert.deepEqual(older, { offset: 0, limit: 5 });
  const total = 30;
  const middle = newerWindow(total, nextOffset(older, total))!;
  assert.deepEqual(middle, { offset: 5, limit: CHECKPOINT_PAGE });
  const last = newerWindow(total, nextOffset(middle, total))!;
  assert.deepEqual(last, { offset: 25, limit: CHECKPOINT_PAGE });
  const seen = [older, middle, last].flatMap((window) => listed(window, total));
  assert.deepEqual(seen, Array.from({ length: total }, (_, index) => index));
});
