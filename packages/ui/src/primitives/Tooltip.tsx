"use client";

import { Tooltip as BaseTooltip } from "@base-ui/react/tooltip";
import { cloneElement, useEffect, useId, useRef, useState, type ReactElement, type ReactNode } from "react";
import { cx } from "../utils/cx";
import { Kbd } from "./Kbd";
import { focusFollowsNavigationKey, trackNavigationKeys } from "./layers";
import styles from "./Tooltip.module.css";

export type TooltipSide = "top" | "bottom" | "left" | "right" | "inline-start" | "inline-end";

export type TooltipProps = {
  /** What the tooltip says. */
  content: ReactNode;
  /** The trigger: one element that accepts a ref and props, e.g. a button. */
  children: ReactElement<{ "aria-label"?: string; "aria-describedby"?: string; label?: unknown }>;
  /** Preferred side; it flips when there is no room (default `top`). */
  side?: TooltipSide;
  /** Gap to the trigger in pixels (default 4). */
  sideOffset?: number;
  align?: "start" | "center" | "end";
  /** A keyboard shortcut shown after the text, e.g. "Ctrl B". Hidden when keyboard hints are off. */
  shortcut?: ReactNode;
  /** Shows nothing, e.g. while the trigger is disabled. */
  disabled?: boolean;
  /** Overrides the `TooltipProvider` open delay in milliseconds. */
  delay?: number;
  /**
   * Links the text as the trigger's description (`aria-describedby`). Defaults to true when the
   * content is text that differs from the trigger's `aria-label`, so a name is never read twice.
   */
  describe?: boolean;
  className?: string;
};

export type TooltipProviderProps = {
  children?: ReactNode;
  /** Open delay in milliseconds (default 600). */
  delay?: number;
  /** Close delay in milliseconds (default 0). */
  closeDelay?: number;
  /** Another tooltip opens at once when the previous one closed within this time (default 400). */
  timeout?: number;
};

/** Shares the open delay among tooltips, so moving along a toolbar shows the next one at once. */
export function TooltipProvider({ children, delay = 600, closeDelay = 0, timeout = 400 }: TooltipProviderProps) {
  return (
    <BaseTooltip.Provider delay={delay} closeDelay={closeDelay} timeout={timeout}>
      {children}
    </BaseTooltip.Provider>
  );
}

/**
 * A short label shown on hover and keyboard focus (Base UI Tooltip). Focus shows it only when the
 * user moved focus there (Tab, an arrow key…), not when a script did, e.g. a dialog's initial focus
 * or focus going back to an opener, so the next Escape still reaches the dialog. It hides on a
 * press, on Escape, and when a container around the trigger scrolls. Text that only repeats the
 * trigger's accessible name stays visual.
 */
export function Tooltip({
  content,
  children,
  side = "top",
  sideOffset = 4,
  align = "center",
  shortcut,
  disabled = false,
  delay,
  describe,
  className,
}: TooltipProps) {
  const descriptionId = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const actionsRef = useRef<BaseTooltip.Root.Actions>(null);
  const [open, setOpen] = useState(false);
  useEffect(trackNavigationKeys, []);

  // Scrolling a container around the trigger moves it away from the tooltip: close it.
  useEffect(() => {
    if (!open) return;
    const onScroll = (event: Event) => {
      const trigger = triggerRef.current;
      if (trigger && event.target instanceof Node && event.target.contains(trigger)) actionsRef.current?.close();
    };
    document.addEventListener("scroll", onScroll, true);
    return () => document.removeEventListener("scroll", onScroll, true);
  }, [open]);

  // The trigger's accessible name: its `aria-label`, or the `label` an IconButton turns into one.
  const label = children.props["aria-label"] ?? children.props.label;
  const describedBy = describe ?? (typeof content === "string" && content !== label);
  // The trigger's own props win over the Trigger's when Base UI merges them, so extend them here.
  const trigger = describedBy
    ? cloneElement(children, { "aria-describedby": cx(children.props["aria-describedby"], descriptionId) })
    : children;

  return (
    <BaseTooltip.Root
      actionsRef={actionsRef}
      disabled={disabled}
      disableHoverablePopup
      onOpenChange={(next, details) => {
        // Focus a script placed (not the user's Tab or arrow key) shows no tooltip: otherwise it
        // would open on a keyboard-opened dialog's first focus and take that dialog's first Escape.
        if (next && details.reason === "trigger-focus" && !focusFollowsNavigationKey(performance.now())) {
          details.cancel();
          return;
        }
        setOpen(next);
      }}
    >
      <BaseTooltip.Trigger ref={triggerRef} render={trigger} delay={delay} />
      {describedBy ? (
        <span id={descriptionId} hidden>
          {content}
        </span>
      ) : null}
      <BaseTooltip.Portal>
        <BaseTooltip.Positioner side={side} sideOffset={sideOffset} align={align} className={styles.positioner}>
          <BaseTooltip.Popup className={cx(styles.popup, className)}>
            <span className={styles.text}>{content}</span>
            {shortcut ? <Kbd className={styles.shortcut}>{shortcut}</Kbd> : null}
          </BaseTooltip.Popup>
        </BaseTooltip.Positioner>
      </BaseTooltip.Portal>
    </BaseTooltip.Root>
  );
}
