/** Where keyboard focus is as a list changes: dropped to the page, in a (closing) dialog, or elsewhere. */
export type FocusPlace = "dropped" | "dialog" | "elsewhere";

/**
 * The rows to try focusing, in order, after the row at `index` left a list that now has `count`
 * rows: the row that took its place (or the new last row), then the rows after it, then the rows
 * before it, nearest first. Empty when no row is left.
 */
export function removalFocusOrder(count: number, index: number): number[] {
  if (count <= 0) return [];
  const start = Math.min(Math.max(0, index), count - 1);
  const order: number[] = [];
  for (let next = start; next < count; next += 1) order.push(next);
  for (let previous = start - 1; previous >= 0; previous -= 1) order.push(previous);
  return order;
}

/**
 * Decides where focus goes after a row is deleted. A confirmed delete closes its dialog and re-reads
 * the list, in either order: when the list loses the row first, the dialog's close places focus
 * (`dialogClosed`); when the dialog closes first, focus returns to the row's own control, drops to
 * the page as the row goes, and `listChanged` places it. Each answer is the rows to try, in order
 * (none left: the caller's fallback), or `undefined` to leave focus alone.
 */
export function createRemovalFocus() {
  let pending: { id: string; index: number } | undefined;
  const settle = (ids: readonly string[]) => {
    if (!pending || ids.includes(pending.id)) return undefined;
    const { index } = pending;
    pending = undefined;
    return removalFocusOrder(ids.length, index);
  };
  return {
    /** The row `id` was deleted; `ids` (the rows shown, in order) still lists it. */
    removed(ids: readonly string[], id: string) {
      pending = { id, index: Math.max(0, ids.indexOf(id)) };
    },
    /**
     * The list now shows `ids`. Once the deleted row is gone: with focus dropped, place it; inside the
     * closing dialog, wait for `dialogClosed`; anywhere else the user moved on, so forget the delete.
     */
    listChanged(ids: readonly string[], focus: FocusPlace): number[] | undefined {
      if (!pending || ids.includes(pending.id) || focus === "dialog") return undefined;
      const order = settle(ids);
      return focus === "dropped" ? order : undefined;
    },
    /** The confirmation closed and focus is about to return; a delete whose row is gone places it. */
    dialogClosed(ids: readonly string[]): number[] | undefined {
      return settle(ids);
    },
  };
}
