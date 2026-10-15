// Pure keyboard model for `FileTree` (the WAI-ARIA APG tree pattern): no DOM or React, so it runs
// in unit tests and during prerendering.

/** The shape `FileTree` nodes share; `children` only matter for folders. */
export type TreeKeyNode = {
  id: string;
  name: string;
  kind: "folder" | "file";
  children?: readonly TreeKeyNode[];
};

/** One visible row of the tree, in display order. */
export type FlatTreeNode = {
  id: string;
  name: string;
  kind: "folder" | "file";
  /** 1 for top-level rows (`aria-level`). */
  level: number;
  parentId: string | null;
  /** 1-based position among its siblings (`aria-posinset`). */
  position: number;
  /** Number of siblings, itself included (`aria-setsize`). */
  setSize: number;
  /** Folders only: whether its children are shown. */
  expanded: boolean;
  /** Folders only: whether it has any children to show. */
  hasChildren: boolean;
};

/** What a key press does to the tree. */
export type TreeKeyAction =
  /** Not a tree key here: let it through. */
  | { type: "none" }
  /** Move focus to this row. */
  | { type: "focus"; id: string }
  /** Show the folder's children; focus stays on the folder. */
  | { type: "expand"; id: string }
  /** Hide the folder's children; focus stays on the folder. */
  | { type: "collapse"; id: string }
  /** Enter or Space: toggle a folder, pick a file. */
  | { type: "activate"; id: string }
  /** `*`: show every sibling folder of the focused row (focus stays). */
  | { type: "expandSiblings"; ids: string[] };

/** The rows a tree shows: top-level nodes, and the children of every expanded folder, depth first. */
export function flattenVisibleTree(nodes: readonly TreeKeyNode[], expanded: ReadonlySet<string>): FlatTreeNode[] {
  const rows: FlatTreeNode[] = [];
  const walk = (list: readonly TreeKeyNode[], level: number, parentId: string | null) => {
    list.forEach((node, index) => {
      const isFolder = node.kind === "folder";
      const hasChildren = isFolder && (node.children?.length ?? 0) > 0;
      const isOpen = isFolder && expanded.has(node.id);
      rows.push({
        id: node.id,
        name: node.name,
        kind: node.kind,
        level,
        parentId,
        position: index + 1,
        setSize: list.length,
        expanded: isOpen,
        hasChildren,
      });
      if (isOpen && hasChildren) walk(node.children ?? [], level + 1, node.id);
    });
  };
  walk(nodes, 1, null);
  return rows;
}

/** A single printable character, which starts or extends a typeahead search. */
export function isTypeaheadKey(key: string): boolean {
  return Array.from(key).length === 1 && key !== " " && key !== "*";
}

/**
 * The visible row whose name starts with `search` (case-insensitive), looking after the current row
 * and wrapping. A search longer than one character also accepts the current row, so typing more of
 * the same name keeps focus where it is.
 */
export function typeaheadMatch(rows: readonly FlatTreeNode[], currentId: string | null, search: string): string | null {
  const needle = search.toLowerCase();
  if (!needle || rows.length === 0) return null;
  const current = currentId === null ? -1 : rows.findIndex((row) => row.id === currentId);
  const start = Array.from(needle).length > 1 && current >= 0 ? current : current + 1;
  for (let step = 0; step < rows.length; step += 1) {
    const row = rows[(start + step + rows.length) % rows.length];
    if (row.name.toLowerCase().startsWith(needle)) return row.id;
  }
  return null;
}

/**
 * What `key` does with focus on `currentId` (the caller skips Alt, Ctrl and Meta presses):
 * - ArrowDown / ArrowUp: the next or previous visible row (no wrap).
 * - ArrowRight: a closed folder opens; an open folder moves to its first child.
 * - ArrowLeft: an open folder closes; any other row moves to its parent.
 * - Home / End: the first or last visible row.
 * - Enter / Space: activate the row.
 * - `*`: open every sibling folder.
 * - A printable character: typeahead; pass the whole buffered `search` when keys arrive in a burst.
 */
export function nextTreeFocus(
  rows: readonly FlatTreeNode[],
  currentId: string | null,
  key: string,
  search: string = key,
): TreeKeyAction {
  if (rows.length === 0) return { type: "none" };
  const index = currentId === null ? -1 : rows.findIndex((row) => row.id === currentId);
  const row = index >= 0 ? rows[index] : undefined;
  const focus = (target: FlatTreeNode | undefined): TreeKeyAction =>
    target && target.id !== currentId ? { type: "focus", id: target.id } : { type: "none" };

  switch (key) {
    case "ArrowDown":
      return focus(row ? rows[index + 1] : rows[0]);
    case "ArrowUp":
      return focus(row ? rows[index - 1] : rows[rows.length - 1]);
    case "Home":
      return focus(rows[0]);
    case "End":
      return focus(rows[rows.length - 1]);
    case "ArrowRight": {
      if (!row || row.kind !== "folder" || !row.hasChildren) return { type: "none" };
      if (!row.expanded) return { type: "expand", id: row.id };
      const child = rows[index + 1];
      return child && child.parentId === row.id ? { type: "focus", id: child.id } : { type: "none" };
    }
    case "ArrowLeft": {
      if (!row) return { type: "none" };
      if (row.kind === "folder" && row.expanded) return { type: "collapse", id: row.id };
      return focus(rows.find((candidate) => candidate.id === row.parentId));
    }
    case "Enter":
    case " ":
      return row ? { type: "activate", id: row.id } : { type: "none" };
    case "*": {
      if (!row) return { type: "none" };
      const ids = rows
        .filter((sibling) => sibling.parentId === row.parentId && sibling.hasChildren && !sibling.expanded)
        .map((sibling) => sibling.id);
      return ids.length > 0 ? { type: "expandSiblings", ids } : { type: "none" };
    }
    default: {
      if (!isTypeaheadKey(key)) return { type: "none" };
      const match = typeaheadMatch(rows, currentId, search);
      return match && match !== currentId ? { type: "focus", id: match } : { type: "none" };
    }
  }
}
