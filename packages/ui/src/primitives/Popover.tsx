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
  /** Gap between the anchor and the popover in pixels (default 6). */
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
   * Portal target (default `document.body`). Pass the open `<dialog>` element when the popover is
   * opened from inside a native modal, so it renders in the same top layer.
   */
  container?: HTMLElement | null;
  /** Id of the popover surface, e.g. for the trigger's `aria-controls`. */
  id?: string;
  className?: string;
  children: ReactNode;
};

const VIEWPORT_MARGIN = 8;
const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

type Side = "top" | "bottom";
type Position = { top: number; left: number; side: Side; availableHeight: number; width?: number };

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
  const top = side === "bottom" ? anchor.bottom + offset : anchor.top - offset - renderedHeight;
  const preferredLeft = align === "start" ? anchor.left : anchor.right - width;
  const left = Math.max(VIEWPORT_MARGIN, Math.min(preferredLeft, viewportWidth - width - VIEWPORT_MARGIN));
  return { top: Math.round(top), left: Math.round(left), side, availableHeight };
}

function samePosition(a: Position | null, b: Position): boolean {
  return (
    a !== null &&
    a.top === b.top &&
    a.left === b.left &&
    a.side === b.side &&
    a.availableHeight === b.availableHeight &&
    a.width === b.width
  );
}

/**
 * Non-modal floating surface anchored to an element. It portals out of clipping ancestors, flips
 * vertically to stay on screen, moves focus inside on open, and closes on Escape (returning focus to
 * the anchor), on a pointer press outside, or when focus leaves it.
 */
export function Popover({ open, container, ...props }: PopoverProps) {
  // Portals need the DOM; skip the first (prerendered) pass so static export and hydration agree.
  const [mounted, setMounted] = useState(false);
  useEffect(() => setMounted(true), []);
  if (!mounted || !open) return null;
  return createPortal(<PopoverSurface {...props} />, container ?? document.body);
}

function PopoverSurface({
  onOpenChange,
  anchorRef,
  placement = "bottom-start",
  offset = 6,
  label,
  labelledBy,
  role = "dialog",
  initialFocusRef,
  autoFocus = true,
  matchAnchorWidth = false,
  id,
  className,
  children,
}: Omit<PopoverProps, "open" | "container">) {
  const surfaceRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<Position | null>(null);
  const restoreFocusRef = useRef(true);
  const onOpenChangeRef = useRef(onOpenChange);
  useLayoutEffect(() => {
    onOpenChangeRef.current = onOpenChange;
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

  // Position before the first paint, then follow layout changes.
  useLayoutEffect(updatePosition, [updatePosition]);
  useEffect(() => {
    let frame = 0;
    const schedule = (event?: Event) => {
      // Scrolling inside the popover does not move the anchor.
      if (event?.target instanceof Node && surfaceRef.current?.contains(event.target)) return;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(updatePosition);
    };
    const onResize = () => schedule();
    window.addEventListener("resize", onResize);
    window.addEventListener("scroll", schedule, true);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(onResize);
    if (surfaceRef.current) observer?.observe(surfaceRef.current);
    if (anchorRef.current) observer?.observe(anchorRef.current);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("resize", onResize);
      window.removeEventListener("scroll", schedule, true);
      observer?.disconnect();
    };
  }, [anchorRef, updatePosition]);

  // Move focus in on open; on close, return it to the anchor unless the user moved it elsewhere.
  useLayoutEffect(() => {
    const surface = surfaceRef.current;
    if (!surface || !autoFocus) return;
    const anchor = anchorRef.current;
    const focusInitial = () => {
      const target = initialFocusRef?.current ?? surface.querySelector<HTMLElement>(FOCUSABLE) ?? surface;
      target.focus({ preventScroll: true });
    };
    focusInitial();
    const frame = requestAnimationFrame(() => {
      if (!surface.contains(document.activeElement)) focusInitial();
    });
    return () => {
      cancelAnimationFrame(frame);
      if (!restoreFocusRef.current) return;
      const active = document.activeElement;
      if (!active || active === document.body || surface.contains(active)) anchor?.focus({ preventScroll: true });
    };
    // Runs once per opening; the refs are stable for the popover's lifetime.
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing) return;
      // Capture phase: the popover is the top-most layer, so Escape closes it before anything below
      // (including a native dialog it sits in) reacts.
      event.preventDefault();
      event.stopPropagation();
      close(true);
    };
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (surfaceRef.current?.contains(target) || anchorRef.current?.contains(target)) return;
      close(false);
    };
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("pointerdown", onPointerDown, true);
    };
  }, [anchorRef, close]);

  const handleBlur = (event: FocusEvent<HTMLDivElement>) => {
    const next = event.relatedTarget;
    if (!(next instanceof Node)) return;
    if (surfaceRef.current?.contains(next) || anchorRef.current?.contains(next)) return;
    close(false);
  };

  const style = {
    top: position?.top ?? 0,
    left: position?.left ?? 0,
    width: position?.width,
    "--aidex-popover-available-height": position ? `${position.availableHeight}px` : undefined,
  } as CSSProperties;

  return (
    <div
      ref={surfaceRef}
      id={id}
      role={role}
      aria-label={label}
      aria-labelledby={labelledBy}
      tabIndex={-1}
      data-side={position?.side ?? placement.split("-")[0]}
      className={cx(styles.popover, (position?.side ?? placement.split("-")[0]) === "top" && styles.top, className)}
      style={style}
      onBlur={handleBlur}
    >
      {children}
    </div>
  );
}
