// Pure view derivation for the model picker: which rows show for the current rail selection or
// search, in what order, and the copy for provider states. No React or DOM here.
import type { StatusDotTone } from "../../primitives/StatusDot";
import { rankBySearch, tokenizeSearchQuery } from "./search";
import type { ModelPickerCapability, ModelPickerModel, ModelPickerProvider, ModelPickerProviderStatus } from "./types";

/** Rail selection: a provider id, or `null` for the Favorites view. */
export type RailSelection = string | null;

export type ModelRow = {
  type: "model";
  /** Item key, unique among rows. */
  id: string;
  model: ModelPickerModel;
  provider: ModelPickerProvider | undefined;
  favorite: boolean;
  disabled: boolean;
};

export type LegacyRow = {
  type: "legacy";
  id: string;
  providerId: string;
  count: number;
  expanded: boolean;
};

export type PickerRow = ModelRow | LegacyRow;

export type PickerView = {
  rows: PickerRow[];
  /** Keys of the models reachable with Mod+1…Mod+9, in order. */
  jumpKeys: string[];
  searching: boolean;
};

export const JUMP_LIMIT = 9;

export const modelRowId = (key: string) => `model:${key}`;
export const legacyRowId = (providerId: string) => `legacy:${providerId}`;

export function isProviderSelectable(provider: ModelPickerProvider): boolean {
  return provider.status !== "disabled";
}

export function isRowEnabled(row: PickerRow): boolean {
  return row.type === "legacy" || !row.disabled;
}

/** Models that can appear at all: those of known, enabled providers, plus the current selection. */
function eligibleModels(
  models: readonly ModelPickerModel[],
  providerById: ReadonlyMap<string, ModelPickerProvider>,
  value: string | null,
): ModelPickerModel[] {
  return models.filter((model) => {
    if (model.key === value) return true;
    const provider = providerById.get(model.providerId);
    return provider !== undefined && isProviderSelectable(provider);
  });
}

function toModelRow(
  model: ModelPickerModel,
  providerById: ReadonlyMap<string, ModelPickerProvider>,
  favorites: ReadonlySet<string>,
): ModelRow {
  return {
    type: "model",
    id: modelRowId(model.key),
    model,
    provider: providerById.get(model.providerId),
    favorite: favorites.has(model.key),
    disabled: Boolean(model.disabledReason),
  };
}

/**
 * Initial rail selection: Favorites when any listed model is a favorite, otherwise the selected
 * model's provider, otherwise the first ready provider, otherwise the first selectable one.
 */
export function initialRailSelection(input: {
  providers: readonly ModelPickerProvider[];
  models: readonly ModelPickerModel[];
  favorites: ReadonlySet<string>;
  value: string | null;
}): RailSelection {
  const { providers, models, favorites, value } = input;
  const providerById = new Map(providers.map((provider) => [provider.id, provider]));
  const eligible = eligibleModels(models, providerById, value);
  if (eligible.some((model) => favorites.has(model.key))) return null;
  const selectedProvider = providerById.get(models.find((model) => model.key === value)?.providerId ?? "");
  if (selectedProvider && isProviderSelectable(selectedProvider)) return selectedProvider.id;
  const ready = providers.find((provider) => provider.status === "ready");
  if (ready) return ready.id;
  return providers.find(isProviderSelectable)?.id ?? null;
}

/** Next or previous rail entry (Favorites first), skipping disabled providers and wrapping around. */
export function adjacentRailSelection(
  providers: readonly ModelPickerProvider[],
  current: RailSelection,
  direction: 1 | -1,
): RailSelection {
  const sequence: RailSelection[] = [null, ...providers.filter(isProviderSelectable).map((provider) => provider.id)];
  const index = sequence.indexOf(current);
  if (index === -1) return direction === 1 ? sequence[0] : sequence[sequence.length - 1];
  return sequence[(index + direction + sequence.length) % sequence.length];
}

/** Builds the rows for a search, the Favorites view, or one provider's view. */
export function buildPickerView(input: {
  providers: readonly ModelPickerProvider[];
  models: readonly ModelPickerModel[];
  favorites: ReadonlySet<string>;
  value: string | null;
  query: string;
  rail: RailSelection;
  expandedLegacy: ReadonlySet<string>;
}): PickerView {
  const { providers, models, favorites, value, query, rail, expandedLegacy } = input;
  const providerById = new Map(providers.map((provider) => [provider.id, provider]));
  const eligible = eligibleModels(models, providerById, value);
  const row = (model: ModelPickerModel) => toModelRow(model, providerById, favorites);
  const searching = tokenizeSearchQuery(query).length > 0;
  let rows: PickerRow[];

  if (searching) {
    // A search ignores the rail and spans every provider.
    rows = rankBySearch(eligible, query, (model) => ({
      name: model.name,
      shortName: model.shortName,
      providerName: providerById.get(model.providerId)?.name ?? model.providerId,
      key: model.key,
      isFavorite: favorites.has(model.key),
    })).map(row);
  } else if (rail === null) {
    // Favorites, ordered by the rail order of their providers, then by catalog order.
    const providerOrder = new Map(providers.map((provider, index) => [provider.id, index]));
    rows = eligible
      .map((model, index) => ({ model, index }))
      .filter(({ model }) => favorites.has(model.key))
      .sort(
        (left, right) =>
          (providerOrder.get(left.model.providerId) ?? Number.MAX_SAFE_INTEGER) -
            (providerOrder.get(right.model.providerId) ?? Number.MAX_SAFE_INTEGER) || left.index - right.index,
      )
      .map(({ model }) => row(model));
  } else {
    // One provider: favorites first, then current models, then legacy ones behind an expander row.
    const own = eligible.filter((model) => model.providerId === rail);
    const favoriteModels = own.filter((model) => favorites.has(model.key));
    const current = own.filter((model) => !favorites.has(model.key) && !model.legacy);
    const legacy = own.filter((model) => !favorites.has(model.key) && model.legacy);
    rows = [...favoriteModels, ...current].map(row);
    if (legacy.length > 0) {
      const expanded = expandedLegacy.has(rail);
      rows.push({ type: "legacy", id: legacyRowId(rail), providerId: rail, count: legacy.length, expanded });
      if (expanded) rows.push(...legacy.map(row));
    }
  }

  const jumpKeys: string[] = [];
  for (const entry of rows) {
    if (jumpKeys.length >= JUMP_LIMIT) break;
    if (entry.type === "model" && !entry.disabled) jumpKeys.push(entry.model.key);
  }
  return { rows, jumpKeys, searching };
}

/** Legacy sections that start expanded: the selected model's provider when that model is legacy. */
export function initialExpandedLegacy(
  models: readonly ModelPickerModel[],
  favorites: ReadonlySet<string>,
  value: string | null,
): Set<string> {
  const selected = models.find((model) => model.key === value);
  return selected?.legacy && !favorites.has(selected.key) ? new Set([selected.providerId]) : new Set();
}

/** Rail tooltip and accessible name, naming the status whenever the provider is not ready. */
export function providerRailLabel(provider: ModelPickerProvider): string {
  const detail = provider.statusText?.trim();
  const withDetail = (state: string) => (detail ? `${provider.name} — ${state}. ${detail}` : `${provider.name} — ${state}.`);
  switch (provider.status) {
    case "ready":
      return provider.name;
    case "disabled":
      return `${provider.name} — Disabled in settings.`;
    case "warning":
      return withDetail("Limited");
    case "error":
      return withDetail("Unavailable");
    case "setup":
      return withDetail("Not ready");
  }
}

/**
 * Footer message for the provider selected in the rail, or null when it is ready and has models.
 */
export function providerStatusMessage(provider: ModelPickerProvider, hasModels: boolean): string | null {
  if (provider.status === "disabled") return null;
  if (provider.status === "ready") return hasModels ? null : "No models are available for this provider.";
  const detail = provider.statusText?.trim();
  if (detail) return detail;
  switch (provider.status) {
    case "setup":
      return "Open provider setup to sign in.";
    case "error":
      return `${provider.name} provider is unavailable.`;
    case "warning":
      return `${provider.name} provider has limited availability.`;
  }
}

const STATUS_TONE: Record<ModelPickerProviderStatus, StatusDotTone> = {
  ready: "success",
  warning: "warning",
  setup: "warning",
  error: "danger",
  disabled: "neutral",
};

export function providerStatusTone(status: ModelPickerProviderStatus): StatusDotTone {
  return STATUS_TONE[status];
}

/** Compact context size: 8192 → "8K", 200000 → "200K", 1048576 → "1M". */
export function formatContextWindow(tokens: number | null | undefined): string | null {
  if (tokens == null || !Number.isFinite(tokens) || tokens <= 0) return null;
  if (tokens >= 1_000_000) return `${Number((tokens / 1_000_000).toFixed(1))}M`;
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K`;
  return String(Math.round(tokens));
}

const CAPABILITY_LABEL: Record<ModelPickerCapability, string> = {
  tools: "Tools",
  vision: "Vision",
  json: "JSON",
  reasoning: "Reasoning",
};

export function capabilityLabel(capability: ModelPickerCapability): string {
  return CAPABILITY_LABEL[capability];
}

/** Second row line: provider name, context size, and capability words joined by " · ". */
export function modelDetailLine(model: ModelPickerModel, providerName: string): string {
  const context = formatContextWindow(model.contextWindow);
  return [providerName, context ? `${context} context` : null, ...(model.capabilities ?? []).map(capabilityLabel)]
    .filter((part): part is string => Boolean(part))
    .join(" · ");
}

export function modelDisplayName(model: ModelPickerModel): string {
  return model.shortName?.trim() || model.name;
}

const BADGE_LABEL: Record<NonNullable<ModelPickerModel["badge"]>, string> = {
  new: "New",
  default: "Default",
  custom: "Custom",
};

/** Badge words for a row, in display order. */
export function modelBadges(model: ModelPickerModel): string[] {
  return [model.badge ? BADGE_LABEL[model.badge] : null, model.unavailable ? "Unavailable" : null].filter(
    (label): label is string => label !== null,
  );
}

/** Jump hint for the row at `index` (0-based): "⌘1" for a symbol modifier, "Ctrl+1" for a word. */
export function jumpLabel(modifierLabel: string, index: number): string {
  const digit = String(index + 1);
  return Array.from(modifierLabel).length === 1 ? `${modifierLabel}${digit}` : `${modifierLabel}+${digit}`;
}
