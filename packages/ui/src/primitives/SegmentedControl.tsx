"use client";

import { useRef, type KeyboardEvent } from "react";
import { cx } from "../utils/cx";
import { segmentedSelection } from "./segmentedSelection";
import styles from "./SegmentedControl.module.css";

export type SegmentedOption<Value extends string> = { value: Value; label: string };

export type SegmentedControlProps<Value extends string> = {
  /** Accessible name for the group. */
  label: string;
  options: readonly SegmentedOption<Value>[];
  /** The checked option; a value outside `options` checks none of them. */
  value: Value;
  onValueChange: (value: Value) => void;
  /** `sm` for compact filter bars. */
  size?: "sm" | "md";
  /** Disables every option: the group is skipped by Tab and ignores clicks and arrow keys. */
  disabled?: boolean;
  className?: string;
};

const STEP_BY_KEY: Partial<Record<string, number>> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };

/** Single-choice switch with radio-group semantics and roving keyboard focus. */
export function SegmentedControl<Value extends string>({
  label,
  options,
  value,
  onValueChange,
  size = "md",
  disabled = false,
  className,
}: SegmentedControlProps<Value>) {
  const optionRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const { checkedIndex, tabStopIndex } = segmentedSelection(options, value);

  const selectAt = (index: number) => {
    if (options.length === 0) return;
    const wrapped = (index + options.length) % options.length;
    onValueChange(options[wrapped].value);
    optionRefs.current[wrapped]?.focus();
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    // Leave modified chords (e.g. Alt+Arrow history navigation) to the browser.
    if (disabled || event.altKey || event.ctrlKey || event.metaKey) return;
    const step = STEP_BY_KEY[event.key];
    if (step !== undefined) selectAt(index + step);
    else if (event.key === "Home") selectAt(0);
    else if (event.key === "End") selectAt(options.length - 1);
    else return;
    event.preventDefault();
  };

  return (
    <div
      role="radiogroup"
      aria-label={label}
      aria-disabled={disabled || undefined}
      className={cx(styles.group, styles[size], disabled && styles.disabled, className)}
    >
      {options.map((option, index) => {
        const checked = index === checkedIndex;
        return (
          <button
            key={option.value}
            ref={(node) => {
              optionRefs.current[index] = node;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            disabled={disabled}
            tabIndex={index === tabStopIndex ? 0 : -1}
            className={cx(styles.option, checked && styles.checked)}
            onClick={() => {
              if (!disabled) onValueChange(option.value);
            }}
            onKeyDown={(event) => handleKeyDown(event, index)}
          >
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
