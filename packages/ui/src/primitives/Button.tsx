import type { ComponentPropsWithRef, ReactNode } from "react";
import { cx } from "../utils/cx";
import styles from "./Button.module.css";

export type ButtonVariant = "primary" | "tonal" | "raised" | "outline" | "ghost";
export type ButtonSize = "sm" | "md" | "lg" | "xl";

export type ButtonProps = ComponentPropsWithRef<"button"> & {
  variant?: ButtonVariant;
  /** Heights follow the control tokens: `sm` 28, `md` 32 (default), `lg` 36, `xl` 40 at the comfortable density. */
  size?: ButtonSize;
  shape?: "rounded" | "pill";
  /** `center` groups icon and label in the middle; a full-width button then pins `trailing` to its end. */
  align?: "start" | "center";
  /** Leading icon; sized by the button. */
  icon?: ReactNode;
  /** Trailing content such as a keyboard hint or arrow. */
  trailing?: ReactNode;
  fullWidth?: boolean;
};

export function Button({
  variant = "outline",
  size = "md",
  shape = "rounded",
  align = "start",
  icon,
  trailing,
  fullWidth = false,
  type = "button",
  className,
  children,
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cx(
        styles.button,
        styles[variant],
        styles[size],
        shape === "pill" && styles.pill,
        align === "center" && styles.center,
        fullWidth && styles.fullWidth,
        align === "center" && fullWidth && trailing != null && styles.pinnedTrailing,
        className,
      )}
      {...props}
    >
      {icon ? <span className={styles.icon}>{icon}</span> : null}
      <span className={styles.label}>{children}</span>
      {trailing ? <span className={styles.trailing}>{trailing}</span> : null}
    </button>
  );
}
