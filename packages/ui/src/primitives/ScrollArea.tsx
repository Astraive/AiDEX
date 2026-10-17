"use client";

import { ScrollArea as BaseScrollArea } from "@base-ui/react/scroll-area";
import type { ComponentPropsWithRef, ReactNode, Ref } from "react";
import { cx } from "../utils/cx";
import styles from "./ScrollArea.module.css";

export type ScrollAreaProps = Omit<ComponentPropsWithRef<"div">, "children"> & {
  children?: ReactNode;
  /** Which axes scroll (default `vertical`). */
  orientation?: "vertical" | "horizontal" | "both";
  /** Fades content out over 24px at an edge that hides more of it (default true). */
  fade?: boolean;
  /** Applied to the scrolling viewport, e.g. for padding; the root takes `className`. */
  viewportClassName?: string;
  viewportRef?: Ref<HTMLDivElement>;
  /**
   * Overrides the viewport's Tab stop. An overflowing viewport is focusable by default so keyboard
   * users can scroll it; pass -1 when focusable content inside already scrolls it into view.
   */
  viewportTabIndex?: number;
};

/**
 * A scroll container with thin overlay scrollbars (they take no layout width and show on hover or
 * while scrolling) and edge fades. It fills its parent: give the parent a height or a max-height.
 */
export function ScrollArea({
  children,
  orientation = "vertical",
  fade = true,
  className,
  viewportClassName,
  viewportRef,
  viewportTabIndex,
  ...rootProps
}: ScrollAreaProps) {
  const vertical = orientation !== "horizontal";
  const horizontal = orientation !== "vertical";
  return (
    <BaseScrollArea.Root {...rootProps} className={cx(styles.root, className)}>
      <BaseScrollArea.Viewport
        ref={viewportRef}
        {...(viewportTabIndex === undefined ? null : { tabIndex: viewportTabIndex })}
        className={cx(
          styles.viewport,
          fade && vertical && styles.fadeY,
          fade && horizontal && styles.fadeX,
          fade && vertical && horizontal && styles.fadeBoth,
          viewportClassName,
        )}
      >
        {children}
      </BaseScrollArea.Viewport>
      {vertical ? (
        <BaseScrollArea.Scrollbar orientation="vertical" className={styles.scrollbar}>
          <BaseScrollArea.Thumb className={styles.thumb} />
        </BaseScrollArea.Scrollbar>
      ) : null}
      {horizontal ? (
        <BaseScrollArea.Scrollbar orientation="horizontal" className={styles.scrollbar}>
          <BaseScrollArea.Thumb className={styles.thumb} />
        </BaseScrollArea.Scrollbar>
      ) : null}
      {vertical && horizontal ? <BaseScrollArea.Corner /> : null}
    </BaseScrollArea.Root>
  );
}
