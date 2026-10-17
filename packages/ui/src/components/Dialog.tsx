"use client";

import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { XIcon } from "../icons/interface";
import { IconButton } from "../primitives/IconButton";
import {
  canRestoreFocus,
  isTopLayer,
  LAYER_ATTRIBUTE,
  PORTAL_ROOT_ATTRIBUTE,
  pushLayer,
  recentlyFocused,
  tabbablesIn,
  trackFocusHistory,
} from "../primitives/layers";
import { ScrollArea } from "../primitives/ScrollArea";
import { cx } from "../utils/cx";
import { FileDropZoneContext } from "./attachments/FileDropZone";
import { containFileDrag } from "./attachments/fileDropGuard";
import styles from "./Dialog.module.css";

export type DialogProps = {
  open: boolean;
  title: string;
  onClose: () => void;
  /** Supporting text under the title, linked as the dialog's description. */
  description?: ReactNode;
  /** Width: `md` 512px (default), `lg` 720px, `xl` 960px; always clamped to the window. */
  size?: "md" | "lg" | "xl";
  /** Actions pinned under the scrolling body, e.g. Cancel and Save. */
  footer?: ReactNode;
  /**
   * Element focused on open. Without it, focus stays on an element the content focused itself
   * (e.g. a field with `autoFocus`), else goes to the first `[data-autofocus]` element in the body,
   * then the first focusable element in the body, then the body itself when it is only text that
   * scrolls (so the keyboard can scroll it), then the footer, then the Close button.
   */
  initialFocusRef?: RefObject<HTMLElement | null>;
  /**
   * Where focus goes once the dialog has closed, in place of the element that opened it: called as
   * focus would return, e.g. so a confirmed delete that removed its opener can focus the row that
   * took its place. Returning nothing (or an element that cannot take focus) keeps the default.
   */
  returnFocus?: () => HTMLElement | null | undefined;
  /** Whether Escape and a press on the backdrop close the dialog (default true). Close always does. */
  dismissible?: boolean;
  /**
   * Keeps the content mounted (hidden) while closed, so its state survives a reopen. By default the
   * content unmounts once the exit transition ends.
   */
  keepMounted?: boolean;
  children: ReactNode;
};

type Content = Pick<DialogProps, "title" | "description" | "footer" | "children">;

/**
 * Modal dialog (Base UI Dialog): traps focus, locks page scroll, closes on Escape or a press on the
 * backdrop, and returns focus to the element that opened it. Focus starts in the body, not on the
 * Close button. The header and `footer` stay put while the body scrolls. Popovers, menus, selects,
 * tooltips and toasts opened from inside sit above it. Files dragged over it never reach a drop
 * target behind it.
 */
export function Dialog({
  open,
  title,
  onClose,
  description,
  size = "md",
  footer,
  initialFocusRef,
  returnFocus,
  dismissible = true,
  keepMounted = false,
  children,
}: DialogProps) {
  const [popup, setPopup] = useState<HTMLDivElement | null>(null);
  const popupRef = useRef<HTMLDivElement | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const footerRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  // While the exit transition runs, keep showing what was there when it closed: owners often
  // derive the title or content from the state that closing clears.
  const shownRef = useRef<Content>({ title, description, footer, children });
  if (open) shownRef.current = { title, description, footer, children };
  const shown = shownRef.current;
  // Where focus goes back to on close: the element that had focus as the dialog opened (read before
  // the content mounts, since a field with `autoFocus` takes focus while it mounts), then the ones
  // focused before it, for an opener that is gone by then (e.g. an item of a menu that closed).
  const returnTargetsRef = useRef<Element[]>([]);
  const sawOpenRef = useRef(false);
  if (open !== sawOpenRef.current) {
    sawOpenRef.current = open;
    if (open && typeof document !== "undefined") {
      const active = document.activeElement;
      returnTargetsRef.current = [...(active && active !== document.body ? [active] : []), ...recentlyFocused()];
    }
  }
  useEffect(trackFocusHistory, []);
  const returnFocusRef = useRef(returnFocus);
  returnFocusRef.current = returnFocus;

  // The content mounts in the same commit as `open` turns true (so an owner's effect can focus into
  // it, as with an always-mounted dialog) and unmounts once the exit transition ends. Only the empty,
  // hidden popup stays in the page while closed.
  const [present, setPresent] = useState(open);
  const [sawOpen, setSawOpen] = useState(open);
  if (open !== sawOpen) {
    setSawOpen(open);
    if (open) setPresent(true);
  }
  const renderContent = open || present || keepMounted;

  // Whether Tab reaches anything in the body. While it does, the scrolling viewport is no Tab stop
  // (focusing a control scrolls it into view); a body of text alone leaves an overflowing viewport
  // focusable, so the keyboard can scroll it. Rechecked (once a frame at most) as the content changes.
  const [bodyTabbable, setBodyTabbable] = useState(false);
  useLayoutEffect(() => {
    const body = bodyRef.current;
    if (!open || !body) return;
    let frame = 0;
    const update = () => {
      frame = 0;
      setBodyTabbable(tabbablesIn(body).length > 0);
    };
    update();
    const observer = new MutationObserver(() => {
      if (!frame) frame = requestAnimationFrame(update);
    });
    observer.observe(body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["disabled", "tabindex", "hidden", "inert", "href", "contenteditable"],
    });
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
    };
  }, [open]);

  // Register as a modal layer while open: popovers inside answer Escape first, and the shell's
  // shortcuts (e.g. Mod+B) stand down.
  useLayoutEffect(() => {
    if (!open || !popup) return;
    return pushLayer(popup, { modal: true });
  }, [open, popup]);

  // Base UI would move focus a frame after opening; doing it here instead (a frame later, once the
  // content's own effects ran) keeps focus the content already placed, e.g. on a field it chose.
  const focusInitial = () => {
    requestAnimationFrame(() => {
      const root = popupRef.current;
      if (!root || !root.isConnected) return;
      const explicit = initialFocusRef?.current;
      const active = document.activeElement;
      const placed =
        active instanceof HTMLElement && active !== root && root.contains(active) && active !== closeRef.current;
      if (!explicit && placed) return;
      const body = bodyRef.current;
      const viewport = viewportRef.current;
      // A body of text alone that scrolls takes focus itself: from the Close button or the footer,
      // the arrow and Page keys would scroll nothing.
      const scrollingText = viewport && viewport.scrollHeight > viewport.clientHeight + 1 ? viewport : undefined;
      const target =
        explicit ??
        body?.querySelector<HTMLElement>("[data-autofocus]") ??
        (body ? tabbablesIn(body)[0] : undefined) ??
        scrollingText ??
        (footerRef.current ? tabbablesIn(footerRef.current)[0] : undefined) ??
        closeRef.current ??
        root;
      target.focus({ preventScroll: target === root || target === scrollingText });
    });
    return false;
  };
  // Once the exit transition ends (the focus trap is off by then), focus goes back to the opener (or
  // where `returnFocus` says), unless the user or the owner already moved it, e.g. into another dialog.
  const wasPresentRef = useRef(present);
  useEffect(() => {
    const closed = wasPresentRef.current && !present;
    wasPresentRef.current = present;
    if (!closed) return;
    const active = document.activeElement;
    const root = popupRef.current;
    const lost = !(active instanceof HTMLElement) || active === document.body || (root?.contains(active) ?? false);
    if (!lost) return;
    const chosen = returnFocusRef.current?.();
    const target = canRestoreFocus(chosen)
      ? chosen
      : returnTargetsRef.current.find((element) => !root?.contains(element) && canRestoreFocus(element));
    if (target instanceof HTMLElement) target.focus({ preventScroll: true });
  }, [present]);
  const setPopupElement = (element: HTMLDivElement | null) => {
    popupRef.current = element;
    setPopup(element);
  };

  return (
    <BaseDialog.Root
      open={open}
      disablePointerDismissal={!dismissible}
      onOpenChangeComplete={(isOpen) => {
        if (!isOpen) setPresent(false);
      }}
      onOpenChange={(next, details) => {
        if (next) return;
        // Escape belongs to the top layer: a popover, menu or select opened inside closes first.
        if (details.reason === "escape-key" && (!dismissible || (popup !== null && !isTopLayer(popup)))) {
          details.cancel();
          return;
        }
        onClose();
      }}
    >
      <BaseDialog.Portal keepMounted>
        <BaseDialog.Backdrop className={styles.backdrop} />
        <BaseDialog.Viewport className={styles.viewport} data-open={open ? "" : undefined}>
          <BaseDialog.Popup
            ref={setPopupElement}
            className={cx(styles.popup, styles[size])}
            data-open={open ? "" : undefined}
            initialFocus={focusInitial}
            finalFocus={false}
            {...{ [PORTAL_ROOT_ATTRIBUTE]: "", [LAYER_ATTRIBUTE]: "" }}
            onDragEnter={containFileDrag}
            onDragOver={containFileDrag}
            onDrop={containFileDrag}
          >
            {/* No page-level drop zone receives drops from inside a modal, so composers here take their own. */}
            {renderContent ? (
              <FileDropZoneContext.Provider value={false}>
                <div className={styles.header}>
                  <BaseDialog.Title className={styles.title}>{shown.title}</BaseDialog.Title>
                  {shown.description ? (
                    <BaseDialog.Description render={<div />} className={styles.description}>
                      {shown.description}
                    </BaseDialog.Description>
                  ) : null}
                </div>
                <ScrollArea
                  className={styles.scroll}
                  viewportRef={viewportRef}
                  viewportTabIndex={bodyTabbable ? -1 : undefined}
                >
                  <div ref={bodyRef} className={styles.body}>
                    {shown.children}
                  </div>
                </ScrollArea>
                {shown.footer ? (
                  <div ref={footerRef} className={styles.footer}>
                    {shown.footer}
                  </div>
                ) : null}
                {/* After the body in DOM order, so Tab reaches the content first. */}
                <IconButton
                  ref={closeRef}
                  className={styles.close}
                  label="Close dialog"
                  icon={<XIcon />}
                  onClick={onClose}
                />
              </FileDropZoneContext.Provider>
            ) : null}
          </BaseDialog.Popup>
        </BaseDialog.Viewport>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}
