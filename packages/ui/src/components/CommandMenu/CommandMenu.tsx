"use client";

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode, type RefObject } from "react";
import { LoaderIcon } from "../../icons/interface";
import { Badge, type BadgeTone } from "../../primitives/Badge";
import { Popover, type PopoverPlacement } from "../../primitives/Popover";
import { Tooltip } from "../../primitives/Tooltip";
import { cx } from "../../utils/cx";
import {
  commandMenuKeyAction,
  commandMenuOptionId,
  flattenCommandMenu,
  highlightMatch,
  resolveCommandMenuActive,
} from "./model";
import a11y from "../content/a11y.module.css";
import styles from "./CommandMenu.module.css";
import { useTextCut } from "./useTextCut";

export type CommandMenuBadge = { label: string; tone?: BadgeTone; icon?: ReactNode };

export type CommandMenuItem = {
  /** Stable id, unique across all sections. */
  id: string;
  title: string;
  /** Muted text before the title, e.g. `/skill:`. */
  prefix?: string;
  /** One line; while it is cut off, the row's tooltip (beside the row, on hover) shows it in full. */
  description?: string;
  /** Decorative leading icon. */
  icon?: ReactNode;
  /** Source or category pill, e.g. "Drive skill". */
  badge?: string | CommandMenuBadge;
  /**
   * Can't run here: dimmed and announced as disabled (`aria-disabled`). It can still be highlighted,
   * but Enter, Tab, and clicks do nothing on it.
   */
  disabled?: boolean;
  /**
   * Text shown beside the row on hover, always. Without it, the row shows its title and description
   * there only while one of them is cut off.
   */
  tooltip?: string;
};

export type CommandMenuSection = {
  id: string;
  /** Visible group heading; omit for an unlabelled group. */
  label?: string;
  items: readonly CommandMenuItem[];
};

export type CommandMenuProps = {
  open: boolean;
  /** Element the menu is positioned against and sized to, typically the composer. */
  anchorRef: RefObject<HTMLElement | null>;
  /**
   * The text field that keeps focus while the menu is open. Arrow, Enter, and Tab presses in it drive
   * the menu, and the menu closes when focus leaves it. Defaults to any element inside `anchorRef`.
   */
  inputRef?: RefObject<HTMLElement | null>;
  /** The text typed after the trigger; its first match in each title is highlighted. */
  query?: string;
  sections: readonly CommandMenuSection[];
  /** Id of the highlighted row (`aria-activedescendant` target). */
  activeId: string | null;
  onActiveChange: (id: string | null) => void;
  onSelect: (item: CommandMenuItem) => void;
  /** Escape, a press outside, or focus leaving the field. */
  onClose: () => void;
  /** Listbox id; pass the same id to the field (see `commandMenuFieldProps`). */
  id: string;
  /** Accessible name of the list, e.g. "Skills" or "Commands". */
  label: string;
  /** Shows `loadingText` while there are no rows yet. */
  loading?: boolean;
  loadingText?: string;
  emptyText?: string;
  /** Content under the list, e.g. a keyboard hint. Keep it non-interactive or pointer-only. */
  footer?: ReactNode;
  /** Default `top-start`: above a composer, flipping below when there is no room. */
  placement?: PopoverPlacement;
  /** Size the menu to the anchor's width (default true). */
  matchAnchorWidth?: boolean;
  /** Portal target; pass the open `<dialog>` when the field is inside a modal. */
  container?: HTMLElement | null;
  className?: string;
};

// Rows keep focus in the text field (aria-activedescendant), so pointer presses must not move it.
const keepFocus = (event: MouseEvent) => event.preventDefault();

/**
 * The input that last moved the highlight. The active row shows a focus ring only after keyboard
 * use; under the pointer it is just tinted, as hover is.
 */
export type CommandMenuModality = "keyboard" | "pointer";

/**
 * Anchored suggestion list for a text field, such as the composer's `$` skills menu and `/` command
 * menu. Focus stays in the field: ArrowUp/ArrowDown move the active row, Enter or Tab selects it (or
 * does nothing on a disabled row), and Escape closes. Fully controlled; the app filters and ranks
 * `sections` from `query`, best match first.
 */
export function CommandMenu({
  open,
  anchorRef,
  inputRef,
  query = "",
  sections,
  activeId,
  onActiveChange,
  onSelect,
  onClose,
  id,
  label,
  loading = false,
  loadingText = "Searching...",
  emptyText = "No matching command.",
  footer,
  placement = "top-start",
  matchAnchorWidth = true,
  container,
  className,
}: CommandMenuProps) {
  const items = useMemo(() => flattenCommandMenu(sections), [sections]);
  // Typing opens the menu or changes the query, so either starts keyboard modality (see below).
  const [modality, setModality] = useState<CommandMenuModality>("keyboard");

  // Latest values for the document listeners, which stay bound for the whole time the menu is open.
  const latest = useRef({ items, activeId, onActiveChange, onSelect, onClose });
  useLayoutEffect(() => {
    latest.current = { items, activeId, onActiveChange, onSelect, onClose };
  });

  // Opening, or a new query, highlights the first row even when it is disabled, so Enter never runs a
  // lower match in place of the best one. Afterwards the highlight stays on its row while the row is
  // listed and moves to the first row otherwise. A layout effect, so the correction lands before the
  // next key press can act on a stale row.
  const previousRef = useRef({ open: false, query });
  useLayoutEffect(() => {
    const previous = previousRef.current;
    previousRef.current = { open, query };
    if (!open) return;
    const restart = !previous.open || previous.query !== query;
    if (restart) setModality("keyboard");
    const next = resolveCommandMenuActive(items, activeId, restart);
    if (next !== activeId) onActiveChange(next);
  }, [open, query, items, activeId, onActiveChange]);

  useEffect(() => {
    if (!open) return;
    const field = () => inputRef?.current ?? anchorRef.current;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
      const target = event.target;
      const input = field();
      if (!input || !(target instanceof Node) || !input.contains(target)) return;
      const { items: rows, activeId: current } = latest.current;
      const action = commandMenuKeyAction(rows, current, event.key, event.shiftKey);
      if (action.type === "pass") return;
      // Capture phase: the field's own handlers (caret moves, send on Enter, focus moves on Tab) never
      // see these keys, so Enter or Tab on a disabled row does nothing and the menu stays open.
      event.preventDefault();
      event.stopPropagation();
      if (action.type === "move") {
        setModality("keyboard");
        latest.current.onActiveChange(action.id);
      } else if (action.type === "select") latest.current.onSelect(action.item);
    };
    // Close when focus leaves the field for somewhere else in the page (switching windows keeps it).
    let frame = 0;
    const onFocusOut = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const input = field();
        const active = document.activeElement;
        if (!input || !active || active === document.body) return;
        if (!input.contains(active)) latest.current.onClose();
      });
    };
    const input = field();
    document.addEventListener("keydown", onKeyDown, true);
    input?.addEventListener("focusout", onFocusOut);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("keydown", onKeyDown, true);
      input?.removeEventListener("focusout", onFocusOut);
    };
  }, [open, anchorRef, inputRef]);

  // Scroll the active row into view inside the list only, never the page.
  const listRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!open || !list || activeId === null) return;
    const option = document.getElementById(commandMenuOptionId(id, activeId));
    if (!option || !list.contains(option)) return;
    // The first row of a section brings its heading along.
    const group = option.parentElement;
    const top = group && group !== list && option === group.querySelector('[role="option"]') ? group.offsetTop : option.offsetTop;
    const bottom = option.offsetTop + option.offsetHeight;
    // Keep the list's own padding visible around the first and last rows.
    const padding = parseFloat(getComputedStyle(list).paddingTop) || 0;
    if (top - padding < list.scrollTop) list.scrollTop = Math.max(0, top - padding);
    else if (bottom + padding > list.scrollTop + list.clientHeight) list.scrollTop = bottom + padding - list.clientHeight;
  }, [open, activeId, id, items]);

  // Browsers can replay a mouse move when the list scrolls under a resting pointer; only real
  // pointer movement may take the active row away from the keyboard.
  const pointerRef = useRef<{ x: number; y: number } | null>(null);
  const handleHover = (event: MouseEvent, item: CommandMenuItem) => {
    const last = pointerRef.current;
    if (last && last.x === event.clientX && last.y === event.clientY) return;
    pointerRef.current = { x: event.clientX, y: event.clientY };
    setModality("pointer");
    // Disabled rows highlight too, as with the arrow keys; clicking one still does nothing.
    if (item.id !== activeId) onActiveChange(item.id);
  };

  const empty = items.length === 0;
  const unavailable = items.filter((item) => item.disabled).length;
  const status = empty
    ? loading
      ? loadingText
      : emptyText
    : `${items.length} ${items.length === 1 ? "result" : "results"}${unavailable > 0 ? `, ${unavailable} unavailable` : ""}`;
  const highlight = query.replace(/^[$/]+/, "").trim();

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        if (!next) onClose();
      }}
      anchorRef={anchorRef}
      placement={placement}
      // Clears the composer's 3px focus ring.
      offset={8}
      role="group"
      autoFocus={false}
      matchAnchorWidth={matchAnchorWidth}
      container={container}
      className={cx(styles.popover, className)}
    >
      <div className={styles.menu} data-modality={modality} onMouseDown={keepFocus}>
        <div
          ref={listRef}
          id={id}
          role="listbox"
          // Focus stays in the field, so the scrolling list is not a Tab stop of its own.
          tabIndex={-1}
          aria-label={label}
          aria-busy={loading || undefined}
          className={cx(styles.list, empty && styles.listEmpty)}
        >
          {sections.map((section, sectionIndex) => {
            if (section.items.length === 0) return null;
            const headingId = section.label ? `${id}-section-${sectionIndex}` : undefined;
            return (
              <div key={section.id} role="group" aria-labelledby={headingId} className={styles.section}>
                {section.label ? (
                  <div id={headingId} role="presentation" className={styles.sectionLabel}>
                    {section.label}
                  </div>
                ) : null}
                {section.items.map((item) => (
                  <CommandMenuOption
                    key={item.id}
                    item={item}
                    domId={commandMenuOptionId(id, item.id)}
                    active={item.id === activeId}
                    highlight={highlight}
                    onHover={handleHover}
                    onSelect={onSelect}
                  />
                ))}
              </div>
            );
          })}
        </div>
        {empty ? (
          <p className={styles.status} aria-hidden="true">
            {loading ? <LoaderIcon size={14} className={styles.spinner} /> : null}
            {loading ? loadingText : emptyText}
          </p>
        ) : null}
        {footer ? <div className={styles.footer}>{footer}</div> : null}
        <div role="status" className={a11y.visuallyHidden}>
          {status}
        </div>
      </div>
    </Popover>
  );
}

/**
 * One row of a CommandMenu or CommandPalette listbox. Focus stays in the field, so the row is an
 * `aria-activedescendant` target, never focused itself. Shared within this folder only.
 */
export function CommandMenuOption({
  item,
  domId,
  active,
  highlight,
  size = "sm",
  onHover,
  onSelect,
}: {
  item: CommandMenuItem;
  domId: string;
  active: boolean;
  highlight: string;
  /** `sm` (--aidex-row-sm) in the anchored menu, `md` (--aidex-row-md) in the palette. */
  size?: "sm" | "md";
  onHover: (event: MouseEvent, item: CommandMenuItem) => void;
  onSelect: (item: CommandMenuItem) => void;
}) {
  const badge = typeof item.badge === "string" ? { label: item.badge } : item.badge;
  const [before, match, after] = highlightMatch(item.title, highlight);
  const name = `${item.prefix ?? ""}${item.title}`;
  const tooltip = item.tooltip ?? [name, item.description].filter(Boolean).join(" — ");
  // The default tooltip only repeats the row's text, so it shows only while that text is cut off.
  const cut = useTextCut(domId, [styles.title, styles.description]);
  // One spoken string in reading order, so the name does not depend on how a browser joins the parts.
  const accessibleName = [name, item.description, badge?.label].filter(Boolean).join(", ");
  return (
    <Tooltip
      content={tooltip}
      disabled={item.tooltip === undefined && !cut}
      side="right"
      sideOffset={8}
      // The row's accessible name already holds its full text.
      describe={false}
    >
      <div
        id={domId}
        role="option"
        aria-selected={active}
        aria-disabled={item.disabled || undefined}
        aria-label={accessibleName}
        className={cx(styles.option, size === "md" && styles.optionMd, active && styles.active, item.disabled && styles.disabled)}
        onMouseDown={keepFocus}
        onMouseMove={(event) => onHover(event, item)}
        onClick={() => {
          if (!item.disabled) onSelect(item);
        }}
      >
        {item.icon ? (
          <span className={styles.icon} aria-hidden="true">
            {item.icon}
          </span>
        ) : null}
        <span className={styles.text}>
          <span className={styles.title}>
            {item.prefix ? <span className={styles.prefix}>{item.prefix}</span> : null}
            {before}
            {match ? <mark className={styles.match}>{match}</mark> : null}
            {after}
          </span>
          {item.description ? <span className={styles.description}>{item.description}</span> : null}
        </span>
        {badge ? (
          <Badge tone={badge.tone ?? "neutral"} size="sm" icon={badge.icon} className={styles.badge}>
            {badge.label}
          </Badge>
        ) : null}
      </div>
    </Tooltip>
  );
}
