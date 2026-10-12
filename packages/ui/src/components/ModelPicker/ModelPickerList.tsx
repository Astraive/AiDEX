"use client";

import { useRef, type MouseEvent, type Ref } from "react";
import { BrandIcon } from "../../icons/brands/BrandIcon";
import { CheckIcon, ChevronRightIcon, StarFilledIcon, StarIcon } from "../../icons/interface";
import { Badge, type BadgeTone } from "../../primitives/Badge";
import { Kbd } from "../../primitives/Kbd";
import { cx } from "../../utils/cx";
import type { ModelPickerModel } from "./types";
import {
  jumpLabel,
  modelBadges,
  modelDetailLine,
  modelDisplayName,
  type LegacyRow,
  type ModelRow,
  type PickerRow,
} from "./view";
import styles from "./ModelPickerList.module.css";

const BADGE_TONE: Record<string, BadgeTone> = {
  New: "accent",
  Default: "neutral",
  Custom: "neutral",
  Unavailable: "warning",
};

type ModelPickerListProps = {
  id: string;
  rows: readonly PickerRow[];
  /** Row id of the active (highlighted) option. */
  activeId: string | null;
  /** DOM id of each row's option element. */
  optionId: (rowId: string) => string;
  value: string | null;
  jumpKeys: readonly string[];
  jumpModifierLabel?: string;
  /** Shown instead of options when there are none; omit to render nothing. */
  emptyText?: string | null;
  /** Favorites view: un-starring removes the row, so keyboard focus moves back to search. */
  favoritesView: boolean;
  onActivate: (rowId: string) => void;
  onSelectModel: (model: ModelPickerModel) => void;
  onToggleLegacy: (providerId: string) => void;
  onToggleFavorite: (key: string, removesRow: boolean) => void;
  listRef?: Ref<HTMLDivElement>;
};

// Options keep focus in the search field (aria-activedescendant), so pointer presses must not move it.
const keepFocus = (event: MouseEvent) => event.preventDefault();

/** The model listbox: model options, the legacy expander, or an empty message. */
export function ModelPickerList({
  id,
  rows,
  activeId,
  optionId,
  value,
  jumpKeys,
  jumpModifierLabel,
  emptyText,
  favoritesView,
  onActivate,
  onSelectModel,
  onToggleLegacy,
  onToggleFavorite,
  listRef,
}: ModelPickerListProps) {
  const empty = rows.length === 0;
  // Browsers can replay a mouse move when the list scrolls under a resting pointer; only real
  // pointer movement may take the active option away from the keyboard.
  const pointerRef = useRef<{ x: number; y: number } | null>(null);
  const handleHover = (event: MouseEvent, rowId: string | null) => {
    const last = pointerRef.current;
    if (last && last.x === event.clientX && last.y === event.clientY) return;
    pointerRef.current = { x: event.clientX, y: event.clientY };
    // Disabled rows (null) are skipped like they are for the keyboard.
    if (rowId !== null && rowId !== activeId) onActivate(rowId);
  };
  return (
    <>
      <div
        ref={listRef}
        id={id}
        role="listbox"
        aria-label="Models"
        className={cx(styles.list, empty && styles.listEmpty)}
      >
        {rows.map((row) =>
          row.type === "legacy" ? (
            <LegacyOption
              key={row.id}
              row={row}
              domId={optionId(row.id)}
              active={row.id === activeId}
              onHover={handleHover}
              onToggle={onToggleLegacy}
            />
          ) : (
            <ModelOption
              key={row.id}
              row={row}
              domId={optionId(row.id)}
              active={row.id === activeId}
              selected={row.model.key === value}
              jumpIndex={jumpKeys.indexOf(row.model.key)}
              jumpModifierLabel={jumpModifierLabel}
              favoritesView={favoritesView}
              onHover={handleHover}
              onSelect={onSelectModel}
              onToggleFavorite={onToggleFavorite}
            />
          ),
        )}
      </div>
      {empty && emptyText ? <p className={styles.empty}>{emptyText}</p> : null}
    </>
  );
}

function ModelOption({
  row,
  domId,
  active,
  selected,
  jumpIndex,
  jumpModifierLabel,
  favoritesView,
  onHover,
  onSelect,
  onToggleFavorite,
}: {
  row: ModelRow;
  domId: string;
  active: boolean;
  selected: boolean;
  jumpIndex: number;
  jumpModifierLabel?: string;
  favoritesView: boolean;
  onHover: (event: MouseEvent, rowId: string | null) => void;
  onSelect: (model: ModelPickerModel) => void;
  onToggleFavorite: (key: string, removesRow: boolean) => void;
}) {
  const { model, provider, favorite, disabled } = row;
  const providerName = provider?.name ?? model.providerId;
  const name = modelDisplayName(model);
  const badges = modelBadges(model);
  const detail = modelDetailLine(model, providerName);
  const jump = jumpIndex >= 0 && jumpModifierLabel ? jumpLabel(jumpModifierLabel, jumpIndex) : null;
  const shortcut =
    jumpIndex >= 0 && jumpModifierLabel ? `${jumpModifierLabel === "⌘" ? "Meta" : "Control"}+${jumpIndex + 1}` : undefined;
  const accessibleName = [
    name,
    ...badges,
    detail,
    favorite ? "favorite" : null,
    disabled && model.disabledReason ? `not available: ${model.disabledReason}` : null,
  ]
    .filter(Boolean)
    .join(", ");
  const tooltip = disabled
    ? (model.disabledReason ?? undefined)
    : [model.name, model.description].filter(Boolean).join(" — ");
  const favoriteLabel = favorite ? "Remove from favorites" : "Add to favorites";

  return (
    <div
      id={domId}
      role="option"
      aria-selected={selected}
      aria-disabled={disabled || undefined}
      aria-label={accessibleName}
      aria-keyshortcuts={shortcut}
      title={tooltip}
      className={cx(styles.option, active && styles.active, selected && styles.selected, disabled && styles.disabled)}
      onMouseDown={keepFocus}
      onMouseMove={(event) => {
        onHover(event, disabled ? null : row.id);
      }}
      onClick={() => {
        if (!disabled) onSelect(model);
      }}
    >
      <span className={styles.main}>
        <span className={styles.nameLine}>
          <span className={styles.name}>{name}</span>
          {badges.map((badge) => (
            <Badge key={badge} tone={BADGE_TONE[badge] ?? "neutral"} size="sm" className={styles.badge}>
              {badge}
            </Badge>
          ))}
        </span>
        <span className={styles.detailLine}>
          {provider ? (
            <BrandIcon
              brand={provider.brand}
              monogramLabel={provider.name}
              size={12}
              className={styles.detailIcon}
              aria-hidden
            />
          ) : null}
          <span className={styles.detail}>{detail}</span>
        </span>
      </span>
      <span className={styles.trailing}>
        {selected ? <CheckIcon size={16} className={styles.check} /> : null}
        {jump ? (
          <Kbd aria-hidden="true" className={styles.jump}>
            {jump}
          </Kbd>
        ) : null}
        <button
          type="button"
          className={cx(styles.star, favorite && styles.starred)}
          aria-label={favoriteLabel}
          title={favoriteLabel}
          // Only the active row's star is in the tab order: Tab from search reaches it, Shift+Tab returns.
          tabIndex={active ? 0 : -1}
          disabled={disabled}
          onMouseDown={keepFocus}
          onClick={(event) => {
            event.stopPropagation();
            onToggleFavorite(model.key, favoritesView && favorite);
          }}
        >
          {favorite ? <StarFilledIcon size={16} /> : <StarIcon size={16} />}
        </button>
      </span>
    </div>
  );
}

function LegacyOption({
  row,
  domId,
  active,
  onHover,
  onToggle,
}: {
  row: LegacyRow;
  domId: string;
  active: boolean;
  onHover: (event: MouseEvent, rowId: string | null) => void;
  onToggle: (providerId: string) => void;
}) {
  const count = `${row.count} ${row.count === 1 ? "model" : "models"}`;
  return (
    <div
      id={domId}
      role="option"
      aria-selected={false}
      aria-label={`Legacy models, ${count}, ${row.expanded ? "expanded" : "collapsed"}`}
      className={cx(styles.option, styles.legacy, active && styles.active)}
      onMouseDown={keepFocus}
      onMouseMove={(event) => {
        onHover(event, row.id);
      }}
      onClick={() => onToggle(row.providerId)}
    >
      <span className={styles.main}>
        <span className={styles.name}>Legacy models</span>
        <span className={styles.detail}>{count}</span>
      </span>
      <ChevronRightIcon size={16} className={cx(styles.chevron, row.expanded && styles.expanded)} />
    </div>
  );
}
