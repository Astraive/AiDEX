"use client";

import { useEffect, type RefObject } from "react";

export type OverflowAxis = "x" | "y";

export type OverflowEdges = { start: boolean; end: boolean };

/** Attributes `useOverflowEdges` sets on the scroll container; style fades or shadows from them. */
export const OVERFLOW_START_ATTRIBUTE = "data-overflow-start";
export const OVERFLOW_END_ATTRIBUTE = "data-overflow-end";

/**
 * Which edges of a scroll container hide content. `offset` is the scroll position along the axis
 * (negative in right-to-left layouts, so only its magnitude counts); a pixel of slack absorbs
 * fractional sizes at high zoom.
 */
export function overflowEdges(offset: number, clientSize: number, scrollSize: number): OverflowEdges {
  const position = Math.abs(offset);
  return { start: position > 1, end: position + clientSize < scrollSize - 1 };
}

function toggle(element: Element, name: string, on: boolean) {
  if (on) element.setAttribute(name, "");
  else element.removeAttribute(name);
}

/**
 * Marks a scroll container with `data-overflow-start` / `data-overflow-end` while content is cut off
 * at that edge, so its CSS can show a fade there. It updates on scroll (passive), when the
 * container or its children resize, and when children are added or removed.
 */
export function useOverflowEdges(ref: RefObject<HTMLElement | null>, axis: OverflowAxis = "x"): void {
  useEffect(() => {
    const element = ref.current;
    if (!element) return;

    let frame = 0;
    const update = () => {
      frame = 0;
      const edges =
        axis === "x"
          ? overflowEdges(element.scrollLeft, element.clientWidth, element.scrollWidth)
          : overflowEdges(element.scrollTop, element.clientHeight, element.scrollHeight);
      toggle(element, OVERFLOW_START_ATTRIBUTE, edges.start);
      toggle(element, OVERFLOW_END_ATTRIBUTE, edges.end);
    };
    const schedule = () => {
      if (frame === 0) frame = requestAnimationFrame(update);
    };

    update();
    element.addEventListener("scroll", schedule, { passive: true });

    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    const observeChildren = () => {
      if (!resizeObserver) return;
      resizeObserver.disconnect();
      resizeObserver.observe(element);
      for (const child of Array.from(element.children)) resizeObserver.observe(child);
    };
    observeChildren();
    const mutationObserver =
      typeof MutationObserver === "undefined"
        ? null
        : new MutationObserver(() => {
            observeChildren();
            schedule();
          });
    mutationObserver?.observe(element, { childList: true });

    return () => {
      if (frame !== 0) cancelAnimationFrame(frame);
      element.removeEventListener("scroll", schedule);
      resizeObserver?.disconnect();
      mutationObserver?.disconnect();
      element.removeAttribute(OVERFLOW_START_ATTRIBUTE);
      element.removeAttribute(OVERFLOW_END_ATTRIBUTE);
    };
  }, [ref, axis]);
}
