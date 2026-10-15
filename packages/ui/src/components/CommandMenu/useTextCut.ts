"use client";

import { useLayoutEffect, useState } from "react";

/**
 * Whether an element with one of `classNames` inside the element with id `rootId` cuts its text off
 * (an ellipsis). Rechecked after every render, which list rows get on each highlight move, so a
 * row's tooltip can show only while the row hides part of its text. Those elements must be
 * block-level or flex items; a pixel of slack absorbs fractional widths at high zoom.
 */
export function useTextCut(rootId: string, classNames: readonly (string | undefined)[]): boolean {
  const [cut, setCut] = useState(false);
  useLayoutEffect(() => {
    const root = document.getElementById(rootId);
    const names = classNames.filter((name): name is string => Boolean(name));
    if (!root || names.length === 0) return;
    const selector = names.map((name) => `.${CSS.escape(name)}`).join(", ");
    let next = false;
    for (const element of Array.from(root.querySelectorAll(selector))) {
      if (element.scrollWidth > element.clientWidth + 1) {
        next = true;
        break;
      }
    }
    setCut((current) => (current === next ? current : next));
  });
  return cut;
}
