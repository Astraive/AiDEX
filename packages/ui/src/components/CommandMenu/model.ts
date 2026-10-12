// Pure helpers for `CommandMenu`: no DOM or React, so they run during prerendering and in unit tests.

import type { CommandMenuItem, CommandMenuSection } from "./CommandMenu";

/**
 * DOM id of a row's option element, for the field's `aria-activedescendant`. Characters outside
 * `[A-Za-z0-9_-]` are hex-escaped, so any item id (including lone surrogates) maps to a unique id.
 */
export function commandMenuOptionId(listId: string, itemId: string): string {
  let safe = "";
  for (const char of Array.from(itemId)) {
    safe += /^[A-Za-z0-9_-]$/.test(char) ? char : `.${(char.codePointAt(0) ?? 0).toString(16)}.`;
  }
  return `${listId}-option-${safe}`;
}

/** Rows of every section in display order. */
export function flattenCommandMenu(sections: readonly CommandMenuSection[]): CommandMenuItem[] {
  const items: CommandMenuItem[] = [];
  for (const section of sections) items.push(...section.items);
  return items;
}

/** Splits `text` around the first case-insensitive occurrence of `query` (empty match when none). */
export function highlightMatch(text: string, query: string): [string, string, string] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [text, "", ""];
  const index = text.toLowerCase().indexOf(needle);
  // Lower-casing can change the length of some characters; only highlight when offsets line up.
  if (index === -1 || text.toLowerCase().length !== text.length) return [text, "", ""];
  return [text.slice(0, index), text.slice(index, index + needle.length), text.slice(index + needle.length)];
}

/**
 * The row to highlight: the first row when `restart` is set (the menu just opened or its query
 * changed), otherwise the current row while it is still listed, else the first row. Disabled rows
 * count like any other, so a best match that can't run is still the highlighted one and Enter cannot
 * fall through to a lower row. Null when there are no rows.
 */
export function resolveCommandMenuActive(
  items: readonly CommandMenuItem[],
  activeId: string | null,
  restart: boolean,
): string | null {
  if (!restart && activeId !== null && items.some((item) => item.id === activeId)) return activeId;
  return items[0]?.id ?? null;
}

export type CommandMenuKeyAction =
  /** Not a menu key here: the field handles it as usual. */
  | { type: "pass" }
  /** Highlight the row with this id. */
  | { type: "move"; id: string }
  /** Run the highlighted row. */
  | { type: "select"; item: CommandMenuItem }
  /** The highlighted row can't run: swallow the key and keep the menu open. */
  | { type: "block" };

/**
 * What a key pressed in the field does to an open menu (the caller skips Alt, Ctrl, Meta, and IME
 * presses). ArrowUp/ArrowDown move through every row, disabled ones included, wrapping at the ends.
 * Enter or Tab runs the highlighted row, or does nothing when that row is disabled; with no highlighted
 * row, and with Shift held, they reach the field.
 */
export function commandMenuKeyAction(
  items: readonly CommandMenuItem[],
  activeId: string | null,
  key: string,
  shiftKey = false,
): CommandMenuKeyAction {
  const index = activeId === null ? -1 : items.findIndex((item) => item.id === activeId);
  switch (key) {
    case "ArrowDown":
    case "ArrowUp": {
      if (items.length === 0) return { type: "pass" };
      const down = key === "ArrowDown";
      const next = index === -1 ? (down ? 0 : items.length - 1) : (index + (down ? 1 : -1) + items.length) % items.length;
      return { type: "move", id: items[next].id };
    }
    case "Enter":
    case "Tab": {
      if (shiftKey || index === -1) return { type: "pass" };
      const item = items[index];
      return item.disabled ? { type: "block" } : { type: "select", item };
    }
    default:
      return { type: "pass" };
  }
}

export type CommandMenuFieldState = {
  /** The menu's `id`. */
  listId: string;
  open: boolean;
  /** The menu's `activeId`. */
  activeId: string | null;
};

/** ARIA attributes that tie a text field to an open `CommandMenu`; spread them onto the field. */
export function commandMenuFieldProps({ listId, open, activeId }: CommandMenuFieldState) {
  return {
    "aria-autocomplete": "list" as const,
    "aria-haspopup": "listbox" as const,
    "aria-controls": open ? listId : undefined,
    "aria-activedescendant": open && activeId !== null ? commandMenuOptionId(listId, activeId) : undefined,
  };
}
