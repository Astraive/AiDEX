import { afterEach, describe, test } from "node:test";
import assert from "node:assert/strict";
import {
  LAYER_ATTRIBUTE,
  NAVIGATION_FOCUS_WINDOW_MS,
  focusFollowsNavigationKey,
  hasModalLayer,
  isBelowModalLayer,
  isInsideDescendantLayer,
  isNavigationKey,
  isTopLayer,
  layerCount,
  noteKeyPress,
  onLayerPush,
  popLayer,
  pushLayer,
  topLayer,
  type LayerElement,
  type LayerTarget,
} from "./layers";

/** A stand-in element: bun has no DOM, and the stack needs only `contains` and `closest`. */
type StubNode = LayerElement & Required<Pick<LayerTarget, "closest">> & { parent: StubNode | null; marked: boolean };

function node(parent: StubNode | null = null, marked = false): StubNode {
  const self: StubNode = {
    parent,
    marked,
    contains(other) {
      for (let current = other as StubNode | null; current; current = current.parent) {
        if (current === self) return true;
      }
      return false;
    },
    closest(selector) {
      assert.equal(selector, `[${LAYER_ATTRIBUTE}]`);
      for (let current: StubNode | null = self; current; current = current.parent) {
        if (current.marked) return current;
      }
      return null;
    },
  };
  return self;
}

const cleanups: Array<() => void> = [];
function push(element: StubNode, modal = false) {
  const remove = pushLayer(element, { modal });
  cleanups.push(remove);
  return remove;
}

afterEach(() => {
  cleanups.splice(0).forEach((remove) => remove());
  assert.equal(layerCount(), 0);
});

describe("layer stack", () => {
  test("only the top layer answers Escape", () => {
    const a = node();
    const b = node();
    push(a);
    push(b);
    assert.equal(topLayer(), b);
    assert.equal(isTopLayer(b), true);
    assert.equal(isTopLayer(a), false);
    popLayer(b);
    assert.equal(isTopLayer(a), true);
  });

  test("the returned function removes only its own entry", () => {
    const a = node();
    const b = node();
    const removeA = push(a);
    push(b);
    removeA();
    removeA();
    assert.equal(layerCount(), 1);
    assert.equal(topLayer(), b);
  });

  test("pushing an element again moves it to the top", () => {
    const a = node();
    const b = node();
    push(a);
    push(b);
    push(a);
    assert.equal(layerCount(), 2);
    assert.equal(topLayer(), a);
  });

  test("a press inside a later layer is not outside an earlier one", () => {
    const body = node();
    const a = node(body);
    const b = node(body);
    const insideB = node(b);
    push(a);
    push(b);
    assert.equal(isInsideDescendantLayer(insideB, a), true);
    // The reverse is a real outside press: A sits under B.
    assert.equal(isInsideDescendantLayer(node(a), b), false);
    assert.equal(isInsideDescendantLayer(node(body), a), false);
    assert.equal(isInsideDescendantLayer(null, a), false);
  });

  test("a press inside an unregistered marked layer is not outside", () => {
    const body = node();
    const a = node(body);
    const tooltip = node(body, true);
    push(a);
    assert.equal(isInsideDescendantLayer(node(tooltip), a), true);
  });

  test("a marked ancestor or a marked layer below does not count as on top", () => {
    const body = node();
    const dialog = node(body, true);
    const popover = node(dialog);
    push(dialog, true);
    push(popover);
    // A press on the dialog around the popover is outside the popover.
    assert.equal(isInsideDescendantLayer(node(dialog), popover), false);
    // A marked layer that sits below in the stack is underneath, not on top.
    const other = node(body);
    push(other);
    const lower = node(body, true);
    cleanups.push(pushLayer(lower));
    popLayer(other);
    push(other);
    assert.equal(isInsideDescendantLayer(node(lower), other), false);
  });

  test("text nodes resolve through their parent element", () => {
    const body = node();
    const a = node(body);
    const tooltip = node(body, true);
    push(a);
    const text: LayerTarget = { parentElement: node(tooltip) };
    assert.equal(isInsideDescendantLayer(text, a), true);
  });

  test("hasModalLayer reports any open modal layer", () => {
    assert.equal(hasModalLayer(), false);
    const popover = node();
    push(popover);
    assert.equal(hasModalLayer(), false);
    const dialog = node();
    const removeDialog = push(dialog, true);
    assert.equal(hasModalLayer(), true);
    removeDialog();
    assert.equal(hasModalLayer(), false);
  });

  test("a modal layer pushed later is above an earlier layer it does not contain", () => {
    const body = node();
    const popover = node(body);
    push(popover);
    assert.equal(isBelowModalLayer(popover), false);
    // A non-modal layer on top (a select opened from the popover) does not count.
    const select = node(body);
    const removeSelect = push(select);
    assert.equal(isBelowModalLayer(popover), false);
    removeSelect();
    const dialog = node(body, true);
    const removeDialog = push(dialog, true);
    assert.equal(isBelowModalLayer(popover), true);
    assert.equal(isBelowModalLayer(dialog), false);
    removeDialog();
    assert.equal(isBelowModalLayer(popover), false);
    // A popover opened inside a dialog sits above it, and a later modal that contains it is not over it.
    const outer = node(body, true);
    push(outer, true);
    const inner = node(outer);
    push(inner);
    assert.equal(isBelowModalLayer(inner), false);
    const viewer = node(outer, true);
    push(viewer, true);
    assert.equal(isBelowModalLayer(inner), true);
    assert.equal(isBelowModalLayer(node(body)), false);
  });

  test("push listeners run after each push until they unsubscribe", () => {
    const seen: number[] = [];
    const unsubscribe = onLayerPush(() => seen.push(layerCount()));
    push(node());
    push(node(), true);
    unsubscribe();
    push(node());
    assert.deepEqual(seen, [1, 2]);
  });
});

describe("focus intent", () => {
  afterEach(() => noteKeyPress(null));

  test("Tab, arrows, Home/End, Page keys and F6 move focus; Enter, Escape and letters do not", () => {
    const keys = ["Tab", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "F6"];
    for (const key of keys) {
      assert.equal(isNavigationKey(key), true, key);
    }
    for (const key of ["Enter", " ", "Escape", "a", "Shift", "Control"]) {
      assert.equal(isNavigationKey(key), false, key);
    }
  });

  test("focus right after Tab follows navigation", () => {
    noteKeyPress("Tab", 1000);
    assert.equal(focusFollowsNavigationKey(1000), true);
    assert.equal(focusFollowsNavigationKey(1000 + NAVIGATION_FOCUS_WINDOW_MS), true);
  });

  test("focus placed after Enter or Escape (a dialog opening or closing) does not", () => {
    noteKeyPress("Enter", 1000);
    assert.equal(focusFollowsNavigationKey(1016), false);
    noteKeyPress("Escape", 2000);
    assert.equal(focusFollowsNavigationKey(2016), false);
  });

  test("a stale key, a pointer press, or no key at all does not", () => {
    noteKeyPress("Tab", 1000);
    assert.equal(focusFollowsNavigationKey(1001 + NAVIGATION_FOCUS_WINDOW_MS), false);
    noteKeyPress(null);
    assert.equal(focusFollowsNavigationKey(1000), false);
  });
});
