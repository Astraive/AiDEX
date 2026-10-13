import assert from "node:assert/strict";
import { test } from "node:test";
import { pageRangeLabel } from "./pageRange";

test("a page range names its first and last item", () => {
  assert.equal(pageRangeLabel(0, 20, 75), "1–20 of 75");
  assert.equal(pageRangeLabel(60, 15, 75), "61–75 of 75");
});

test("an empty page never reads 1–0", () => {
  assert.equal(pageRangeLabel(0, 0, 0), "0 of 0");
  assert.equal(pageRangeLabel(40, 0, 40), "0 of 40");
});
