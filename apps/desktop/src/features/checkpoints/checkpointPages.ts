// Paging rules of the checkpoint list, as pure functions so they can be tested without a daemon.
// `checkpoint.list` returns checkpoints oldest first; the page opens on the newest ones.

/** Checkpoints per `checkpoint.list` page. */
export const CHECKPOINT_PAGE = 20;

/** A `checkpoint.list` request window. */
export type CheckpointWindow = { offset: number; limit: number };

/**
 * The page chosen by the user: a fixed window, or `null` for the newest page, which moves on as
 * checkpoints are added.
 */
export type PageWindow = CheckpointWindow | null;

/** Offset of the newest page of `total` checkpoints. */
export function newestPageOffset(total: number): number {
  return Math.max(0, total - CHECKPOINT_PAGE);
}

/** The window `page` asks for while the conversation holds `total` checkpoints. */
export function shownWindow(page: PageWindow, total: number): CheckpointWindow {
  return page ?? { offset: newestPageOffset(total), limit: CHECKPOINT_PAGE };
}

/** The page before `shown`, ending where `shown` starts. Only valid when `shown.offset > 0`. */
export function olderWindow(shown: CheckpointWindow): CheckpointWindow {
  const offset = Math.max(0, shown.offset - CHECKPOINT_PAGE);
  return { offset, limit: shown.offset - offset };
}

/**
 * The page after `shown`, from the `checkpoint.list` answer for `shown` (`total`, `nextOffset`);
 * `undefined` when that answer had nothing newer. It starts at `nextOffset`, so pages never
 * overlap and every checkpoint stays reachable even when some were added after `shown` was
 * chosen. It returns to following the newest page only when it starts exactly where that page does.
 */
export function newerWindow(total: number, nextOffset: number | null): PageWindow | undefined {
  if (nextOffset == null) return undefined;
  return nextOffset === newestPageOffset(total) ? null : { offset: nextOffset, limit: CHECKPOINT_PAGE };
}
