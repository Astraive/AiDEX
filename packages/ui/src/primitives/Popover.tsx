"use client";

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type FocusEvent,
  type ReactNode,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { cx } from "../utils/cx";
import {
  isBelowModalLayer,
  isInsideDescendantLayer,
  isTopLayer,
  onLayerPush,
  PORTAL_ROOT_ATTRIBUTE,
  pushLayer,
  tabbableAfter,
  tabbablesIn,
} from "./layers";
import styles from "./Popover.module.css";

export type PopoverPlacement = "bottom-start" | "bottom-end" | "top-start" | "top-end";

export type PopoverProps = {
  open: boolean;
  /** Called with `false` on Escape, an outside pointer press, or focus leaving the popover. */
  onOpenChange: (open: boolean) => void;
  /** Element the popover is positioned against; focus returns to it when the popover closes from inside. */
  anchorRef: RefObject<HTMLElement | null>;
  /** Preferred side and alignment; the popover flips vertically when it would overflow the viewport. */
  placement?: PopoverPlacement;
  /** Gap between the anchor and the popover in pixels (default 4). */
  offset?: number;
  /** Accessible name; use `labelledBy` instead when a visible heading names the popover. */
  label?: string;
  labelledBy?: string;
  /** `group` suits a surface that only wraps another widget, e.g. a listbox driven from a text field. */
  role?: "dialog" | "menu" | "listbox" | "group";
  /** Element focused on open; defaults to the first focusable element inside the popover. */
  initialFocusRef?: RefObject<HTMLElement | null>;
  /**
   * When false, focus stays where it is on open and is not moved back on close: for a popup driven
   * from a text field that keeps focus (combobox pattern with `aria-activedescendant`). Default true.
   */
  autoFocus?: boolean;
  /** Sizes the popover to the anchor's width, e.g. a suggestion list under a composer. */
  matchAnchorWidth?: boolean;
  /**
   * Portal target. Defaults to the nearest `[data-aidex-portal-root]` around the anchor (a Dialog
   * popup), so a popover opened inside a dialog stays inside its focus trap, else `document.body`.
   */
  container?: HTMLElement | null;
  /** Id of the popover surface, e.g. for the trigger's `aria-controls`. */
  id?: string;
  className?: string;
  children: ReactNode;
};

/** Keeps the surface this far from the viewport edges (collision padding). */
const VIEWPORT_MARGIN = 8;
/** Exit transition length; the surface unmounts after it (or at once when motion is reduced). */
const EXIT_MS = 150;
const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

type Side = "top" | "bottom";
type Position = {
  top: number;
  left: number;
  side: Side;
  availableHeight: number;
  availableWidth: number;
  anchorWidth: number;
  originX: number;
  width?: number;
};

function computePosition(
  anchor: DOMRect,
  width: number,
  height: number,
  placement: PopoverPlacement,
  offset: number,
): Position {
  const viewportWidth = document.documentElement.clientWidth || window.innerWidth;
  const viewportHeight = window.innerHeight;
  const [preferredSide, align] = placement.split("-") as [Side, "start" | "end"];
  const spaceBelow = viewportHeight - anchor.bottom - offset - VIEWPORT_MARGIN;
  const spaceAbove = anchor.top - offset - VIEWPORT_MARGIN;
  let side = preferredSide;
  if (side === "bottom" && height > spaceBelow && spaceAbove > spaceBelow) side = "top";
  else if (side === "top" && height > spaceAbove && spaceBelow > spaceAbove) side = "bottom";
  const availableHeight = Math.max(0, Math.floor(side === "bottom" ? spaceBelow : spaceAbove));
  const renderedHeight = Math.min(height, availableHeight);
  const preferredTop = side === "bottom" ? anchor.bottom + offset : anchor.top - offset - renderedHeight;
  // Clamp vertically too: an anchor scrolled partly off screen must not push the surface past an edge.
  const maxTop = Math.max(VIEWPORT_MARGIN, viewportHeight - VIEWPORT_MARGIN - renderedHeight);
  const top = Math.min(Math.max(preferredTop, VIEWPORT_MARGIN), maxTop);
  const preferredLeft = align === "start" ? anchor.left : anchor.right - width;
  const left = Math.max(VIEWPORT_MARGIN, Math.min(preferredLeft, viewportWidth - width - VIEWPORT_MARGIN));
  const originX = Math.min(Math.max(anchor.left + anchor.width / 2 - left, 0), width);
  return {
    top: Math.round(top),
    left: Math.round(left),
    side,
    availableHeight,
    availableWidth: Math.max(0, viewportWidth - VIEWPORT_MARGIN * 2),
    anchorWidth: Math.round(anchor.width),
    originX: Math.round(originX),
  };
}

function samePosition(a: Position | null, b: Position): boolean {
  return (
    a !== null &&
    a.top === b.top &&
    a.left === b.left &&
    a.side === b.side &&
    a.availableHeight === b.availableHeight &&
    a.availableWidth === b.availableWidth &&
    a.anchorWidth === b.anchorWidth &&
    a.originX === b.originX &&
    a.width === b.width
  );
}

function prefersReducedMotion(): boolean {
  if (document.documentElement.dataset.reduceMotion === "true") return true;
  return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/**
 * Non-modal floating surface anchored to an element. It portals out of clipping ancestors (into the
 * enclosing dialog, if any), flips vertically and clamps to stay on screen, follows the anchor as it
 * moves, moves focus inside on open, and closes on Escape (returning focus to the anchor), on a
 * pointer press outside, or when focus leaves it. Tab past either end leaves the popover as if it
 * sat right after its anchor. Nested layers answer Escape and outside presses top-first.
 */
export function Popover({ open, container, anchorRef, ...props }: PopoverProps) {
  // Portals need the DOM; skip the first (prerendered) pass so static export and hydration agree.
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);
  // Each opening gets its own surface; a closing one stays until its exit transition ends.
  const [session, setSession] = useState(open ? 1 : 0);
  const [present, setPresent] = useState(open);
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setPresent(true);
      setSession((current) => current + 1);
    }
  }
  if (!mounted || !present) return null;
  const target =
    container ?? anchorRef.current?.closest<HTMLElement>(`[${PORTAL_ROOT_ATTRIBUTE}]`) ?? document.body;
  return createPortal(
    <PopoverSurface key={session} {...props} anchorRef={anchorRef} open={open} onExited={() => setPresent(false)} />,
    target,
  );
}

function PopoverSurface({
  open,
  onExited,
  onOpenChange,
  anchorRef,
  placement = "bottom-start",
  offset = 4,
  label,
  labelledBy,
  role = "dialog",
  initialFocusRef,
  autoFocus = true,
  matchAnchorWidth = false,
  id,
  className,
  children,
}: Omit<PopoverProps, "container"> & { onExited: () => void }) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const beforeGuardRef = useRef<HTMLSpanElement>(null);
  const afterGuardRef = useRef<HTMLSpanElement>(null);
  const [position, setPosition] = useState<Position | null>(null);
  const [starting, setStarting] = useState(true);
  const [anchorHidden, setAnchorHidden] = useState(false);
  const restoreFocusRef = useRef(true);
  const onOpenChangeRef = useRef(onOpenChange);
  const onExitedRef = useRef(onExited);
  useLayoutEffect(() => {
    onOpenChangeRef.current = onOpenChange;
    onExitedRef.current = onExited;
  });

  const close = useCallback((restoreFocus: boolean) => {
    restoreFocusRef.current = restoreFocus;
    onOpenChangeRef.current(false);
  }, []);

  const updatePosition = useCallback(() => {
    const surface = surfaceRef.current;
    const anchor = anchorRef.current;
    if (!surface || !anchor) return;
    const anchorRect = anchor.getBoundingClientRect();
    let width = surface.offsetWidth;
    if (matchAnchorWidth) {
      // Apply the width before measuring so the height reflects the final line wrapping.
      width = Math.round(anchorRect.width);
      surface.style.width = `${width}px`;
    }
    const next: Position = computePosition(anchorRect, width, surface.offsetHeight, placement, offset);
    if (matchAnchorWidth) next.width = width;
    setPosition((previous) => (samePosition(previous, next) ? previous : next));
  }, [anchorRef, placement, offset, matchAnchorWidth]);

  // Position before the first paint, then follow the anchor while open: size changes, scrolling,
  // and layout shifts that move the anchor without resizing it (checked once per frame).
  useLayoutEffect(updatePosition, [updatePosition]);
  useEffect(() => {
    if (!open) return;
    let frame = 0;
    let last = "";
    const track = () => {
      const anchor = anchorRef.current;
      if (anchor) {
        const rect = anchor.getBoundingClientRect();
        const key = `${rect.top},${rect.left},${rect.width},${rect.height},${window.innerWidth},${window.innerHeight}`;
        if (key !== last) {
          last = key;
          updatePosition();
        }
      }
      frame = requestAnimationFrame(track);
    };
    frame = requestAnimationFrame(track);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(() => updatePosition());
    if (surfaceRef.current) observer?.observe(surfaceRef.current);
    return () => {
      cancelAnimationFrame(frame);
      observer?.disconnect();
    };
  }, [open, anchorRef, updatePosition]);

  // Hide the surface while its anchor is scrolled out of view or clipped away.
  useEffect(() => {
    const anchor = anchorRef.current;
    if (!open || !anchor || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(([entry]) => setAnchorHidden(entry ? !entry.isIntersecting : false));
    observer.observe(anchor);
    return () => observer.disconnect();
  }, [open, anchorRef]);

  // Enter transition: paint the starting style once (two frames, since a click can flush effects
  // before the first paint), then let it transition to the resting style.
  useEffect(() => {
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => setStarting(false));
    });
    return () => cancelAnimationFrame(frame);
  }, []);

  // Exit: wait for the transition (or skip it when motion is reduced), then unmount.
  useEffect(() => {
    if (open) return;
    const surface = surfaceRef.current;
    if (!surface || prefersReducedMotion()) {
      onExitedRef.current();
      return;
    }
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      onExitedRef.current();
    };
    const onTransitionEnd = (event: TransitionEvent) => {
      if (event.target === surface) finish();
    };
    surface.addEventListener("transitionend", onTransitionEnd);
    // Fallback for a transition that never runs (e.g. the surface was already transparent).
    const timer = window.setTimeout(finish, EXIT_MS + 50);
    return () => {
      surface.removeEventListener("transitionend", onTransitionEnd);
      window.clearTimeout(timer);
    };
  }, [open]);

  // Register as a layer while open, so nested layers answer Escape and outside presses top-first.
  useLayoutEffect(() => {
    const surface = surfaceRef.current;
    if (!open || !surface) return;
    return pushLayer(surface);
  }, [open]);

  // A dialog or the command palette opening over the page closes the popover, which would otherwise
  // float above the modal and stay in its layer list. Focus is the modal's to place.
  useEffect(() => {
    if (!open) return;
    return onLayerPush(() => {
      const surface = surfaceRef.current;
      if (surface && isBelowModalLayer(surface)) close(false);
    });
  }, [open, close]);

  // Move focus in on open.
  useLayoutEffect(() => {
    const surface = surfaceRef.current;
    if (!surface || !autoFocus) return;
    const focusInitial = () => {
      const target = initialFocusRef?.current ?? surface.querySelector<HTMLElement>(FOCUSABLE) ?? surface;
      target.focus({ preventScroll: true });
    };
    focusInitial();
    const frame = requestAnimationFrame(() => {
      if (!surface.contains(document.activeElement)) focusInitial();
    });
    return () => cancelAnimationFrame(frame);
    // Runs once per opening; the refs are stable for the popover's lifetime.
  }, []);

  // On close, return focus to the anchor unless the user moved it elsewhere.
  useLayoutEffect(() => {
    const surface = surfaceRef.current;
    if (open || !surface || !autoFocus || !restoreFocusRef.current) return;
    const active = document.activeElement;
    if (!active || active === document.body || surface.contains(active)) {
      anchorRef.current?.focus({ preventScroll: true });
    }
  }, [open, autoFocus, anchorRef]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing) return;
      const surface = surfaceRef.current;
      // A layer opened on top (a menu or select inside this popover) closes first.
      if (!surface || !isTopLayer(surface)) return;
      // Capture phase: the popover closes before anything below it (including a dialog) reacts.
      event.preventDefault();
      event.stopPropagation();
      close(true);
    };
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      const surface = surfaceRef.current;
      if (!surface || surface.contains(target) || anchorRef.current?.contains(target)) return;
      if (isInsideDescendantLayer(target, surface)) return;
      close(false);
    };
    // Focus landing anywhere else closes the popover, also when it moves on from the anchor (where
    // Shift+Tab out of the popover leaves it open, as Radix does). Moves out of the surface and through
    // the focus guards are handled by `handleBlur` and `handleGuardFocus`.
    const onFocusIn = (event: globalThis.FocusEvent) => {
      const target = event.target;
      const surface = surfaceRef.current;
      if (!surface || !(target instanceof Node)) return;
      const ownedByPopover = (node: Node) =>
        surface.contains(node) || node === beforeGuardRef.current || node === afterGuardRef.current;
      const from = event.relatedTarget;
      if ((from instanceof Node && ownedByPopover(from)) || ownedByPopover(target)) return;
      if (anchorRef.current?.contains(target) || isInsideDescendantLayer(target, surface)) return;
      close(false);
    };
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("focusin", onFocusIn, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("focusin", onFocusIn, true);
    };
  }, [open, anchorRef, close]);

  const isGuard = (node: unknown) => node === beforeGuardRef.current || node === afterGuardRef.current;

  const handleBlur = (event: FocusEvent<HTMLDivElement>) => {
    const next = event.relatedTarget;
    if (!open || !(next instanceof Node) || isGuard(next)) return;
    const surface = surfaceRef.current;
    if (!surface || surface.contains(next) || anchorRef.current?.contains(next)) return;
    if (isInsideDescendantLayer(next, surface)) return;
    close(false);
  };

  // Focus guards: Tab past the last item goes to whatever follows the anchor (and closes); Shift+Tab
  // past the first returns to the anchor. Focus arriving from outside goes to the nearest end.
  const handleGuardFocus = (edge: "before" | "after") => (event: FocusEvent<HTMLSpanElement>) => {
    const surface = surfaceRef.current;
    const anchor = anchorRef.current;
    if (!surface) return;
    const fromInside = event.relatedTarget instanceof Node && surface.contains(event.relatedTarget);
    if (!fromInside) {
      const items = tabbablesIn(surface);
      const target = (edge === "before" ? items[0] : items.at(-1)) ?? surface;
      target.focus({ preventScroll: true });
      return;
    }
    if (edge === "before") {
      anchor?.focus({ preventScroll: true });
      return;
    }
    const next = anchor ? tabbableAfter(anchor, surface) : null;
    (next ?? anchor)?.focus({ preventScroll: true });
    close(false);
  };

  const side = position?.side ?? (placement.split("-")[0] as Side);
  const style = {
    top: position?.top ?? 0,
    left: position?.left ?? 0,
    width: position?.width,
    transformOrigin: position ? `${position.originX}px ${side === "bottom" ? "top" : "bottom"}` : undefined,
    "--aidex-popover-available-height": position ? `${position.availableHeight}px` : undefined,
    "--aidex-popover-available-width": position ? `${position.availableWidth}px` : undefined,
    "--aidex-popover-anchor-width": position ? `${position.anchorWidth}px` : undefined,
  } as CSSProperties;

  return (
    <>
      <span
        ref={beforeGuardRef}
        tabIndex={open ? 0 : -1}
        aria-hidden="true"
        data-focus-guard=""
        className={styles.guard}
        onFocus={handleGuardFocus("before")}
      />
      <div
        ref={surfaceRef}
        id={id}
        role={role}
        aria-label={label}
        aria-labelledby={labelledBy}
        tabIndex={-1}
        data-side={side}
        data-starting-style={open && starting ? "" : undefined}
        data-ending-style={open ? undefined : ""}
        data-anchor-hidden={anchorHidden ? "" : undefined}
        inert={!open}
        className={cx(styles.popover, className)}
        style={style}
        onBlur={handleBlur}
      >
        {children}
      </div>
      <span
        ref={afterGuardRef}
        tabIndex={open ? 0 : -1}
        aria-hidden="true"
        data-focus-guard=""
        className={styles.guard}
        onFocus={handleGuardFocus("after")}
      />
    </>
  );
}
