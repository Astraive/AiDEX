import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { flattenVisibleTree, nextTreeFocus, typeaheadMatch, type TreeKeyNode } from "./fileTreeKeys";

const TREE: TreeKeyNode[] = [
  {
    id: "src",
    name: "src",
    kind: "folder",
    children: [
      {
        id: "src/app",
        name: "app",
        kind: "folder",
        children: [{ id: "src/app/main.ts", name: "main.ts", kind: "file" }],
      },
      { id: "src/index.ts", name: "index.ts", kind: "file" },
    ],
  },
  { id: "docs", name: "docs", kind: "folder", children: [{ id: "docs/guide.md", name: "guide.md", kind: "file" }] },
  { id: "empty", name: "empty", kind: "folder", children: [] },
  { id: "README.md", name: "README.md", kind: "file" },
];

const ids = (expanded: string[]) => flattenVisibleTree(TREE, new Set(expanded)).map((row) => row.id);

describe("flattenVisibleTree", () => {
  test("lists top-level rows and the children of open folders only", () => {
    assert.deepEqual(ids([]), ["src", "docs", "empty", "README.md"]);
    assert.deepEqual(ids(["src"]), ["src", "src/app", "src/index.ts", "docs", "empty", "README.md"]);
    // A child folder's own state counts only while its parent is open.
    assert.deepEqual(ids(["src/app"]), ["src", "docs", "empty", "README.md"]);
  });

  test("reports level, parent and position for the ARIA attributes", () => {
    const rows = flattenVisibleTree(TREE, new Set(["src", "src/app"]));
    const main = rows.find((row) => row.id === "src/app/main.ts");
    assert.deepEqual(
      main && { level: main.level, parentId: main.parentId, position: main.position, setSize: main.setSize },
      { level: 3, parentId: "src/app", position: 1, setSize: 1 },
    );
    const readme = rows.find((row) => row.id === "README.md");
    assert.equal(readme?.position, 4);
    assert.equal(readme?.setSize, 4);
    assert.equal(rows.find((row) => row.id === "empty")?.hasChildren, false);
  });
});

describe("nextTreeFocus", () => {
  test("Right on a closed folder opens it, and a second Right moves to its first child", () => {
    const closed = flattenVisibleTree(TREE, new Set());
    assert.deepEqual(nextTreeFocus(closed, "src", "ArrowRight"), { type: "expand", id: "src" });
    const open = flattenVisibleTree(TREE, new Set(["src"]));
    assert.deepEqual(nextTreeFocus(open, "src", "ArrowRight"), { type: "focus", id: "src/app" });
  });

  test("Right does nothing on a file or an empty folder", () => {
    const rows = flattenVisibleTree(TREE, new Set());
    assert.deepEqual(nextTreeFocus(rows, "README.md", "ArrowRight"), { type: "none" });
    assert.deepEqual(nextTreeFocus(rows, "empty", "ArrowRight"), { type: "none" });
  });

  test("Left on a child moves to the parent, and Left on an open folder closes it", () => {
    const rows = flattenVisibleTree(TREE, new Set(["src", "src/app"]));
    assert.deepEqual(nextTreeFocus(rows, "src/app/main.ts", "ArrowLeft"), { type: "focus", id: "src/app" });
    assert.deepEqual(nextTreeFocus(rows, "src/index.ts", "ArrowLeft"), { type: "focus", id: "src" });
    assert.deepEqual(nextTreeFocus(rows, "src/app", "ArrowLeft"), { type: "collapse", id: "src/app" });
    assert.deepEqual(nextTreeFocus(rows, "README.md", "ArrowLeft"), { type: "none" });
  });

  test("Up and Down walk the visible rows without wrapping", () => {
    const rows = flattenVisibleTree(TREE, new Set(["src"]));
    assert.deepEqual(nextTreeFocus(rows, "src", "ArrowDown"), { type: "focus", id: "src/app" });
    assert.deepEqual(nextTreeFocus(rows, "src/index.ts", "ArrowDown"), { type: "focus", id: "docs" });
    assert.deepEqual(nextTreeFocus(rows, "docs", "ArrowUp"), { type: "focus", id: "src/index.ts" });
    assert.deepEqual(nextTreeFocus(rows, "src", "ArrowUp"), { type: "none" });
    assert.deepEqual(nextTreeFocus(rows, "README.md", "ArrowDown"), { type: "none" });
  });

  test("Home and End reach the first and the last visible row", () => {
    const rows = flattenVisibleTree(TREE, new Set(["docs"]));
    assert.deepEqual(nextTreeFocus(rows, "docs/guide.md", "Home"), { type: "focus", id: "src" });
    assert.deepEqual(nextTreeFocus(rows, "src", "End"), { type: "focus", id: "README.md" });
    // The last row in display order: an open folder's children come before its later siblings.
    const nested = flattenVisibleTree([TREE[0]], new Set(["src", "src/app"]));
    assert.deepEqual(nextTreeFocus(nested, "src", "End"), { type: "focus", id: "src/index.ts" });
  });

  test("Enter and Space activate the row", () => {
    const rows = flattenVisibleTree(TREE, new Set());
    assert.deepEqual(nextTreeFocus(rows, "src", "Enter"), { type: "activate", id: "src" });
    assert.deepEqual(nextTreeFocus(rows, "README.md", " "), { type: "activate", id: "README.md" });
  });

  test("* opens every closed sibling folder that has children", () => {
    const rows = flattenVisibleTree(TREE, new Set());
    assert.deepEqual(nextTreeFocus(rows, "README.md", "*"), { type: "expandSiblings", ids: ["src", "docs"] });
    const open = flattenVisibleTree(TREE, new Set(["src", "docs"]));
    assert.deepEqual(nextTreeFocus(open, "src", "*"), { type: "none" });
  });

  test("typeahead jumps to the next row starting with the typed text, wrapping", () => {
    const rows = flattenVisibleTree(TREE, new Set(["src"]));
    assert.deepEqual(nextTreeFocus(rows, "src", "d"), { type: "focus", id: "docs" });
    assert.deepEqual(nextTreeFocus(rows, "docs", "S"), { type: "focus", id: "src" });
    assert.deepEqual(nextTreeFocus(rows, "src", "i"), { type: "focus", id: "src/index.ts" });
    assert.deepEqual(nextTreeFocus(rows, "src", "x"), { type: "none" });
  });

  test("a longer typeahead search keeps a row that still matches", () => {
    const rows = flattenVisibleTree(TREE, new Set());
    assert.equal(typeaheadMatch(rows, "README.md", "re"), "README.md");
    assert.deepEqual(nextTreeFocus(rows, "README.md", "e", "re"), { type: "none" });
    assert.deepEqual(nextTreeFocus(rows, "src", "o", "do"), { type: "focus", id: "docs" });
  });

  test("other keys and an empty tree are not handled", () => {
    const rows = flattenVisibleTree(TREE, new Set());
    assert.deepEqual(nextTreeFocus(rows, "src", "Tab"), { type: "none" });
    assert.deepEqual(nextTreeFocus([], null, "ArrowDown"), { type: "none" });
  });
});
