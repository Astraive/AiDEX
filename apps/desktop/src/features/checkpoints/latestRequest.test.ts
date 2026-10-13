import assert from "node:assert/strict";
import { test } from "node:test";
import { createLatestRequest } from "./latestRequest";

test("a late answer for an earlier selection is ignored", () => {
  const selection = createLatestRequest();
  const first = selection.begin();
  const second = selection.begin();
  assert.equal(selection.isCurrent(first), false);
  assert.equal(selection.isCurrent(second), true);
});

test("invalidating drops every request in flight", () => {
  const selection = createLatestRequest();
  const pending = selection.begin();
  selection.invalidate();
  assert.equal(selection.isCurrent(pending), false);
  assert.equal(selection.isCurrent(selection.begin()), true);
});
