"use client";

import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type KeyboardEvent,
  type RefObject,
} from "react";
import { SearchIcon } from "../../icons/actions";
import { LoaderIcon, SlidersIcon } from "../../icons/interface";
import { cx } from "../../utils/cx";
import { ModelPickerList } from "./ModelPickerList";
import { ModelPickerRail, type ModelPickerRailHandle } from "./ModelPickerRail";
import type { ModelPickerModel, ModelPickerProvider } from "./types";
import {
  adjacentRailSelection,
  buildPickerView,
  initialExpandedLegacy,
  initialRailSelection,
  isRowEnabled,
  modelRowId,
  providerStatusMessage,
  type RailSelection,
} from "./view";
import styles from "./ModelPicker.module.css";

export type ModelPickerPanelProps = {
  providers: readonly ModelPickerProvider[];
  models: readonly ModelPickerModel[];
  value: string | null;
  onValueChange: (key: string) => void;
  favorites: ReadonlySet<string>;
  onToggleFavorite: (key: string) => void;
  onClose: () => void;
  searchRef: RefObject<HTMLInputElement | null>;
  onOpenProviderSetup?: (providerId: string) => void;
  onManageProviders?: () => void;
  emptyText?: string;
  loading?: boolean;
  jumpModifierLabel?: string;
};

const JUMP_CODE = /^(?:Digit|Numpad)([1-9])$/;

function jumpDigit(event: globalThis.KeyboardEvent): number | null {
  const fromCode = JUMP_CODE.exec(event.code)?.[1];
  const digit = fromCode ?? (/^[1-9]$/.test(event.key) ? event.key : undefined);
  return digit ? Number(digit) : null;
}

function sameMembers(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
  if (a.size !== b.size) return false;
  for (const entry of a) if (!b.has(entry)) return false;
  return true;
}

/** Picker body: provider rail, search, model listbox, status footer, and the manage bar. */
export function ModelPickerPanel({
  providers,
  models,
  value,
  onValueChange,
  favorites,
  onToggleFavorite,
  onClose,
  searchRef,
  onOpenProviderSetup,
  onManageProviders,
  emptyText,
  loading = false,
  jumpModifierLabel,
}: ModelPickerPanelProps) {
  const baseId = useId();
  const listId = `${baseId}-list`;
  const panelRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const railRef = useRef<ModelPickerRailHandle>(null);
  const [rail, setRail] = useState<RailSelection>(() =>
    initialRailSelection({ providers, models, favorites, value }),
  );
  const [query, setQuery] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [expandedLegacy, setExpandedLegacy] = useState<ReadonlySet<string>>(() =>
    initialExpandedLegacy(models, favorites, value),
  );
  // Height captured when a search starts, so results scroll instead of resizing the popover.
  const [frozenHeight, setFrozenHeight] = useState<number | null>(null);
  // Scroll the active option into view after keyboard moves and view changes, not after hovering.
  const scrollPendingRef = useRef(true);
  // Set once the user acts in the panel; until then the start view follows data that is still arriving.
  const viewTouchedRef = useRef(false);
  const touchView = () => {
    viewTouchedRef.current = true;
  };

  // The panel can open while the catalog, lazily listed runtime models, or favorites are still
  // loading. Re-derive the start view as they arrive, until the user has interacted.
  useEffect(() => {
    if (viewTouchedRef.current) return;
    const nextRail = initialRailSelection({ providers, models, favorites, value });
    if (nextRail !== rail) {
      setRail(nextRail);
      scrollPendingRef.current = true;
      if (listRef.current) listRef.current.scrollTop = 0;
    }
    const nextLegacy = initialExpandedLegacy(models, favorites, value);
    setExpandedLegacy((current) => (sameMembers(current, nextLegacy) ? current : nextLegacy));
  }, [providers, models, favorites, value, rail]);

  const view = useMemo(
    () => buildPickerView({ providers, models, favorites, value, query, rail, expandedLegacy }),
    [providers, models, favorites, value, query, rail, expandedLegacy],
  );
  const { rows, jumpKeys, searching } = view;
  const enabledIds = useMemo(() => rows.filter(isRowEnabled).map((row) => row.id), [rows]);
  const optionIds = useMemo(() => new Map(rows.map((row, index) => [row.id, `${baseId}-option-${index}`])), [
    rows,
    baseId,
  ]);
  const optionId = useCallback((rowId: string) => optionIds.get(rowId) ?? `${baseId}-option`, [optionIds, baseId]);

  // The highlighted option: the last hovered or arrowed one if still listed; otherwise the first
  // result while searching, or the selected model, or the first enabled row.
  const effectiveActiveId = useMemo(() => {
    if (activeId && enabledIds.includes(activeId)) return activeId;
    if (!searching && value && enabledIds.includes(modelRowId(value))) return modelRowId(value);
    return enabledIds[0] ?? null;
  }, [activeId, enabledIds, searching, value]);

  const showRail = !searching;
  const railProvider = rail === null ? undefined : providers.find((provider) => provider.id === rail);
  const railHasModels = rail !== null && models.some((model) => model.providerId === rail);
  const footerMessage =
    !searching && railProvider && !(loading && !railHasModels)
      ? providerStatusMessage(railProvider, railHasModels)
      : null;

  const listEmptyText = (() => {
    if (rows.length > 0) return null;
    if (loading) return "Loading models…";
    if (searching) return "No models found";
    if (rail === null) {
      return models.length === 0
        ? (emptyText ?? "No models are available.")
        : "No favorites yet. Use the star next to a model to add it here.";
    }
    return footerMessage ? null : (emptyText ?? "No models are available for this provider.");
  })();

  const focusSearch = useCallback(() => searchRef.current?.focus({ preventScroll: true }), [searchRef]);

  const selectModel = useCallback(
    (model: ModelPickerModel) => {
      if (model.disabledReason) return;
      onValueChange(model.key);
      onClose();
    },
    [onClose, onValueChange],
  );

  const selectRail = useCallback(
    (next: RailSelection) => {
      viewTouchedRef.current = true;
      setRail(next);
      setActiveId(null);
      scrollPendingRef.current = true;
      if (listRef.current) listRef.current.scrollTop = 0;
      requestAnimationFrame(focusSearch);
    },
    [focusSearch],
  );

  const toggleLegacy = useCallback((providerId: string) => {
    viewTouchedRef.current = true;
    setExpandedLegacy((current) => {
      const next = new Set(current);
      if (next.has(providerId)) next.delete(providerId);
      else next.add(providerId);
      return next;
    });
  }, []);

  const activateRow = (rowId: string | null) => {
    if (!rowId) return;
    const row = rows.find((entry) => entry.id === rowId);
    if (!row) return;
    if (row.type === "legacy") toggleLegacy(row.providerId);
    else selectModel(row.model);
  };

  const moveActive = (step: number) => {
    touchView();
    if (enabledIds.length === 0) return;
    const index = effectiveActiveId ? enabledIds.indexOf(effectiveActiveId) : -1;
    const next = index === -1 ? (step > 0 ? 0 : enabledIds.length - 1) : (index + step + enabledIds.length) % enabledIds.length;
    scrollPendingRef.current = true;
    setActiveId(enabledIds[next]);
  };

  useLayoutEffect(() => {
    // Stay pending while nothing is highlighted (e.g. still loading), so the row scrolls in once listed.
    if (!scrollPendingRef.current || !effectiveActiveId) return;
    scrollPendingRef.current = false;
    const element = document.getElementById(optionId(effectiveActiveId));
    element?.scrollIntoView({ block: "nearest" });
  }, [effectiveActiveId, optionId]);

  // Global keys while open, in the capture phase so they work wherever focus is inside the app.
  const latest = useRef({ jumpKeys, models, providers, rail, selectModel, selectRail });
  useLayoutEffect(() => {
    latest.current = { jumpKeys, models, providers, rail, selectModel, selectRail };
  });
  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if (event.defaultPrevented || event.isComposing || event.altKey) return;
      if (!(event.metaKey || event.ctrlKey)) return;
      const current = latest.current;
      if (event.shiftKey && (event.key === "ArrowUp" || event.key === "ArrowDown")) {
        event.preventDefault();
        event.stopPropagation();
        setQuery("");
        setFrozenHeight(null);
        current.selectRail(adjacentRailSelection(current.providers, current.rail, event.key === "ArrowDown" ? 1 : -1));
        return;
      }
      if (event.shiftKey || event.repeat) return;
      const digit = jumpDigit(event);
      if (digit === null) return;
      const key = current.jumpKeys[digit - 1];
      const model = key === undefined ? undefined : current.models.find((entry) => entry.key === key);
      if (!model) return;
      event.preventDefault();
      event.stopPropagation();
      current.selectModel(model);
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, []);

  const handleQueryChange = (event: ChangeEvent<HTMLInputElement>) => {
    const next = event.target.value;
    const nextSearching = next.trim().length > 0;
    if (nextSearching) touchView();
    if (nextSearching && !searching) setFrozenHeight(panelRef.current?.offsetHeight ?? null);
    if (!nextSearching) setFrozenHeight(null);
    setQuery(next);
    setActiveId(null);
    scrollPendingRef.current = true;
    if (listRef.current) listRef.current.scrollTop = 0;
  };

  const handleSearchKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    const modified = event.altKey || event.ctrlKey || event.metaKey;
    if (!modified) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        moveActive(event.key === "ArrowDown" ? 1 : -1);
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        activateRow(effectiveActiveId);
        return;
      }
      const toRail =
        showRail &&
        ((event.key === "ArrowLeft" && !event.shiftKey && query.length === 0) || (event.key === "Tab" && event.shiftKey));
      if (toRail) {
        event.preventDefault();
        touchView();
        railRef.current?.focus();
        return;
      }
      // Keep plain typing keys from reaching app-level shortcuts; modified chords still pass through.
      event.stopPropagation();
    }
  };

  const handleToggleFavorite = (key: string, removesRow: boolean) => {
    // Starring must not move the view to Favorites.
    touchView();
    onToggleFavorite(key);
    // The row (and its focused star) disappears from the Favorites view; keep focus usable.
    if (removesRow) focusSearch();
  };

  const openSetup = (providerId: string) => {
    onClose();
    onOpenProviderSetup?.(providerId);
  };

  const resultsText = searching ? `${rows.length} ${rows.length === 1 ? "model" : "models"} found` : "";

  return (
    <div
      ref={panelRef}
      className={styles.panel}
      style={frozenHeight !== null ? { height: frozenHeight } : undefined}
      data-model-picker-content=""
      onMouseDown={(event) => {
        // Clicks on non-interactive parts keep focus in the search field.
        const target = event.target as HTMLElement;
        if (!target.closest("button, input, a[href], [tabindex]:not([tabindex='-1'])")) event.preventDefault();
      }}
    >
      <div className={styles.body}>
        {showRail ? (
          <ModelPickerRail
            ref={railRef}
            providers={providers}
            selected={rail}
            onSelect={selectRail}
            onFocusSearch={focusSearch}
          />
        ) : null}
        <div className={cx(styles.main, showRail && styles.withRail)}>
          <div className={styles.search}>
            <SearchIcon size={16} className={styles.searchIcon} />
            <input
              ref={searchRef}
              type="text"
              role="combobox"
              aria-label="Search models"
              aria-expanded="true"
              aria-controls={listId}
              aria-autocomplete="list"
              aria-activedescendant={effectiveActiveId ? optionId(effectiveActiveId) : undefined}
              placeholder="Search models…"
              autoComplete="off"
              autoCorrect="off"
              spellCheck={false}
              className={styles.searchInput}
              value={query}
              onChange={handleQueryChange}
              onKeyDown={handleSearchKeyDown}
            />
            {loading ? <LoaderIcon size={16} className={styles.spinner} /> : null}
          </div>
          <ModelPickerList
            id={listId}
            listRef={listRef}
            rows={rows}
            activeId={effectiveActiveId}
            optionId={optionId}
            value={value}
            jumpKeys={jumpKeys}
            jumpModifierLabel={jumpModifierLabel}
            emptyText={listEmptyText}
            favoritesView={!searching && rail === null}
            onActivate={(rowId) => {
              touchView();
              setActiveId(rowId);
            }}
            onSelectModel={selectModel}
            onToggleLegacy={toggleLegacy}
            onToggleFavorite={handleToggleFavorite}
          />
          {footerMessage && railProvider ? (
            <div className={styles.footer}>
              <p className={styles.footerText}>{footerMessage}</p>
              {onOpenProviderSetup ? (
                <button type="button" className={styles.inlineButton} onClick={() => openSetup(railProvider.id)}>
                  {railProvider.setupLabel ?? "Open provider setup"}
                </button>
              ) : null}
            </div>
          ) : null}
        </div>
      </div>
      {onManageProviders ? (
        <div className={styles.bar}>
          <button
            type="button"
            className={styles.barButton}
            onClick={() => {
              onClose();
              onManageProviders();
            }}
          >
            <SlidersIcon size={16} />
            Manage providers
          </button>
        </div>
      ) : null}
      <div className={styles.visuallyHidden} role="status" aria-live="polite">
        {loading ? "Loading models" : resultsText}
      </div>
    </div>
  );
}
