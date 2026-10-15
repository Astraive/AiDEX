"use client";

import {
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ComponentPropsWithRef,
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { ChevronRightIcon, FileIcon, FolderIcon } from "../../icons";
import { cx } from "../../utils/cx";
import { flattenVisibleTree, isTypeaheadKey, nextTreeFocus, type FlatTreeNode } from "./fileTreeKeys";
import styles from "./FileTree.module.css";

export type TreeNode = {
  id: string;
  name: string;
  kind: "folder" | "file";
  children?: readonly TreeNode[];
  /** Trailing detail such as a change count or status letter. */
  meta?: string;
  /** Replaces the default folder or file icon. */
  icon?: ReactNode;
};

export type FileTreeProps = Omit<ComponentPropsWithRef<"ul">, "children" | "onSelect"> & {
  nodes: readonly TreeNode[];
  selectedId?: string;
  /** Called with a file's id; without it, files can be focused and read but not picked. */
  onSelect?: (id: string) => void;
  /** Folder ids that start expanded. */
  defaultExpanded?: readonly string[];
};

/** Keys typed within this time extend one typeahead search. */
const TYPEAHEAD_RESET_MS = 500;

type ItemContext = {
  baseId: string;
  rows: ReadonlyMap<string, FlatTreeNode>;
  tabStopId: string | null;
  selectedId?: string;
  selectable: boolean;
  onRowClick: (node: TreeNode) => void;
};

function RowContent({ node }: { node: TreeNode }) {
  const isFolder = node.kind === "folder";
  return (
    <>
      <span className={styles.chevron} aria-hidden="true">
        {isFolder ? <ChevronRightIcon /> : null}
      </span>
      <span className={styles.icon} aria-hidden="true">
        {node.icon ?? (isFolder ? <FolderIcon /> : <FileIcon />)}
      </span>
      <span className={styles.name}>{node.name}</span>
      {node.meta ? <span className={styles.meta}>{node.meta}</span> : null}
    </>
  );
}

function treeItemId(baseId: string, id: string) {
  return `${baseId}-${encodeURIComponent(id).replace(/%/g, ".")}`;
}

function TreeItem({ node, context }: { node: TreeNode; context: ItemContext }) {
  const row = context.rows.get(node.id);
  if (!row) return null;
  const domId = treeItemId(context.baseId, node.id);
  const isFolder = node.kind === "folder";
  const isSelected = !isFolder && node.id === context.selectedId;
  const showChildren = isFolder && row.expanded && row.hasChildren;
  const indent = { "--file-tree-depth": row.level - 1 } as CSSProperties;
  return (
    <li
      id={domId}
      role="treeitem"
      // Named by its own row only; the default name would also take in every nested child.
      aria-labelledby={`${domId}-row`}
      aria-level={row.level}
      aria-setsize={row.setSize}
      aria-posinset={row.position}
      aria-expanded={isFolder && row.hasChildren ? row.expanded : undefined}
      aria-selected={!isFolder && context.selectable ? isSelected : undefined}
      tabIndex={node.id === context.tabStopId ? 0 : -1}
      className={styles.item}
      data-tree-id={node.id}
    >
      <div
        id={`${domId}-row`}
        className={cx(
          styles.row,
          !isFolder && !context.selectable && styles.static,
          isSelected && styles.selected,
          showChildren && styles.open,
        )}
        style={indent}
        onClick={() => context.onRowClick(node)}
      >
        <RowContent node={node} />
      </div>
      {showChildren ? (
        <ul role="group" className={styles.group}>
          {node.children?.map((child) => (
            <TreeItem key={child.id} node={child} context={context} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}

/**
 * Folder and file tree (the WAI-ARIA tree pattern). It is one Tab stop; ArrowUp/ArrowDown move
 * between visible rows, ArrowRight opens a folder or enters it, ArrowLeft closes it or goes to the
 * parent, Home/End jump to the ends, typing jumps to a name, `*` opens the sibling folders, and
 * Enter or Space toggles a folder or picks a file. Clicks do the same as Enter.
 */
export function FileTree({
  nodes,
  selectedId,
  onSelect,
  defaultExpanded,
  className,
  onKeyDown,
  onFocus,
  ...props
}: FileTreeProps) {
  const baseId = useId();
  const treeRef = useRef<HTMLUListElement>(null);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set(defaultExpanded));
  const [focusId, setFocusId] = useState<string | null>(null);
  const typeahead = useRef({ text: "", at: 0 });
  const pendingFocus = useRef<string | null>(null);

  const rows = useMemo(() => flattenVisibleTree(nodes, expanded), [nodes, expanded]);
  const rowMap = useMemo(() => new Map(rows.map((row) => [row.id, row])), [rows]);

  // Parent of every node, visible or not, so a hidden focus target falls back to its open ancestor.
  const parents = useMemo(() => {
    const map = new Map<string, string | null>();
    const walk = (list: readonly TreeNode[], parent: string | null) => {
      for (const node of list) {
        map.set(node.id, parent);
        if (node.children) walk(node.children, node.id);
      }
    };
    walk(nodes, null);
    return map;
  }, [nodes]);

  const visibleAncestor = (id: string | null | undefined): string | null => {
    let current = id ?? null;
    while (current !== null && !rowMap.has(current)) current = parents.get(current) ?? null;
    return current;
  };
  // The one row in the Tab order: the last focused row, else the picked file, else the first row.
  const tabStopId = visibleAncestor(focusId) ?? visibleAncestor(selectedId) ?? rows[0]?.id ?? null;

  // Moves DOM focus once the row exists (an expand renders its children first).
  useEffect(() => {
    const id = pendingFocus.current;
    if (id === null) return;
    pendingFocus.current = null;
    const element = treeRef.current?.querySelector<HTMLElement>(`#${CSS.escape(treeItemId(baseId, id))}`);
    element?.focus();
  });

  const moveFocus = (id: string) => {
    pendingFocus.current = id;
    setFocusId(id);
  };

  const setOpen = (ids: readonly string[], open: boolean) =>
    setExpanded((current) => {
      const next = new Set(current);
      for (const id of ids) {
        if (open) next.add(id);
        else next.delete(id);
      }
      return next;
    });

  const activate = (node: { id: string; kind: "folder" | "file" }) => {
    if (node.kind === "folder") setOpen([node.id], !expanded.has(node.id));
    else onSelect?.(node.id);
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLUListElement>) => {
    onKeyDown?.(event);
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.nativeEvent.isComposing) return;
    const item = (event.target as HTMLElement).closest<HTMLElement>('[role="treeitem"]');
    // Only keys on a row itself, not on something focusable a row might contain.
    if (!item || item !== event.target) return;
    const current = item.dataset.treeId ?? null;
    let search = event.key;
    if (isTypeaheadKey(event.key)) {
      const now = event.timeStamp;
      const buffer = now - typeahead.current.at < TYPEAHEAD_RESET_MS ? typeahead.current.text + event.key : event.key;
      typeahead.current = { text: buffer, at: now };
      search = buffer;
    }
    const action = nextTreeFocus(rows, current, event.key, search);
    if (action.type === "none") {
      if (isTypeaheadKey(event.key)) event.preventDefault();
      return;
    }
    event.preventDefault();
    if (action.type === "focus") moveFocus(action.id);
    else if (action.type === "expand") setOpen([action.id], true);
    else if (action.type === "collapse") setOpen([action.id], false);
    else if (action.type === "expandSiblings") setOpen(action.ids, true);
    else if (action.type === "activate") {
      const row = rowMap.get(action.id);
      if (row) activate(row);
    }
  };

  const context: ItemContext = {
    baseId,
    rows: rowMap,
    tabStopId,
    selectedId,
    selectable: onSelect !== undefined,
    onRowClick: (node) => {
      moveFocus(node.id);
      activate(node);
    },
  };

  return (
    <ul
      ref={treeRef}
      role="tree"
      aria-label="Files"
      className={cx(styles.tree, className)}
      onKeyDown={handleKeyDown}
      onFocus={(event) => {
        onFocus?.(event);
        const id = (event.target as HTMLElement).dataset?.treeId;
        if (id !== undefined && id !== focusId) setFocusId(id);
      }}
      {...props}
    >
      {nodes.map((node) => (
        <TreeItem key={node.id} node={node} context={context} />
      ))}
    </ul>
  );
}
