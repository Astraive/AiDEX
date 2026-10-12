import type { ComponentPropsWithRef } from "react";
import { cx } from "../utils/cx";
import styles from "./StatusDot.module.css";

export type StatusDotTone = "success" | "warning" | "danger" | "neutral" | "accent";

export type StatusDotProps = Omit<ComponentPropsWithRef<"span">, "children" | "role"> & {
  tone: StatusDotTone;
  /** `sm` is 6px for overlays and dense rows; `md` is 9px beside a text label. */
  size?: "sm" | "md";
  /**
   * Accessible name. With a label the dot is announced as an image; without one it is decorative
   * and a neighbouring text must carry the meaning, so color is never the only signal.
   */
  label?: string;
};

/** Small colored status dot. */
export function StatusDot({ tone, size = "md", label, className, ...props }: StatusDotProps) {
  return (
    <span
      role={label ? "img" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      title={label}
      className={cx(styles.dot, styles[size], styles[tone], className)}
      {...props}
    />
  );
}
