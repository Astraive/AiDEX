"use client";

import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { cx } from "../utils/cx";
import styles from "./IconButton.module.css";
import { Tooltip, type TooltipSide } from "./Tooltip";

export type IconButtonVariant = "ghost" | "soft" | "solid" | "primary";
export type IconButtonSize = "sm" | "md" | "lg" | "xl";

export type IconButtonProps = Omit<ComponentPropsWithRef<"button">, "children"> & {
  /** Accessible name, also the tooltip unless `tooltip` or `title` says otherwise. */
  label: string;
  icon: ReactNode;
  variant?: IconButtonVariant;
  size?: IconButtonSize;
  shape?: "rounded" | "circle";
  /** Highlights the button as the current selection, e.g. the active navigation destination. */
  selected?: boolean;
  /** Tooltip content (default `label`); `false` shows none. */
  tooltip?: ReactNode | false;
  /** Preferred tooltip side (default `top`), e.g. `right` in a vertical rail. */
  tooltipSide?: TooltipSide;
  /** Gap between the button and its tooltip in pixels (default 4). */
  tooltipSideOffset?: number;
  /** A keyboard shortcut shown in the tooltip, e.g. "Ctrl B". */
  shortcut?: ReactNode;
  /**
   * Shown as the tooltip, never as a native `title` attribute: a reason the action is unavailable,
   * for example. Text other than the label is also linked as the button's description.
   */
  title?: string;
};

/**
 * A square or round button that shows only an icon. Its label is the accessible name and the
 * tooltip (shown on hover and keyboard focus). Refs and other props reach the `<button>`, so it can
 * be a menu or popover trigger.
 */
export function IconButton({
  label,
  icon,
  variant = "ghost",
  size = "md",
  shape = "rounded",
  selected = false,
  type = "button",
  className,
  tooltip,
  tooltipSide,
  tooltipSideOffset,
  shortcut,
  title,
  ...props
}: IconButtonProps) {
  const descriptionId = useId();
  const content = tooltip === false ? null : (title ?? tooltip ?? label);
  // A caller's description wins; otherwise text beyond the name (e.g. a disabled reason) describes it.
  const describe = typeof content === "string" && content !== label && !props["aria-describedby"];
  const button = (
    <button
      type={type}
      aria-label={label}
      className={cx(styles.button, styles[variant], styles[size], styles[shape], selected && styles.selected, className)}
      {...props}
      aria-describedby={describe ? descriptionId : props["aria-describedby"]}
    >
      {icon}
      {describe ? (
        <span id={descriptionId} hidden>
          {content}
        </span>
      ) : null}
    </button>
  );
  if (content === null || content === undefined || content === "") return button;
  return (
    <Tooltip
      content={content}
      side={tooltipSide}
      sideOffset={tooltipSideOffset}
      shortcut={shortcut}
      disabled={props.disabled}
      describe={false}
    >
      {button}
    </Tooltip>
  );
}
