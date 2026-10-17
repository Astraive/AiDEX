"use client";

import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  type KeyboardEvent,
  type ReactElement,
  type ReactNode,
  type RefObject,
} from "react";
import { tabbableAfter } from "../primitives/layers";
import { Menu, MenuItem, MenuPopup, MenuTrigger } from "../primitives/Menu";
import { Tooltip } from "../primitives/Tooltip";
import styles from "./ComposerAttachMenu.module.css";

export type ComposerAttachAction = {
  id: string;
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
  /** Why the action is unavailable: shown as the item's tooltip and read as its description. */
  disabledReason?: string;
  onSelect: () => void;
};

export type ComposerAttachMenuProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /**
   * The attach button, when it is rendered elsewhere and toggles `open` itself. The menu opens
   * against it, ArrowDown / ArrowUp on it open the menu, and focus returns to it.
   */
  anchorRef?: RefObject<HTMLElement | null>;
  /**
   * The attach button as an element (e.g. an IconButton). The menu renders it in place and wires it
   * as the menu trigger, so it needs no `onClick`, `aria-haspopup` or `aria-expanded` of its own.
   */
  trigger?: ReactElement;
  actions: readonly ComposerAttachAction[];
  label: string;
};

type CloseReason = string | null;

/**
 * The attach button's menu (for example "Upload from computer" and "From project…"), on the Menu
 * primitive: arrow keys, Home/End and typeahead move the highlight, Enter or Space picks, Escape
 * closes and returns to the button, Tab closes and moves on. Unavailable actions stay focusable and
 * show their reason. The pick runs inside the press, so it may open a file chooser.
 */
export function ComposerAttachMenu({ open, onOpenChange, anchorRef, trigger, actions, label }: ComposerAttachMenuProps) {
  const menuId = useId();
  const popupRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const closeReasonRef = useRef<CloseReason>(null);
  const openFromKeyRef = useRef<"first" | "last" | null>(null);
  const onOpenChangeRef = useRef(onOpenChange);
  useLayoutEffect(() => {
    onOpenChangeRef.current = onOpenChange;
  });
  const anchored = !trigger;

  // Anchor mode: the button lives elsewhere, so give it the keys a menu trigger has.
  useEffect(() => {
    const anchor = anchorRef?.current;
    if (!anchored || !anchor) return;
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (anchor.getAttribute("aria-disabled") === "true" || anchor.hasAttribute("disabled")) return;
      if (event.key === "Enter" || event.key === " ") {
        openFromKeyRef.current = "first";
      } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        openFromKeyRef.current = event.key === "ArrowDown" ? "first" : "last";
        onOpenChangeRef.current(true);
      }
    };
    const onPointerDown = () => {
      openFromKeyRef.current = null;
    };
    anchor.addEventListener("keydown", onKeyDown);
    anchor.addEventListener("pointerdown", onPointerDown);
    return () => {
      anchor.removeEventListener("keydown", onKeyDown);
      anchor.removeEventListener("pointerdown", onPointerDown);
    };
  }, [anchored, anchorRef]);

  // Anchor mode: point the button at the open menu, and highlight the first (or last) item when a
  // key opened it, as a menu trigger does.
  useEffect(() => {
    const anchor = anchorRef?.current;
    if (!anchored || !anchor || !open) return;
    anchor.setAttribute("aria-controls", menuId);
    const edge = openFromKeyRef.current;
    openFromKeyRef.current = null;
    let frame = 0;
    if (edge) {
      frame = requestAnimationFrame(() => {
        // The first or last action that can run, as a Base UI trigger opens.
        const items = popupRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([aria-disabled="true"])');
        (edge === "first" ? items?.[0] : items?.[items.length - 1])?.focus();
      });
    }
    return () => {
      cancelAnimationFrame(frame);
      anchor.removeAttribute("aria-controls");
    };
  }, [anchored, anchorRef, open, menuId]);

  // Focus goes back to the button before the action runs, so a dialog it opens returns focus there
  // when it closes (the item is gone by then). Still inside the press, so a file chooser may open.
  const pick = (action: ComposerAttachAction) => {
    (anchored ? anchorRef?.current : triggerRef.current)?.focus({ preventScroll: true });
    action.onSelect();
  };

  // Anchor mode: Tab closes the menu and moves on from the button, as if the menu sat after it.
  const onPopupKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const anchor = anchorRef?.current;
    if (!anchored || event.key !== "Tab" || !anchor) return;
    event.preventDefault();
    closeReasonRef.current = "tab";
    onOpenChange(false);
    const next = event.shiftKey ? anchor : (tabbableAfter(anchor, event.currentTarget) ?? anchor);
    next.focus();
  };

  return (
    <Menu
      open={open}
      onOpenChange={(next, details) => {
        closeReasonRef.current = next ? null : details.reason;
        onOpenChange(next);
      }}
    >
      {trigger ? <MenuTrigger ref={triggerRef} render={trigger} /> : null}
      <MenuPopup
        ref={popupRef}
        id={menuId}
        aria-label={label}
        side="top"
        align="start"
        anchor={anchored ? anchorRef : undefined}
        className={styles.menu}
        onKeyDown={onPopupKeyDown}
        // Anchor mode: Escape and a pick return to the button; an outside press or Tab leaves focus be,
        // and so does a pick that moved focus elsewhere (e.g. into the dialog it opened).
        finalFocus={
          anchored
            ? () => {
                const reason = closeReasonRef.current;
                if (reason !== "escape-key" && reason !== "item-press") return false;
                const active = document.activeElement;
                const moved =
                  active instanceof HTMLElement && active !== document.body && !popupRef.current?.contains(active);
                return moved ? false : (anchorRef?.current ?? false);
              }
            : undefined
        }
      >
        {actions.map((action) => {
          const item = (
            <MenuItem key={action.id} icon={action.icon} disabled={action.disabled} onClick={() => pick(action)}>
              {action.label}
            </MenuItem>
          );
          return action.disabled && action.disabledReason ? (
            <Tooltip key={action.id} content={action.disabledReason} side="right">
              {item}
            </Tooltip>
          ) : (
            item
          );
        })}
      </MenuPopup>
    </Menu>
  );
}
