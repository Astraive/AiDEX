"use client";

import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import {
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type KeyboardEvent,
  type MouseEvent,
  type ReactNode,
  type Ref,
} from "react";
import { SearchIcon } from "../../icons/actions";
import { LoaderIcon } from "../../icons/interface";
import { Kbd } from "../../primitives/Kbd";
import { isTopLayer, LAYER_ATTRIBUTE, PORTAL_ROOT_ATTRIBUTE, pushLayer } from "../../primitives/layers";
import { cx } from "../../utils/cx";
import a11y from "../content/a11y.module.css";
import {
  CommandMenuOption,
  type CommandMenuItem,
  type CommandMenuModality,
  type CommandMenuSection,
} from "./CommandMenu";
import {
  commandMenuKeyAction,
  commandMenuOptionId,
  flattenCommandMenu,
  resolveCommandMenuActive,
} from "./model";
import styles from "./CommandPalette.module.css";

export type CommandPaletteProps = {
  open: boolean;
  /** Escape, a press on the backdrop, or the caller after a pick. */
  onClose: () => void;
  /** Accessible name of the dialog, e.g. "Search". */
  label: string;
  /** Accessible name of the search field (default `label`). */
  inputLabel?: string;
  placeholder?: string;
  query: string;
  onQueryChange: (query: string) => void;
  /** Results, best match first; the caller filters them from `query`. */
  sections: readonly CommandMenuSection[];
  /** Enter or a click on a row that can run. The palette stays open until `onClose`. */
  onSelect: (item: CommandMenuItem) => void;
  /** The search field, which has focus while the palette is open. */
  inputRef?: Ref<HTMLInputElement>;
  /** Shows `loadingText` while there are no rows yet. */
  loading?: boolean;
  loadingText?: string;
  emptyText?: string;
  /** Content between the field and the results, e.g. a `Notice` about a failed lookup. */
  notice?: ReactNode;
  /** Replaces the key hints under the results. */
  footer?: ReactNode;
};

// Rows keep focus in the search field (aria-activedescendant), so pointer presses must not move it.
const keepFocus = (event: MouseEvent) => event.preventDefault();

function assignRef<T>(ref: Ref<T> | undefined, value: T | null) {
  if (typeof ref === "function") ref(value);
  else if (ref) ref.current = value;
}

/**
 * A search dialog for jumping somewhere (Base UI Dialog): a search field that keeps focus, and a
 * list of results driven like `CommandMenu`. ArrowUp/ArrowDown move the highlight, Enter runs it,
 * Escape or a press outside closes and returns focus to where it was. Fully controlled: the caller
 * owns the query and filters `sections` from it.
 */
export function CommandPalette({
  open,
  onClose,
  label,
  inputLabel,
  placeholder,
  query,
  onQueryChange,
  sections,
  onSelect,
  inputRef,
  loading = false,
  loadingText = "Searching…",
  emptyText = "No results.",
  notice,
  footer,
}: CommandPaletteProps) {
  const listId = useId();
  const fieldRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const [popup, setPopup] = useState<HTMLDivElement | null>(null);
  const items = useMemo(() => flattenCommandMenu(sections), [sections]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [modality, setModality] = useState<CommandMenuModality>("keyboard");

  // Opening, or a new query, highlights the first row; afterwards the highlight stays on its row
  // while that row is listed. A layout effect, so Enter never acts on a stale row.
  const previousRef = useRef({ open: false, query });
  useLayoutEffect(() => {
    const previous = previousRef.current;
    previousRef.current = { open, query };
    if (!open) return;
    const restart = !previous.open || previous.query !== query;
    if (restart) setModality("keyboard");
    const next = resolveCommandMenuActive(items, activeId, restart);
    if (next !== activeId) setActiveId(next);
  }, [open, query, items, activeId]);

  // A modal layer while open: the shell's shortcuts (e.g. Mod+B) stand down.
  useLayoutEffect(() => {
    if (!open || !popup) return;
    return pushLayer(popup, { modal: true });
  }, [open, popup]);

  // Scroll the active row into view inside the list only, never the page.
  useLayoutEffect(() => {
    const list = listRef.current;
    if (!open || !list || activeId === null) return;
    const option = document.getElementById(commandMenuOptionId(listId, activeId));
    if (!option || !list.contains(option)) return;
    const group = option.parentElement;
    const top = group && group !== list && option === group.querySelector('[role="option"]') ? group.offsetTop : option.offsetTop;
    const bottom = option.offsetTop + option.offsetHeight;
    const padding = parseFloat(getComputedStyle(list).paddingTop) || 0;
    if (top - padding < list.scrollTop) list.scrollTop = Math.max(0, top - padding);
    else if (bottom + padding > list.scrollTop + list.clientHeight) list.scrollTop = bottom + padding - list.clientHeight;
  }, [open, activeId, listId, items]);

  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
    // Tab stays a focus key here (the dialog keeps focus inside); only arrows and Enter drive the list.
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp" && event.key !== "Enter") return;
    const action = commandMenuKeyAction(items, activeId, event.key, event.shiftKey);
    if (action.type === "pass") return;
    event.preventDefault();
    if (action.type === "move") {
      setModality("keyboard");
      setActiveId(action.id);
    } else if (action.type === "select") onSelect(action.item);
  };

  // Browsers can replay a mouse move when the list scrolls under a resting pointer; only real
  // pointer movement may take the highlight away from the keyboard.
  const pointerRef = useRef<{ x: number; y: number } | null>(null);
  const handleHover = (event: MouseEvent, item: CommandMenuItem) => {
    const last = pointerRef.current;
    if (last && last.x === event.clientX && last.y === event.clientY) return;
    pointerRef.current = { x: event.clientX, y: event.clientY };
    setModality("pointer");
    if (item.id !== activeId) setActiveId(item.id);
  };

  const empty = items.length === 0;
  const unavailable = items.filter((item) => item.disabled).length;
  const status = empty
    ? loading
      ? loadingText
      : emptyText
    : `${items.length} ${items.length === 1 ? "result" : "results"}${unavailable > 0 ? `, ${unavailable} unavailable` : ""}`;
  const highlight = query.trim();

  return (
    <BaseDialog.Root
      open={open}
      onOpenChange={(next, details) => {
        if (next) return;
        // Escape belongs to the top layer, e.g. a menu opened from a result.
        if (details.reason === "escape-key" && popup !== null && !isTopLayer(popup)) {
          details.cancel();
          return;
        }
        onClose();
      }}
    >
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className={styles.backdrop} />
        <BaseDialog.Viewport className={styles.viewport}>
          <BaseDialog.Popup
            ref={setPopup}
            className={styles.popup}
            initialFocus={fieldRef}
            {...{ [PORTAL_ROOT_ATTRIBUTE]: "", [LAYER_ATTRIBUTE]: "" }}
          >
            <BaseDialog.Title className={a11y.visuallyHidden}>{label}</BaseDialog.Title>
            <div className={styles.search}>
              <SearchIcon className={styles.searchIcon} aria-hidden="true" />
              <input
                ref={(node) => {
                  fieldRef.current = node;
                  assignRef(inputRef, node);
                }}
                type="text"
                role="combobox"
                aria-label={inputLabel ?? label}
                aria-expanded="true"
                aria-controls={listId}
                aria-autocomplete="list"
                aria-activedescendant={activeId !== null ? commandMenuOptionId(listId, activeId) : undefined}
                autoComplete="off"
                autoCorrect="off"
                spellCheck={false}
                className={styles.input}
                placeholder={placeholder}
                value={query}
                onChange={(event: ChangeEvent<HTMLInputElement>) => onQueryChange(event.target.value)}
                onKeyDown={handleKeyDown}
              />
              {loading ? <LoaderIcon className={styles.spinner} aria-hidden="true" /> : null}
            </div>
            {notice ? <div className={styles.notice}>{notice}</div> : null}
            <div className={styles.results} data-modality={modality} onMouseDown={keepFocus}>
              <div
                ref={listRef}
                id={listId}
                role="listbox"
                // Focus stays in the field, so the scrolling list is not a Tab stop of its own.
                tabIndex={-1}
                aria-label={label}
                aria-busy={loading || undefined}
                className={cx(styles.list, empty && styles.listEmpty)}
              >
                {sections.map((section, sectionIndex) => {
                  if (section.items.length === 0) return null;
                  const headingId = section.label ? `${listId}-section-${sectionIndex}` : undefined;
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
                          size="md"
                          domId={commandMenuOptionId(listId, item.id)}
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
                  {loading ? loadingText : emptyText}
                </p>
              ) : null}
            </div>
            <div className={cx(styles.footer, footer === undefined && styles.hints)}>
              {footer ?? (
                <>
                  <span className={styles.hint}>
                    <Kbd>↑</Kbd>
                    <Kbd>↓</Kbd> to move
                  </span>
                  <span className={styles.hint}>
                    <Kbd>Enter</Kbd> to open
                  </span>
                  <span className={styles.hint}>
                    <Kbd>Esc</Kbd> to close
                  </span>
                </>
              )}
            </div>
            <div role="status" className={a11y.visuallyHidden}>
              {open ? status : ""}
            </div>
          </BaseDialog.Popup>
        </BaseDialog.Viewport>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
