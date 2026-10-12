"use client";

import {
  Fragment,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type Ref,
} from "react";
import { BrandIcon } from "../../icons/brands/BrandIcon";
import { StarFilledIcon } from "../../icons/interface";
import { StatusDot } from "../../primitives/StatusDot";
import { cx } from "../../utils/cx";
import type { ModelPickerProvider } from "./types";
import { isProviderSelectable, providerRailLabel, providerStatusTone, type RailSelection } from "./view";
import styles from "./ModelPickerRail.module.css";

export type ModelPickerRailHandle = {
  /** Focuses the selected rail button (or the first enabled one). */
  focus: () => void;
};

type RailItem = { selection: RailSelection; provider?: ModelPickerProvider; enabled: boolean };

type ModelPickerRailProps = {
  providers: readonly ModelPickerProvider[];
  selected: RailSelection;
  onSelect: (selection: RailSelection) => void;
  /** Moves focus to the search field (ArrowRight, or typing a character). */
  onFocusSearch: () => void;
  ref?: Ref<ModelPickerRailHandle>;
};

const INDICATOR_HALF_HEIGHT = 10;

/** Vertical provider toolbar: Favorites, then one button per provider, with roving focus. */
export function ModelPickerRail({ providers, selected, onSelect, onFocusSearch, ref }: ModelPickerRailProps) {
  const items: RailItem[] = [
    { selection: null, enabled: true },
    ...providers.map((provider) => ({ selection: provider.id, provider, enabled: isProviderSelectable(provider) })),
  ];
  const selectedIndex = Math.max(
    0,
    items.findIndex((item) => item.selection === selected),
  );
  const [focusIndex, setFocusIndex] = useState(selectedIndex);
  const buttonRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const contentRef = useRef<HTMLDivElement>(null);
  const [indicatorTop, setIndicatorTop] = useState<number | null>(null);

  // Keep the tab stop on the selected entry whenever the selection changes.
  useLayoutEffect(() => setFocusIndex(selectedIndex), [selectedIndex]);

  useLayoutEffect(() => {
    const button = buttonRefs.current[selectedIndex];
    const wrapper = button?.parentElement;
    setIndicatorTop(wrapper ? wrapper.offsetTop + wrapper.offsetHeight / 2 - INDICATOR_HALF_HEIGHT : null);
  }, [selectedIndex, providers]);

  const focusAt = (index: number) => {
    const wrapped = (index + items.length) % items.length;
    setFocusIndex(wrapped);
    buttonRefs.current[wrapped]?.focus();
  };

  useImperativeHandle(
    ref,
    () => ({
      focus: () => {
        const index = items[selectedIndex]?.enabled ? selectedIndex : items.findIndex((item) => item.enabled);
        focusAt(Math.max(0, index));
      },
    }),
  );

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const index = buttonRefs.current.findIndex((button) => button === event.target);
    if (index === -1) return;
    if (event.key === "ArrowDown") focusAt(index + 1);
    else if (event.key === "ArrowUp") focusAt(index - 1);
    else if (event.key === "Home") focusAt(0);
    else if (event.key === "End") focusAt(items.length - 1);
    else if (event.key === "ArrowRight" && !event.shiftKey) onFocusSearch();
    else if (event.key.length === 1 && event.key !== " ") {
      // Type-to-search: move focus first so the character lands in the search field.
      onFocusSearch();
      return;
    } else return;
    event.preventDefault();
  };

  let previousKind: ModelPickerProvider["kind"] | undefined;
  return (
    <div
      role="toolbar"
      aria-orientation="vertical"
      aria-label="Providers"
      className={styles.rail}
      onKeyDown={handleKeyDown}
    >
      <div ref={contentRef} className={styles.content}>
        {indicatorTop !== null ? (
          <span className={styles.indicator} style={{ top: indicatorTop }} aria-hidden="true" />
        ) : null}
        {items.map((item, index) => {
          const isSelected = item.selection === selected;
          const provider = item.provider;
          const label = provider ? providerRailLabel(provider) : "Favorites";
          // Runtimes form their own group after model providers.
          const startsRuntimeGroup =
            provider !== undefined && provider.kind === "runtime" && previousKind !== undefined && previousKind !== "runtime";
          if (provider) previousKind = provider.kind ?? "model";
          return (
            <Fragment key={item.selection ?? "\u0000favorites"}>
              {startsRuntimeGroup ? <span className={styles.divider} aria-hidden="true" /> : null}
              <div className={styles.item}>
                <button
                  ref={(node) => {
                    buttonRefs.current[index] = node;
                  }}
                  type="button"
                  className={cx(styles.button, isSelected && styles.selected, !item.enabled && styles.disabled)}
                  aria-label={label}
                  aria-pressed={isSelected}
                  aria-disabled={item.enabled ? undefined : true}
                  title={label}
                  tabIndex={index === focusIndex ? 0 : -1}
                  onFocus={() => setFocusIndex(index)}
                  onClick={() => {
                    if (item.enabled) onSelect(item.selection);
                  }}
                >
                  {provider ? (
                    <span className={styles.glyph}>
                      <BrandIcon
                        brand={provider.brand}
                        monogramLabel={provider.name}
                        size={20}
                        aria-hidden
                      />
                      <StatusDot tone={providerStatusTone(provider.status)} size="sm" className={styles.status} />
                    </span>
                  ) : (
                    <StarFilledIcon size={20} className={styles.favorites} />
                  )}
                </button>
              </div>
              {item.selection === null && providers.length > 0 ? (
                <span className={styles.divider} aria-hidden="true" />
              ) : null}
            </Fragment>
          );
        })}
      </div>
    </div>
  );
}
