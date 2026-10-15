"use client";

import { Collapsible as BaseCollapsible } from "@base-ui/react/collapsible";
import { cx } from "../utils/cx";
import styles from "./Collapsible.module.css";

export type CollapsibleProps = Omit<BaseCollapsible.Root.Props, "className"> & { className?: string };

/**
 * Shows and hides a panel (Base UI Collapsible). Compose a `CollapsibleTrigger` (use `render` to
 * make a Button or a row the trigger) and a `CollapsiblePanel`.
 */
export function Collapsible({ className, ...props }: CollapsibleProps) {
  return <BaseCollapsible.Root {...props} className={className} />;
}

export type CollapsibleTriggerProps = Omit<BaseCollapsible.Trigger.Props, "className"> & { className?: string };

/** Toggles the panel; it carries `aria-expanded` and `aria-controls`, and `data-panel-open` while open. */
export function CollapsibleTrigger({ className, ...props }: CollapsibleTriggerProps) {
  return <BaseCollapsible.Trigger {...props} className={cx(styles.trigger, className)} />;
}

export type CollapsiblePanelProps = Omit<BaseCollapsible.Panel.Props, "className"> & {
  className?: string;
  /** Animates the height over 200ms (default true; never when motion is reduced). */
  animate?: boolean;
};

/**
 * The content. It stays in the page while closed and is found by the browser's find-in-page, which
 * opens it (`hidden="until-found"`), unless `hiddenUntilFound` or `keepMounted` say otherwise.
 */
export function CollapsiblePanel({
  className,
  animate = true,
  hiddenUntilFound = true,
  keepMounted = true,
  ...props
}: CollapsiblePanelProps) {
  return (
    <BaseCollapsible.Panel
      {...props}
      hiddenUntilFound={hiddenUntilFound}
      keepMounted={keepMounted}
      className={cx(styles.panel, animate && styles.animated, className)}
    />
  );
}
