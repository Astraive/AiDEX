import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { segmentedSelection } from "./segmentedSelection";

const OPTIONS = [{ value: "compact" }, { value: "comfortable" }, { value: "spacious" }] as const;

describe("segmentedSelection", () => {
  test("an unknown value checks nothing and keeps the first option as the tab stop", () => {
    assert.deepEqual(segmentedSelection<string>(OPTIONS, "huge"), { checkedIndex: -1, tabStopIndex: 0 });
  });

  test("a missing value checks nothing", () => {
    assert.deepEqual(segmentedSelection<string>(OPTIONS, undefined), { checkedIndex: -1, tabStopIndex: 0 });
  });

  test("a match is both the checked option and the tab stop", () => {
    assert.deepEqual(segmentedSelection<string>(OPTIONS, "comfortable"), { checkedIndex: 1, tabStopIndex: 1 });
    assert.deepEqual(segmentedSelection<string>(OPTIONS, "spacious"), { checkedIndex: 2, tabStopIndex: 2 });
    assert.deepEqual(segmentedSelection<string>(OPTIONS, "compact"), { checkedIndex: 0, tabStopIndex: 0 });
  });

  test("no options give no tab stop beyond index 0", () => {
    assert.deepEqual(segmentedSelection<string>([], "compact"), { checkedIndex: -1, tabStopIndex: 0 });
  });
});
