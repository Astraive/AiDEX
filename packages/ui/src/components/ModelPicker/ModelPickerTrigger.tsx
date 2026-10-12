import type { ComponentPropsWithRef } from "react";
import { BrandIcon } from "../../icons/brands/BrandIcon";
import { ChevronDownIcon } from "../../icons/interface";
import { BoxIcon } from "../../icons/navigation";
import { Badge } from "../../primitives/Badge";
import { cx } from "../../utils/cx";
import type { ModelPickerModel, ModelPickerProvider } from "./types";
import { modelDisplayName } from "./view";
import styles from "./ModelPickerTrigger.module.css";

export type ModelPickerTriggerProps = Omit<ComponentPropsWithRef<"button">, "children" | "onToggle" | "type"> & {
  /** Provider of the selected model; its brand icon leads the button. */
  provider?: ModelPickerProvider;
  /** Selected model; its short name is shown. */
  model?: ModelPickerModel;
  /** Text when no model is selected (default "Choose model"). */
  placeholder?: string;
  open: boolean;
  onToggle: () => void;
  /** `sm` (32px) for composer toolbars; `md` (40px) beside standard buttons. */
  size?: "sm" | "md";
  /** Shortcut that toggles the picker, appended to the tooltip (e.g. "Ctrl+Shift+M"). */
  shortcutLabel?: string;
  disabled?: boolean;
};

/** Compact button that shows the selected model and opens the `ModelPicker`. */
export function ModelPickerTrigger({
  provider,
  model,
  placeholder = "Choose model",
  open,
  onToggle,
  size = "sm",
  shortcutLabel,
  disabled = false,
  className,
  title,
  ...props
}: ModelPickerTriggerProps) {
  const label = model ? modelDisplayName(model) : placeholder;
  const tooltipName = `${model?.name ?? label}${model?.unavailable ? " (Unavailable)" : ""}`;
  const tooltip = title ?? (shortcutLabel ? `${tooltipName} · ${shortcutLabel}` : tooltipName);
  return (
    <button
      type="button"
      aria-haspopup="dialog"
      aria-expanded={open}
      title={tooltip}
      disabled={disabled}
      className={cx(styles.trigger, styles[size], className)}
      onClick={onToggle}
      {...props}
    >
      <span className={styles.icon} aria-hidden="true">
        {provider ? (
          <BrandIcon brand={provider.brand} monogramLabel={provider.name} size={16} aria-hidden />
        ) : (
          <BoxIcon size={16} />
        )}
      </span>
      <span className={styles.label}>
        <span className={styles.visuallyHidden}>Model: </span>
        {label}
        {provider ? <span className={styles.visuallyHidden}>, {provider.name}</span> : null}
      </span>
      {model?.unavailable ? (
        <Badge tone="warning" size="sm" className={styles.badge}>
          Unavailable
        </Badge>
      ) : null}
      <ChevronDownIcon size={14} className={styles.chevron} />
    </button>
  );
}
