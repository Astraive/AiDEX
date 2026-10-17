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
  /**
   * While `disabled`, keep the button focusable: it is marked `aria-disabled` and ignores presses
   * instead. Pass it while an action the button started runs, so focus stays on the button rather
   * than dropping to the page when it turns disabled.
   */
  focusableWhenDisabled?: boolean;
};

export function Button({
  variant = "outline",
  size = "md",
  shape = "rounded",
  align = "start",
  icon,
  trailing,
  fullWidth = false,
  focusableWhenDisabled = false,
  type = "button",
  disabled,
  onClick,
  className,
  children,
  ...props
}: ButtonProps) {
  // A soft-disabled button ignores presses (preventDefault also stops a submit button's submit).
  const softDisabled = !!disabled && focusableWhenDisabled;
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
      disabled={softDisabled ? undefined : disabled}
      aria-disabled={softDisabled ? true : props["aria-disabled"]}
      onClick={softDisabled ? (event) => event.preventDefault() : onClick}
    >
      {icon ? <span className={styles.icon}>{icon}</span> : null}
      <span className={styles.label}>{children}</span>
      {trailing ? <span className={styles.trailing}>{trailing}</span> : null}
    </button>
  );
}
