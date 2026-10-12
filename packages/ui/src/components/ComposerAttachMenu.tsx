"use client";

import { useRef, type KeyboardEvent, type ReactNode, type RefObject } from "react";
import { Popover } from "../primitives/Popover";
import styles from "./ComposerAttachMenu.module.css";

export type ComposerAttachAction = {
  id: string;
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
  onSelect: () => void;
};

export type ComposerAttachMenuProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  anchorRef: RefObject<HTMLElement | null>;
  actions: readonly ComposerAttachAction[];
  label: string;
};

/**
 * The attach button's menu (for example "Upload from computer" and "From project…"). Focus moves to
 * the first item; ArrowUp/ArrowDown wrap, Home/End jump, Enter or Space picks, Escape closes.
 */
export function ComposerAttachMenu({ open, onOpenChange, anchorRef, actions, label }: ComposerAttachMenuProps) {
  const listRef = useRef<HTMLDivElement>(null);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const items = Array.from(
      listRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)') ?? [],
    );
    if (items.length === 0) return;
    const current = items.findIndex((item) => item === document.activeElement);
    let next = -1;
    if (event.key === "ArrowDown") next = current < 0 ? 0 : (current + 1) % items.length;
    else if (event.key === "ArrowUp") next = current < 0 ? items.length - 1 : (current - 1 + items.length) % items.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = items.length - 1;
    if (next < 0) return;
    event.preventDefault();
    items[next]?.focus();
  };

  return (
    <Popover open={open} onOpenChange={onOpenChange} anchorRef={anchorRef} placement="top-start" role="menu" label={label}>
      <div ref={listRef} className={styles.menu} onKeyDown={onKeyDown}>
        {actions.map((action) => (
          <button
            key={action.id}
            type="button"
            role="menuitem"
            className={styles.item}
            disabled={action.disabled}
            onClick={() => {
              onOpenChange(false);
              action.onSelect();
            }}
          >
            {action.icon ? (
              <span className={styles.icon} aria-hidden="true">
                {action.icon}
              </span>
            ) : null}
            <span className={styles.label}>{action.label}</span>
          </button>
        ))}
      </div>
    </Popover>
  );
}
