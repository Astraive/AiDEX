"use client";

import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  type Ref,
  type RefObject,
} from "react";
import { isTopLayer, LAYER_ATTRIBUTE, pushLayer, tabbablesIn } from "../primitives/layers";
import { TooltipProvider } from "../primitives/Tooltip";
import { cx } from "../utils/cx";
import styles from "./AppShell.module.css";
import { clampSidebarWidth, resizeStep, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH } from "./sidebarLayout";

/** `docked` places the sidebar beside main; `overlay` floats it over main, for windows too narrow to dock. */
export type AppShellSidebarMode = "docked" | "overlay";

export type AppShellProps = {
  /** Primary icon navigation, usually a `NavRail`. */
  rail: ReactNode;
  /** Optional contextual panel, usually a `Sidebar`. Omit it to leave the shell without one. */
  sidebar?: ReactNode;
  /** Top bar of the main region, usually a `TopBar`. */
  header?: ReactNode;
  /** Ref to the scrollable main region, e.g. to reset scroll on navigation. */
  contentRef?: Ref<HTMLElement>;
  children: ReactNode;
  /**
   * `docked` (default) or `overlay`. The overlay sits over main with a scrim, makes main inert, takes
   * focus when it opens, and closes on Escape (when it is the top layer) or a press on the scrim.
   */
  sidebarMode?: AppShellSidebarMode;
  /** Whether the sidebar shows (default true). A closed sidebar stays mounted, hidden and inert. */
  sidebarOpen?: boolean;
  /** Called with `false` when the overlay asks to close (Escape or the scrim). */
  onSidebarOpenChange?: (open: boolean) => void;
  /** The sidebar width in CSS px in effect (default 256): the separator's value and where a drag starts. */
  sidebarWidth?: number;
  /** Each drag step or arrow key, with the new width. The shell already shows it. */
  onSidebarResize?: (width: number) => void;
  /**
   * The width once a drag or key press ends: save it here and pass it back as `sidebarWidth` (or set
   * `--aidex-sidebar-width`). With this or `onSidebarResize`, a docked sidebar gets a resize separator.
   */
  onSidebarResizeEnd?: (width: number) => void;
  /** Double-click on the separator: return to the default width. */
  onSidebarResetWidth?: () => void;
  /** The control that opens the sidebar again; focus returns to it when the sidebar closes with focus inside. */
  sidebarToggleRef?: RefObject<HTMLElement | null>;
};

type Drag = { pointerId: number; startX: number; startWidth: number; max: number; last: number };

/**
 * Full-window desktop frame: rail, optional sidebar, and a scrollable main region. The main region is
 * the `aidex-main` size container (page padding and the chat lane follow its width), and tooltips in
 * the shell share one delay group (the rail's own group opens sooner). Docked, the sidebar can be resized with a separator (pointer,
 * arrow keys, Home/End, double-click to reset); in overlay mode it floats over main instead.
 */
export function AppShell({
  rail,
  sidebar,
  header,
  contentRef,
  children,
  sidebarMode = "docked",
  sidebarOpen = true,
  onSidebarOpenChange,
  sidebarWidth,
  onSidebarResize,
  onSidebarResizeEnd,
  onSidebarResetWidth,
  sidebarToggleRef,
}: AppShellProps) {
  const hasSidebar = sidebar !== undefined && sidebar !== null && sidebar !== false;
  const open = hasSidebar && sidebarOpen;
  const overlay = sidebarMode === "overlay";
  const overlayOpen = overlay && open;
  const slotRef = useRef<HTMLDivElement>(null);
  const slotId = useId();
  const baseWidth = sidebarWidth ?? SIDEBAR_DEFAULT_WIDTH;
  // A drag (or key) width shown inline until the saved width catches up with it.
  const [dragWidth, setDragWidth] = useState<number | null>(null);
  const [resizing, setResizing] = useState(false);
  const dragRef = useRef<Drag | null>(null);
  const keyResizing = useRef(false);
  const width = dragWidth ?? baseWidth;
  const viewport = useViewportWidth();
  // A sidebar already wider than the window allows (auto-collapse off) may keep its width.
  const maxWidth = Math.max(clampSidebarWidth(SIDEBAR_MAX_WIDTH, viewport), Math.min(baseWidth, SIDEBAR_MAX_WIDTH));
  const resizable = !overlay && open && (onSidebarResize !== undefined || onSidebarResizeEnd !== undefined);

  useEffect(() => {
    if (!resizing && dragWidth !== null && dragWidth === baseWidth) setDragWidth(null);
  }, [resizing, dragWidth, baseWidth]);

  // The open overlay is a dismissable layer: popovers opened inside it answer Escape first.
  useLayoutEffect(() => {
    const slot = slotRef.current;
    if (!overlayOpen || !slot) return;
    return pushLayer(slot);
  }, [overlayOpen]);

  // Escape closes the overlay only when it was the top layer as the key went down, and nothing inside
  // handled the key (e.g. a field clearing itself).
  useEffect(() => {
    const slot = slotRef.current;
    if (!overlayOpen || !slot) return;
    let topAtPress = false;
    const onCapture = (event: KeyboardEvent) => {
      topAtPress = event.key === "Escape" && isTopLayer(slot);
    };
    const onBubble = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing || !topAtPress || event.defaultPrevented) return;
      topAtPress = false;
      event.preventDefault();
      onSidebarOpenChange?.(false);
    };
    window.addEventListener("keydown", onCapture, true);
    window.addEventListener("keydown", onBubble);
    return () => {
      window.removeEventListener("keydown", onCapture, true);
      window.removeEventListener("keydown", onBubble);
    };
  }, [overlayOpen, onSidebarOpenChange]);

  // The overlay takes focus as it opens.
  useEffect(() => {
    const slot = slotRef.current;
    if (!overlayOpen || !slot || slot.contains(document.activeElement)) return;
    tabbablesIn(slot)[0]?.focus({ preventScroll: true });
  }, [overlayOpen]);

  // Focus inside a sidebar that closes (or left behind on the body) goes back to the toggle.
  const wasOpen = useRef(open);
  useEffect(() => {
    const was = wasOpen.current;
    wasOpen.current = open;
    if (!was || open) return;
    const active = document.activeElement;
    if (active && active !== document.body && !slotRef.current?.contains(active)) return;
    sidebarToggleRef?.current?.focus({ preventScroll: true });
  }, [open, sidebarToggleRef]);

  const show = (next: number) => {
    setDragWidth(next);
    onSidebarResize?.(next);
  };

  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    event.currentTarget.focus({ preventScroll: true });
    dragRef.current = { pointerId: event.pointerId, startX: event.clientX, startWidth: width, max: maxWidth, last: width };
    setResizing(true);
  };

  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    const next = Math.min(drag.max, Math.max(SIDEBAR_MIN_WIDTH, Math.round(drag.startWidth + event.clientX - drag.startX)));
    if (next === drag.last) return;
    drag.last = next;
    show(next);
  };

  const endDrag = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
    setResizing(false);
    if (drag.last !== drag.startWidth) onSidebarResizeEnd?.(drag.last);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const next = resizeStep(event.key, event.shiftKey, width, maxWidth);
    if (next === null) return;
    event.preventDefault();
    keyResizing.current = true;
    setResizing(true);
    if (next !== width) show(next);
  };

  const onKeyUp = () => {
    if (!keyResizing.current) return;
    keyResizing.current = false;
    setResizing(false);
    if (dragWidth !== null && dragWidth !== baseWidth) onSidebarResizeEnd?.(dragWidth);
  };

  const slotStyle = dragWidth === null ? undefined : ({ "--aidex-sidebar-width": `${dragWidth}px` } as CSSProperties);

  return (
    <TooltipProvider>
      <div
        className={cx(styles.shell, overlay ? styles.overlay : styles.docked, resizing && styles.resizing)}
        data-sidebar={hasSidebar ? (open ? "open" : "closed") : undefined}
      >
        {rail}
        {hasSidebar ? (
          <div
            ref={slotRef}
            id={slotId}
            className={styles.sidebar}
            style={slotStyle}
            inert={!open || undefined}
            {...(overlayOpen ? { [LAYER_ATTRIBUTE]: "" } : {})}
          >
            {sidebar}
          </div>
        ) : null}
        <div className={styles.main} inert={overlayOpen || undefined}>
          {resizable ? (
            <div
              role="separator"
              aria-orientation="vertical"
              aria-label="Resize sidebar"
              aria-controls={slotId}
              aria-valuenow={width}
              aria-valuemin={SIDEBAR_MIN_WIDTH}
              aria-valuemax={maxWidth}
              tabIndex={0}
              className={styles.separator}
              onPointerDown={onPointerDown}
              onPointerMove={onPointerMove}
              onPointerUp={endDrag}
              onPointerCancel={endDrag}
              onLostPointerCapture={endDrag}
              onKeyDown={onKeyDown}
              onKeyUp={onKeyUp}
              onBlur={onKeyUp}
              onDoubleClick={() => {
                setDragWidth(null);
                onSidebarResetWidth?.();
              }}
            />
          ) : null}
          {header}
          <main ref={contentRef} className={styles.content}>
            {children}
          </main>
        </div>
        {overlay && hasSidebar ? (
          <div
            aria-hidden="true"
            className={styles.scrim}
            data-open={overlayOpen ? "" : undefined}
            // A mouse press closes at once. A touch or pen press closes on its tap instead: the mouse
            // events a tap fires come after the scrim has let go of the pointer, so closing on its
            // pointerdown would leave them (and focus) to whatever lies under it.
            onPointerDown={
              overlayOpen
                ? (event) => {
                    if (event.pointerType === "mouse") onSidebarOpenChange?.(false);
                  }
                : undefined
            }
            onClick={overlayOpen ? () => onSidebarOpenChange?.(false) : undefined}
            // A press on the scrim must not move focus: focus goes back to the toggle as the sidebar
            // closes. Chromium moves focus on mousedown, which a cancelled pointerdown would not stop.
            onMouseDown={(event) => event.preventDefault()}
          />
        ) : null}
      </div>
    </TooltipProvider>
  );
}

/** The window's CSS width, tracked so the separator's maximum follows resizes and zoom. */
function useViewportWidth(): number {
  return useSyncExternalStore(subscribeToResize, readViewportWidth, readServerViewportWidth);
}

function subscribeToResize(onChange: () => void): () => void {
  window.addEventListener("resize", onChange);
  return () => window.removeEventListener("resize", onChange);
}

function readViewportWidth(): number {
  return window.innerWidth;
}

/** Prerendering has no window; assume a wide one until hydration reads the real width. */
function readServerViewportWidth(): number {
  return 1440;
}
