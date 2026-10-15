import { useId, type ComponentPropsWithRef, type ReactNode } from "react";
import { cx } from "../utils/cx";
import fieldStyles from "./field.module.css";
import styles from "./TextField.module.css";

export type TextFieldProps = Omit<ComponentPropsWithRef<"input">, "size"> & {
  label: string;
  /** Hide the label only when surrounding content makes the field's purpose clear. */
  labelVisibility?: "visible" | "hidden";
  /** Helper text under the field, linked to the input. */
  hint?: string;
  leadingIcon?: ReactNode;
  /** Trailing control such as an `IconButton`; kept outside the label so it stays independently clickable. */
  trailing?: ReactNode;
  shape?: "rounded" | "pill";
  /** `md` (default, 32px at the comfortable density) suits forms; `lg` (36px) suits standalone fields. */
  size?: "md" | "lg";
  /** Validation message shown under the field, announced when it appears and linked to the input. */
  error?: string;
  /** Applied to the outer wrapper, not the input. */
  className?: string;
};

export function TextField({
  label,
  labelVisibility = "visible",
  hint,
  leadingIcon,
  trailing,
  shape = "rounded",
  size = "md",
  error,
  className,
  id,
  "aria-describedby": describedBy,
  ...inputProps
}: TextFieldProps) {
  const generatedId = useId();
  const inputId = id ?? generatedId;
  const hintId = `${inputId}-hint`;
  const errorId = `${inputId}-error`;
  const describedByIds =
    [describedBy, hint ? hintId : undefined, error ? errorId : undefined].filter(Boolean).join(" ") || undefined;
  const labelVisible = labelVisibility === "visible";

  return (
    <div className={cx(fieldStyles.root, className)}>
      {labelVisible ? (
        <label className={fieldStyles.label} htmlFor={inputId}>
          {label}
        </label>
      ) : null}
      <div className={cx(styles.field, styles[shape], styles[size], error && styles.invalid)}>
        {/* Wrapping label: clicking the icon or padding focuses the input. */}
        <label className={styles.control} htmlFor={inputId}>
          {leadingIcon ? <span className={styles.icon}>{leadingIcon}</span> : null}
          {labelVisible ? null : <span className={fieldStyles.visuallyHidden}>{label}</span>}
          <input
            {...inputProps}
            id={inputId}
            className={styles.input}
            aria-invalid={error ? true : inputProps["aria-invalid"]}
            aria-describedby={describedByIds}
          />
        </label>
        {trailing ? <span className={styles.trailing}>{trailing}</span> : null}
      </div>
      {hint ? (
        <p id={hintId} className={fieldStyles.hint}>
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={errorId} role="alert" className={fieldStyles.error}>
          {error}
        </p>
      ) : null}
    </div>
  );
}
