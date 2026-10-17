import type { ComponentPropsWithRef } from "react";
import { BrandIcon } from "../../icons/brands/BrandIcon";
import { ChevronDownIcon } from "../../icons/interface";
import { BoxIcon } from "../../icons/navigation";
import { Badge } from "../../primitives/Badge";
import { Tooltip } from "../../primitives/Tooltip";
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
  /** `sm` (`--aidex-control-sm`) for composer toolbars; `md` (`--aidex-control-md`) beside standard buttons. */
  size?: "sm" | "md";
  /** Shortcut that toggles the picker, shown in the tooltip (e.g. "Ctrl+Shift+M"). */
  shortcutLabel?: string;
  disabled?: boolean;
};

/**
 * Compact button that shows the selected model and opens the `ModelPicker`. Its tooltip names the
 * full model and the shortcut; a caller's `title` replaces that text and is never a native tooltip.
 */
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
  return (
    <Tooltip
      content={title ?? tooltipName}
      shortcut={title === undefined ? shortcutLabel : undefined}
      disabled={disabled || open}
      // The button's own text already names the model and provider.
      describe={title !== undefined}
    >
      <button
        type="button"
        aria-haspopup="dialog"
        aria-expanded={open}
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
    </Tooltip>
  );
}
