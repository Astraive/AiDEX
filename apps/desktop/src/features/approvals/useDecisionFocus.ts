import { useCallback, useEffect, useRef } from "react";

/** Set on an `ApprovalRequestCard`'s decision buttons, with the request's id as its value. */
export const DECISION_ATTRIBUTE = "data-approval-decision";

/** The first enabled decision button of a pending request's card, if it is shown. */
function firstAction(id: string): HTMLElement | null {
  const group = document.querySelector(`[${DECISION_ATTRIBUTE}="${CSS.escape(id)}"]`);
  return group?.querySelector<HTMLElement>("button:not(:disabled)") ?? null;
}

/**
 * Keeps keyboard focus after an approval decision. Deciding takes the request's buttons away (its
 * card leaves a pending list, or stays without them), which would drop focus to the page. Call
 * `begin(id)` as a decision is sent and `cancel()` if it fails. Once the request is no longer
 * pending, and only if focus was dropped meanwhile, focus moves to the first action of the request
 * that took its place (or the last one left); with none left, `fallback()` places focus.
 */
export function useDecisionFocus(pendingIds: readonly string[], fallback: () => void) {
  const decided = useRef<{ id: string; index: number } | null>(null);
  const pending = useRef(pendingIds);
  pending.current = pendingIds;
  const fallbackRef = useRef(fallback);
  fallbackRef.current = fallback;
  const key = pendingIds.join("\n");

  useEffect(() => {
    const target = decided.current;
    if (!target || pending.current.includes(target.id)) return;
    decided.current = null;
    const active = document.activeElement;
    if (active && active !== document.body) return;
    const ids = pending.current;
    const next = ids.length ? firstAction(ids[Math.min(target.index, ids.length - 1)]) : null;
    if (next) next.focus();
    else fallbackRef.current();
  }, [key]);

  const begin = useCallback((id: string) => {
    decided.current = { id, index: Math.max(0, pending.current.indexOf(id)) };
  }, []);
  const cancel = useCallback(() => {
    decided.current = null;
  }, []);
  return { begin, cancel };
}
