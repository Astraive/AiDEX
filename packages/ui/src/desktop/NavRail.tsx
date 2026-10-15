"use client";

import { useEffect, useRef, type ReactNode } from "react";
import { IconButton } from "../primitives/IconButton";
import { TooltipProvider } from "../primitives/Tooltip";
import { useOverflowEdges } from "../utils/useOverflowEdges";
import styles from "./NavRail.module.css";

export type NavRailItem<Id extends string> = { id: Id; label: string; icon: ReactNode };

export type NavRailProps<Id extends string> = {
  /** Accessible name for the navigation landmark. */
  label: string;
  items: readonly NavRailItem<Id>[];
  activeId: Id;
  onSelect: (id: Id) => void;
};

/**
 * Vertical icon navigation; each destination is labelled for assistive technology and shows its
 * label as a tooltip on the right. In a short window the rail scrolls, with fades at a hidden edge.
 */
export function NavRail<Id extends string>({ label, items, activeId, onSelect }: NavRailProps<Id>) {
  const scrollerRef = useRef<HTMLDivElement>(null);
  useOverflowEdges(scrollerRef, "y");

  // The current destination stays in view when the rail scrolls. Only the rail scrolls: scrollIntoView
  // could also move the clipped shell around it.
  useEffect(() => {
    const scroller = scrollerRef.current;
    const button = scroller?.querySelector<HTMLElement>('[aria-current="page"]');
    if (!scroller || !button) return;
    const view = scroller.getBoundingClientRect();
    const target = button.getBoundingClientRect();
    const inset = parseFloat(getComputedStyle(scroller).paddingBlockEnd) || 0;
    if (target.top < view.top + inset) scroller.scrollTop -= view.top + inset - target.top;
    else if (target.bottom > view.bottom - inset) scroller.scrollTop += target.bottom - (view.bottom - inset);
  }, [activeId]);
  return (
    <nav aria-label={label} className={styles.rail}>
      <div ref={scrollerRef} className={styles.scroller}>
        {/* Rail labels open sooner than other tooltips; moving along the rail shows the next at once. */}
        <TooltipProvider delay={150}>
          <ul className={styles.list}>
          {items.map((item) => {
            const active = item.id === activeId;
            return (
              <li key={item.id}>
                <IconButton
                  label={item.label}
                  icon={item.icon}
                  size="lg"
                  selected={active}
                  tooltipSide="right"
                  tooltipSideOffset={8}
                  aria-current={active ? "page" : undefined}
                  onClick={() => onSelect(item.id)}
                />
              </li>
            );
          })}
          </ul>
        </TooltipProvider>
      </div>
    </nav>
  );
}
