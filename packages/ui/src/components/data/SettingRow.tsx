"use client";

import { useRef, type ComponentPropsWithRef, type MouseEvent, type ReactNode } from "react";
import { RotateCcwIcon } from "../../icons/actions";
import { IconButton } from "../../primitives/IconButton";
import { cx } from "../../utils/cx";
import styles from "./SettingRow.module.css";

export type SettingRowReset = {
  /** Accessible name and tooltip, e.g. "Reset density". */
  label: string;
  /** Restores the default. Focus then moves to the control, since the button usually disappears. */
  onReset: () => void;
};

const FOCUSABLE = "button, a[href], input, select, textarea, [tabindex]";

/**
 * Focuses the row's control: the element named by `controlId` when it takes focus, otherwise the
 * first tab stop in the control cell other than the reset button. Returns whether focus moved.
 */
function focusControl(cell: HTMLElement | null, controlId: string | undefined, resetButton: Element | null): boolean {
  if (!cell?.isConnected) return false;
  const named = controlId ? document.getElementById(controlId) : null;
  const stops = Array.from(cell.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (element) =>
      !resetButton?.contains(element) &&
      element.tabIndex >= 0 &&
      !element.matches(":disabled") &&
      element.getClientRects().length > 0,
  );
  for (const target of named ? [named, ...stops] : stops) {
    target.focus();
    if (document.activeElement === target) return true;
  }
  return false;
}

export type SettingRowProps = Omit<ComponentPropsWithRef<"div">, "title" | "children"> & {
  title: ReactNode;
  description?: ReactNode;
  /** The setting's control, e.g. a `Switch`, `Select`, or `Button`. */
  control: ReactNode;
  /**
   * Id of the control. When set, the title becomes its `<label>` and the description gets the id
   * `${controlId}-description` for the control's `aria-describedby`. Without it, label the control yourself.
   */
  controlId?: string;
  /**
   * Shows a reset button before the control; pass it only while the value differs from its default.
   * After a reset, focus moves to the control so it is not lost when the button unmounts.
   */
  reset?: SettingRowReset;
};

/**
 * Settings line: title and description on the left, the control on the right. In a row narrower
 * than 512px the control moves under the text and takes the full width.
 */
export function SettingRow({ title, description, control, controlId, reset, className, ...props }: SettingRowProps) {
  const controlRef = useRef<HTMLDivElement>(null);

  const handleReset = (event: MouseEvent<HTMLButtonElement>) => {
    const resetButton = event.currentTarget;
    reset?.onReset();
    // The control is still mounted here, so focus moves before the button unmounts. If the control
    // re-renders as a new element, focus falls to the body, and a roving group (radios, segments)
    // can leave it on a stop that is no longer the tab stop; retry once after that commit.
    focusControl(controlRef.current, controlId, resetButton);
    requestAnimationFrame(() => {
      const current = document.activeElement as HTMLElement | null;
      const stale = !!current && !!controlRef.current?.contains(current) && current.tabIndex < 0;
      if (current === null || current === document.body || stale) {
        focusControl(controlRef.current, controlId, resetButton);
      }
    });
  };

  return (
    <div className={cx(styles.row, className)} {...props}>
      <div className={styles.layout}>
        <div className={styles.text}>
          {controlId ? (
            <label className={styles.title} htmlFor={controlId}>
              {title}
            </label>
          ) : (
            <span className={styles.title}>{title}</span>
          )}
          {description ? (
            <p id={controlId ? `${controlId}-description` : undefined} className={styles.description}>
              {description}
            </p>
          ) : null}
        </div>
        <div ref={controlRef} className={styles.control}>
          {reset ? (
            <IconButton
              className={styles.reset}
              variant="ghost"
              size="sm"
              label={reset.label}
              icon={<RotateCcwIcon />}
              onClick={handleReset}
            />
          ) : null}
          {control}
        </div>
      </div>
    </div>
  );
}
