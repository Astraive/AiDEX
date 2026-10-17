import { useCallback, useEffect, useRef, useState } from "react";
import { createRemovalFocus, type FocusPlace } from "./removalFocus";

const CONTROL =
  'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

/** The first control in a list row (a `ListRow`'s `<li>`, a `DataTable`'s `<tr>`), if it has one. */
export function firstControl(row: Element | null | undefined): HTMLElement | undefined {
  return row?.querySelector<HTMLElement>(CONTROL) ?? undefined;
}

function focusPlace(): FocusPlace {
  const active = document.activeElement;
  if (!active || active === document.body || !active.isConnected) return "dropped";
  return active.closest('[role="dialog"], [role="alertdialog"]') ? "dialog" : "elsewhere";
}

/**
 * Keeps keyboard focus after a confirmed delete instead of letting it drop to the page. `ids` are the
 * rows shown, in order; `rowTarget(index)` finds a row's control (see `firstControl`) and
 * `fallback()` what takes focus once no row is left (the list's heading or empty state). Call
 * `removed(id)` once the delete succeeded, before the list is re-read, and pass `returnFocus` to the
 * confirmation's `Dialog`. Once the row is gone, focus moves to the row that took its place (or the
 * nearest row with a control), else to the fallback.
 */
export function useRemovalFocus(
  ids: readonly string[],
  rowTarget: (index: number) => HTMLElement | null | undefined,
  fallback: () => HTMLElement | null | undefined,
) {
  const [tracker] = useState(createRemovalFocus);
  const idsRef = useRef(ids);
  idsRef.current = ids;
  const rowTargetRef = useRef(rowTarget);
  rowTargetRef.current = rowTarget;
  const fallbackRef = useRef(fallback);
  fallbackRef.current = fallback;
  const key = ids.join("\n");

  const target = useCallback((order: readonly number[]) => {
    for (const index of order) {
      const row = rowTargetRef.current(index);
      if (row) return row;
    }
    return fallbackRef.current() ?? undefined;
  }, []);

  useEffect(() => {
    const order = tracker.listChanged(idsRef.current, focusPlace());
    if (order) target(order)?.focus();
  }, [key, tracker, target]);

  const removed = useCallback((id: string) => tracker.removed(idsRef.current, id), [tracker]);
  const returnFocus = useCallback(() => {
    const order = tracker.dialogClosed(idsRef.current);
    return order ? target(order) : undefined;
  }, [tracker, target]);
  return { removed, returnFocus };
}
