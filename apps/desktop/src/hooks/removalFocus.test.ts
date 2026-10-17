import { test } from "node:test";
import assert from "node:assert/strict";
import { createRemovalFocus, removalFocusOrder } from "./removalFocus";

test("the row that took the deleted one's place comes first, then later rows, then earlier ones nearest first", () => {
  assert.deepEqual(removalFocusOrder(4, 1), [1, 2, 3, 0]);
  assert.deepEqual(removalFocusOrder(3, 0), [0, 1, 2]);
});

test("deleting the last row tries the new last row first", () => {
  assert.deepEqual(removalFocusOrder(3, 3), [2, 1, 0]);
  assert.deepEqual(removalFocusOrder(2, 9), [1, 0]);
});

test("an emptied list leaves no row to try, so the caller's fallback takes focus", () => {
  assert.deepEqual(removalFocusOrder(0, 0), []);
});

test("a list re-read after the dialog closed places focus that dropped as the row went", () => {
  const focus = createRemovalFocus();
  focus.removed(["a", "b", "c"], "b");
  // The dialog closed first: its opener in row b is still there, so focus returns to it.
  assert.equal(focus.dialogClosed(["a", "b", "c"]), undefined);
  assert.equal(focus.listChanged(["a", "b", "c"], "elsewhere"), undefined, "the row is still listed");
  assert.deepEqual(focus.listChanged(["a", "c"], "dropped"), [1, 0]);
  assert.equal(focus.listChanged(["c"], "dropped"), undefined, "a move happens once");
});

test("a list re-read while the dialog closes leaves focus to the dialog's close", () => {
  const focus = createRemovalFocus();
  focus.removed(["a", "b", "c"], "c");
  assert.equal(focus.listChanged(["a", "b"], "dialog"), undefined);
  assert.deepEqual(focus.dialogClosed(["a", "b"]), [1, 0]);
  assert.equal(focus.dialogClosed(["a", "b"]), undefined, "a move happens once");
});

test("deleting the only row hands focus to the fallback", () => {
  const focus = createRemovalFocus();
  focus.removed(["a"], "a");
  assert.deepEqual(focus.dialogClosed([]), []);
});

test("focus the user moved elsewhere before the row went is left alone", () => {
  const focus = createRemovalFocus();
  focus.removed(["a", "b"], "a");
  assert.equal(focus.listChanged(["b"], "elsewhere"), undefined);
  assert.equal(focus.listChanged([], "dropped"), undefined, "the delete is forgotten once the user moved on");
});

test("nothing moves without a delete", () => {
  const focus = createRemovalFocus();
  assert.equal(focus.listChanged(["a"], "dropped"), undefined);
  assert.equal(focus.dialogClosed([]), undefined);
});
