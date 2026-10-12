import type { ComponentPropsWithRef, ReactNode } from "react";
import { cx } from "../utils/cx";
import styles from "./Badge.module.css";

export type BadgeTone = "accent" | "success" | "warning" | "danger" | "neutral";

export type BadgeProps = ComponentPropsWithRef<"span"> & {
  tone?: BadgeTone;
  /** `md` (default) for standalone status pills; `sm` for dense rows such as list items and pickers. */
  size?: "sm" | "md";
  icon?: ReactNode;
};

/** Compact pill label for a status or category. */
export function Badge({ tone = "accent", size = "md", icon, className, children, ...props }: BadgeProps) {
  return (
    <span className={cx(styles.badge, styles[tone], size === "sm" && styles.sm, className)} {...props}>
      {icon ? <span className={styles.icon}>{icon}</span> : null}
      {children}
    </span>
  );
}
