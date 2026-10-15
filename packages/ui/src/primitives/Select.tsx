"use client";

import { Select as BaseSelect } from "@base-ui/react/select";
import {
  useEffect,
  useId,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type ComponentPropsWithRef,
  type HTMLAttributes,
} from "react";
import { CheckIcon, ChevronDownIcon, ChevronUpIcon } from "../icons/interface";
import { cx } from "../utils/cx";
import fieldStyles from "./field.module.css";
import { LAYER_ATTRIBUTE, pushLayer } from "./layers";
import styles from "./Select.module.css";
import { createSelectChangeEvent } from "./selectEvent";

export type SelectOption = { value: string; label: string; disabled?: boolean };

export type SelectProps = Omit<ComponentPropsWithRef<"select">, "children" | "size"> & {
  label: string;
  labelVisibility?: "visible" | "hidden";
  options: readonly SelectOption[];
  hint?: string;
  /** `field` matches text fields; `ghost` is a borderless inline picker (e.g. a model switcher in a toolbar). */
  variant?: "field" | "ghost";
  /** Applied to the outer wrapper, not the trigger. */
  className?: string;
  /** Called with the chosen value, after `onChange`. */
  onValueChange?: (value: string) => void;
};

/** What a `ref` to the Select reaches: the parts of a `<select>` callers use. */
type SelectHandle = Pick<HTMLSelectElement, "focus" | "blur" | "value" | "name">;

/** Plain props a `<select>` caller may set that mean the same on the trigger button. */
const TRIGGER_PROPS = new Set(["title", "autoFocus", "tabIndex", "style", "onFocus", "onBlur", "onKeyDown"]);

/** Props passed through to the trigger: ARIA and data attributes, plus the plain ones above. */
function passThrough(props: Record<string, unknown>): HTMLAttributes<HTMLButtonElement> {
  const result: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(props)) {
    if (key.startsWith("aria-") || key.startsWith("data-") || TRIGGER_PROPS.has(key)) result[key] = value;
  }
  return result as HTMLAttributes<HTMLButtonElement>;
}

/**
 * A select on Base UI Select, drawn with AiDEX tokens instead of the platform's list: arrow keys,
 * Home/End, typeahead and Enter work, the open list lines the chosen option up with the trigger, and
 * the page behind it is inert. It keeps the native select's API: `onChange` receives an event with
 * `target.value` and `target.name`, `id` names the trigger (so `<label for>` and a SettingRow
 * `controlId` reach it; a label press focuses it without opening), `ref.current.focus()` works, and
 * `name` / `required` take part in forms through a hidden input.
 */
export function Select({
  label,
  labelVisibility = "visible",
  options,
  hint,
  variant = "field",
  className,
  id,
  ref,
  value,
  defaultValue,
  name,
  required,
  disabled,
  form,
  autoComplete,
  onChange,
  onValueChange,
  "aria-describedby": describedBy,
  "aria-label": ariaLabel,
  ...rest
}: SelectProps) {
  const generatedId = useId();
  const triggerId = id ?? generatedId;
  const labelId = `${triggerId}-label`;
  const hintId = hint ? `${triggerId}-hint` : undefined;
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [portal, setPortal] = useState<HTMLDivElement | null>(null);

  // A native select with no value shows (and holds) its first enabled option.
  const fallback = options.find((option) => !option.disabled)?.value ?? options[0]?.value ?? "";
  const [ownValue, setOwnValue] = useState(() => (defaultValue === undefined ? fallback : String(defaultValue)));
  const current = value === undefined ? ownValue : String(value);
  const valueRef = useRef(current);
  useLayoutEffect(() => {
    valueRef.current = current;
  });

  useImperativeHandle(
    ref,
    () =>
      ({
        focus: (options?: FocusOptions) => triggerRef.current?.focus(options),
        blur: () => triggerRef.current?.blur(),
        get value() {
          return valueRef.current;
        },
        get name() {
          return name ?? "";
        },
      }) satisfies SelectHandle as unknown as HTMLSelectElement,
    [name],
  );

  // A press on any `<label for>` this trigger (its own, or a SettingRow's) focuses it without
  // opening, as with a native select. Cancelling the label's click stops it reaching the trigger.
  useEffect(() => {
    const onClick = (event: MouseEvent) => {
      const trigger = triggerRef.current;
      const labelElement = event.target instanceof Element ? event.target.closest("label") : null;
      if (!trigger || !labelElement || labelElement.control !== trigger) return;
      event.preventDefault();
      if (!trigger.matches(":disabled")) trigger.focus();
    };
    document.addEventListener("click", onClick, true);
    return () => document.removeEventListener("click", onClick, true);
  }, []);

  // Register the open list as a layer, so a popover it opened from leaves Escape to it.
  useLayoutEffect(() => {
    if (!open || !portal) return;
    return pushLayer(portal);
  }, [open, portal]);

  const labelOf = (selected: unknown) => {
    const match = options.find((option) => option.value === selected);
    return (match ?? options.find((option) => option.value === fallback))?.label ?? "";
  };

  return (
    <div className={cx(fieldStyles.root, variant === "ghost" && styles.inline, className)}>
      <label
        id={labelId}
        className={labelVisibility === "visible" ? fieldStyles.label : fieldStyles.visuallyHidden}
        htmlFor={triggerId}
      >
        {label}
      </label>
      <BaseSelect.Root
        id={triggerId}
        value={current}
        items={options}
        name={name}
        form={form}
        autoComplete={autoComplete}
        required={required}
        disabled={disabled}
        open={open}
        onOpenChange={setOpen}
        onValueChange={(next) => {
          const nextValue = next === null ? "" : String(next);
          if (value === undefined) setOwnValue(nextValue);
          onChange?.(createSelectChangeEvent(nextValue, name ?? ""));
          onValueChange?.(nextValue);
        }}
      >
        <BaseSelect.Trigger
          ref={triggerRef}
          id={triggerId}
          className={cx(styles.trigger, styles[variant])}
          {...passThrough(rest)}
          aria-label={ariaLabel}
          aria-labelledby={ariaLabel ? undefined : labelId}
          aria-describedby={cx(describedBy, hintId) || undefined}
        >
          <BaseSelect.Value className={styles.value}>{labelOf}</BaseSelect.Value>
          <BaseSelect.Icon className={styles.icon}>
            <ChevronDownIcon />
          </BaseSelect.Icon>
        </BaseSelect.Trigger>
        <BaseSelect.Portal ref={setPortal} className={styles.portal} {...{ [LAYER_ATTRIBUTE]: "" }}>
          <BaseSelect.Positioner className={styles.positioner} sideOffset={4} collisionPadding={8}>
            <BaseSelect.Popup className={styles.popup}>
              <BaseSelect.ScrollUpArrow className={cx(styles.scrollArrow, styles.scrollUp)}>
                <ChevronUpIcon />
              </BaseSelect.ScrollUpArrow>
              {/* The listbox takes the field's name too (the label is a native `<label>`, not Select.Label). */}
              <BaseSelect.List
                className={styles.list}
                aria-label={ariaLabel}
                aria-labelledby={ariaLabel ? undefined : labelId}
              >
                {options.map((option) => (
                  <BaseSelect.Item
                    key={option.value}
                    value={option.value}
                    label={option.label}
                    disabled={option.disabled}
                    className={styles.item}
                  >
                    <BaseSelect.ItemIndicator className={styles.indicator} keepMounted>
                      <CheckIcon />
                    </BaseSelect.ItemIndicator>
                    <BaseSelect.ItemText className={styles.itemText}>{option.label}</BaseSelect.ItemText>
                  </BaseSelect.Item>
                ))}
              </BaseSelect.List>
              <BaseSelect.ScrollDownArrow className={cx(styles.scrollArrow, styles.scrollDown)}>
                <ChevronDownIcon />
              </BaseSelect.ScrollDownArrow>
            </BaseSelect.Popup>
          </BaseSelect.Positioner>
        </BaseSelect.Portal>
      </BaseSelect.Root>
      {hint ? (
        <p id={hintId} className={fieldStyles.hint}>
          {hint}
        </p>
      ) : null}
    </div>
  );
}
