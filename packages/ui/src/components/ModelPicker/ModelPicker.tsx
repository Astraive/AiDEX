"use client";

import { useRef, type RefObject } from "react";
import { Popover, type PopoverPlacement } from "../../primitives/Popover";
import { ModelPickerPanel } from "./ModelPickerPanel";
import type { ModelPickerModel, ModelPickerProvider } from "./types";
import styles from "./ModelPicker.module.css";

export type ModelPickerProps = {
  providers: readonly ModelPickerProvider[];
  models: readonly ModelPickerModel[];
  /** Key of the selected model. */
  value: string | null;
  /** Called with the chosen model's key; the picker then closes. */
  onValueChange: (key: string) => void;
  favorites: ReadonlySet<string>;
  onToggleFavorite: (key: string) => void;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Usually the `ModelPickerTrigger`; focus returns to it when the picker closes. */
  anchorRef: RefObject<HTMLElement | null>;
  /** Accessible name of the popover (default "Choose a model"). */
  label?: string;
  /** Shows "Open provider setup" in the footer for providers that are not ready. */
  onOpenProviderSetup?: (providerId: string) => void;
  /** Shows a "Manage providers" bar at the bottom. */
  onManageProviders?: () => void;
  /** Message when no models exist at all. */
  emptyText?: string;
  loading?: boolean;
  /** Modifier label for jump hints, "⌘" or "Ctrl"; hints are hidden without it (Mod+1…9 still work). */
  jumpModifierLabel?: string;
  /** Preferred side (default `bottom-start`); the popover flips above the trigger when there is more room there. */
  placement?: PopoverPlacement;
  /** Portal target; pass the open `<dialog>` when the picker is used inside a modal. */
  container?: HTMLElement | null;
  /** Id of the popover, for the trigger's `aria-controls`. */
  id?: string;
};

/**
 * Provider-grouped model picker: a provider rail with Favorites, a search field with tokenised
 * ranking, and a keyboard-driven model list. Data only; the caller supplies providers, models, and
 * favorites and persists changes.
 */
export function ModelPicker({
  open,
  onOpenChange,
  anchorRef,
  label = "Choose a model",
  placement = "bottom-start",
  container,
  id,
  ...panelProps
}: ModelPickerProps) {
  const searchRef = useRef<HTMLInputElement>(null);
  return (
    <Popover
      open={open}
      onOpenChange={onOpenChange}
      anchorRef={anchorRef}
      placement={placement}
      label={label}
      role="dialog"
      initialFocusRef={searchRef}
      container={container}
      id={id}
      className={styles.popover}
    >
      <ModelPickerPanel {...panelProps} searchRef={searchRef} onClose={() => onOpenChange(false)} />
    </Popover>
  );
}
