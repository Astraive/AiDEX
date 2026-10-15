"use client";

import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { cx } from "../utils/cx";
import { useOverflowEdges } from "../utils/useOverflowEdges";
import styles from "./Tabs.module.css";

export type TabItem<Value extends string> = {
  value: Value;
  label: string;
  icon?: ReactNode;
  count?: number;
  /** Shown but not selectable; arrow keys skip it. */
  disabled?: boolean;
};

export type TabsProps<Value extends string> = {
  /** Base id linking each tab to its `TabPanel`. */
  id: string;
  /** Accessible name for the tab list. */
  label: string;
  items: readonly TabItem<Value>[];
  value: Value;
  onValueChange: (value: Value) => void;
  /** `pill` for in-page view switches; `underline` for top-level page sections. */
  variant?: "pill" | "underline";
  className?: string;
};

const STEP_BY_KEY: Partial<Record<string, number>> = { ArrowRight: 1, ArrowLeft: -1 };

/** Room kept between a revealed tab and the list edge, so it clears the overflow fade (24px in the CSS). */
const REVEAL_MARGIN = 24;

export const tabId = (tabsId: string, value: string) => `${tabsId}-tab-${value}`;
export const tabPanelId = (tabsId: string, value: string) => `${tabsId}-panel-${value}`;

/** Index of the next enabled item from `start` in `direction` (wrapping), or -1 when all are disabled. */
function enabledIndex(items: readonly { disabled?: boolean }[], start: number, direction: 1 | -1): number {
  for (let offset = 0; offset < items.length; offset += 1) {
    const index = (((start + direction * offset) % items.length) + items.length) % items.length;
    if (!items[index].disabled) return index;
  }
  return -1;
}

/**
 * Scrolls the list sideways just enough to show the tab. Unlike `scrollIntoView`, it never scrolls
 * an ancestor, so selecting a tab cannot move the page.
 */
function revealTab(list: HTMLElement, tab: HTMLElement) {
  if (list.scrollWidth <= list.clientWidth) return;
  const listBox = list.getBoundingClientRect();
  const tabBox = tab.getBoundingClientRect();
  const margin = Math.min(REVEAL_MARGIN, Math.max(0, (listBox.width - tabBox.width) / 2));
  if (tabBox.left < listBox.left + margin) list.scrollLeft -= listBox.left + margin - tabBox.left;
  else if (tabBox.right > listBox.right - margin) list.scrollLeft += tabBox.right - (listBox.right - margin);
}

/**
 * Tab list with roving focus (arrow keys, Home, End) and automatic activation. A list too wide for
 * its container scrolls sideways with fades at the cut-off edges, and keeps the selected tab in view.
 */
export function Tabs<Value extends string>({
  id,
  label,
  items,
  value,
  onValueChange,
  variant = "pill",
  className,
}: TabsProps<Value>) {
  const listRef = useRef<HTMLDivElement | null>(null);
  const tabRefs = useRef<Array<HTMLButtonElement | null>>([]);
  useOverflowEdges(listRef, "x");

  const selectedIndex = items.findIndex((item) => item.value === value);
  // With no selected tab, the first enabled one takes Tab focus so the list stays reachable.
  const tabStopIndex = selectedIndex === -1 ? enabledIndex(items, 0, 1) : selectedIndex;

  useEffect(() => {
    const list = listRef.current;
    const tab = selectedIndex === -1 ? null : tabRefs.current[selectedIndex];
    if (list && tab) revealTab(list, tab);
  }, [selectedIndex]);

  const selectAt = (index: number, direction: 1 | -1) => {
    const target = enabledIndex(items, index, direction);
    if (target === -1) return;
    onValueChange(items[target].value);
    tabRefs.current[target]?.focus();
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const step = STEP_BY_KEY[event.key];
    if (step !== undefined) selectAt(index + step, step > 0 ? 1 : -1);
    else if (event.key === "Home") selectAt(0, 1);
    else if (event.key === "End") selectAt(items.length - 1, -1);
    else return;
    event.preventDefault();
  };

  return (
    <div
      ref={listRef}
      role="tablist"
      aria-label={label}
      aria-orientation="horizontal"
      className={cx(styles.list, styles[variant], className)}
    >
      {items.map((item, index) => {
        const selected = item.value === value;
        return (
          <button
            key={item.value}
            ref={(node) => {
              tabRefs.current[index] = node;
            }}
            type="button"
            role="tab"
            id={tabId(id, item.value)}
            aria-selected={selected}
            aria-controls={selected ? tabPanelId(id, item.value) : undefined}
            aria-disabled={item.disabled || undefined}
            tabIndex={index === tabStopIndex ? 0 : -1}
            className={cx(styles.tab, selected && styles.selected)}
            onClick={() => {
              if (!item.disabled) onValueChange(item.value);
            }}
            onKeyDown={(event) => handleKeyDown(event, index)}
          >
            {item.icon ? <span className={styles.icon}>{item.icon}</span> : null}
            {item.label}
            {item.count !== undefined ? <span className={styles.count}>{item.count}</span> : null}
          </button>
        );
      })}
    </div>
  );
}

export type TabPanelProps = {
  /** The `id` passed to the matching `Tabs`. */
  tabsId: string;
  /** Value of the tab this panel belongs to; render only the selected panel. */
  value: string;
  children: ReactNode;
  className?: string;
};

export function TabPanel({ tabsId, value, children, className }: TabPanelProps) {
  return (
    <div
      role="tabpanel"
      id={tabPanelId(tabsId, value)}
      aria-labelledby={tabId(tabsId, value)}
      tabIndex={0}
      className={cx(styles.panel, className)}
    >
      {children}
    </div>
  );
}
