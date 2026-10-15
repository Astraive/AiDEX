import type { ChangeEvent } from "react";

/** The fields of a native `<select>` that change handlers read. */
export type SelectEventTarget = { value: string; name: string };

/**
 * Builds the change event the Select passes to `onChange`, shaped like a native `<select>` change
 * event, so handlers written for `event.target.value` (and `currentTarget`, `name`) keep working.
 */
export function createSelectChangeEvent(value: string, name = ""): ChangeEvent<HTMLSelectElement> {
  const target: SelectEventTarget = { value, name };
  let defaultPrevented = false;
  let propagationStopped = false;
  const event = {
    type: "change",
    target,
    currentTarget: target,
    bubbles: true,
    cancelable: false,
    timeStamp: Date.now(),
    isTrusted: false,
    nativeEvent: new Event("change"),
    get defaultPrevented() {
      return defaultPrevented;
    },
    preventDefault() {
      defaultPrevented = true;
    },
    isDefaultPrevented() {
      return defaultPrevented;
    },
    stopPropagation() {
      propagationStopped = true;
    },
    isPropagationStopped() {
      return propagationStopped;
    },
    persist() {},
  };
  return event as unknown as ChangeEvent<HTMLSelectElement>;
}
