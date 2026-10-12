// Tokenised model search ranking. Pure functions only, so they can be unit tested without a DOM.
// Behaviour follows t3code's model picker search (MIT License, Copyright (c) 2026 T3 Tools Inc.):
// every whitespace-separated token must match some field; each token scores its best field
// (exact < prefix < word boundary < substring < fuzzy subsequence); token scores add up; lower is better.

/** Score subtracted from favorites so they rank ahead of similar matches. */
export const FAVORITE_SCORE_BOOST = 24;

const BOUNDARY_MARKERS = [" ", "-", "_", "/"] as const;

/** Trims and lowercases a query or field. */
export function normalizeSearchQuery(input: string): string {
  return input.trim().toLowerCase();
}

/** Splits a query into normalized, non-empty tokens. */
export function tokenizeSearchQuery(query: string): string[] {
  const normalized = normalizeSearchQuery(query);
  return normalized ? normalized.split(/\s+/u) : [];
}

function lengthPenalty(value: string, query: string): number {
  return Math.min(64, Math.max(0, value.length - query.length));
}

/**
 * Scores `query` as an in-order subsequence of `value`: earlier first match, smaller gaps, and a
 * tighter span score lower. Returns null when the characters do not all appear in order.
 */
export function scoreSubsequenceMatch(value: string, query: string): number | null {
  if (!query) return 0;
  let queryIndex = 0;
  let firstMatch = -1;
  let previousMatch = -1;
  let gapPenalty = 0;
  for (let valueIndex = 0; valueIndex < value.length; valueIndex += 1) {
    if (value[valueIndex] !== query[queryIndex]) continue;
    if (firstMatch === -1) firstMatch = valueIndex;
    if (previousMatch !== -1) gapPenalty += valueIndex - previousMatch - 1;
    previousMatch = valueIndex;
    queryIndex += 1;
    if (queryIndex === query.length) {
      const spanPenalty = valueIndex - firstMatch + 1 - query.length;
      return firstMatch * 2 + gapPenalty * 3 + spanPenalty + lengthPenalty(value, query);
    }
  }
  return null;
}

function boundaryMatchIndex(value: string, query: string): number | null {
  let best: number | null = null;
  for (const marker of BOUNDARY_MARKERS) {
    const index = value.indexOf(`${marker}${query}`);
    if (index === -1) continue;
    const matchIndex = index + marker.length;
    if (best === null || matchIndex < best) best = matchIndex;
  }
  return best;
}

export type QueryMatchTiers = {
  exactBase: number;
  prefixBase: number;
  boundaryBase: number;
  includesBase: number;
  /** Omit to disable fuzzy matching (short tokens). */
  fuzzyBase?: number;
};

/**
 * Scores one normalized `value` against one normalized `query` token using tiered strategies.
 * Returns null when nothing matches.
 */
export function scoreQueryMatch(value: string, query: string, tiers: QueryMatchTiers): number | null {
  if (!value || !query) return null;
  if (value === query) return tiers.exactBase;
  if (value.startsWith(query)) return tiers.prefixBase + lengthPenalty(value, query);
  const boundary = boundaryMatchIndex(value, query);
  if (boundary !== null) return tiers.boundaryBase + boundary * 2 + lengthPenalty(value, query);
  const includes = value.indexOf(query);
  if (includes !== -1) return tiers.includesBase + includes * 2 + lengthPenalty(value, query);
  if (tiers.fuzzyBase !== undefined) {
    const fuzzy = scoreSubsequenceMatch(value, query);
    if (fuzzy !== null) return tiers.fuzzyBase + fuzzy;
  }
  return null;
}

/** The fields of a model that search looks at. */
export type SearchableModel = {
  name: string;
  shortName?: string;
  providerName: string;
  key: string;
  isFavorite?: boolean;
};

/** Normalized text of a model's name fields, used as the last search field. */
export function buildModelSearchText(model: SearchableModel): string {
  return normalizeSearchQuery(
    [model.name, model.shortName, model.providerName].filter((part): part is string => Boolean(part)).join(" "),
  );
}

/** Search fields in priority order; earlier fields win ties through a lower base score. */
export function modelSearchFields(model: SearchableModel): string[] {
  return [
    normalizeSearchQuery(model.name),
    ...(model.shortName ? [normalizeSearchQuery(model.shortName)] : []),
    normalizeSearchQuery(model.providerName),
    normalizeSearchQuery(model.key),
    buildModelSearchText(model),
  ];
}

function scoreToken(field: string, token: string, fieldIndex: number): number | null {
  const base = fieldIndex * 10;
  return scoreQueryMatch(field, token, {
    exactBase: base,
    prefixBase: base + 2,
    boundaryBase: base + 4,
    includesBase: base + 6,
    // Fuzzy matching on one- and two-letter tokens matches almost everything.
    fuzzyBase: token.length >= 3 ? base + 100 : undefined,
  });
}

/**
 * Scores a model against a query; lower is better. Returns 0 for an empty query and null when any
 * token matches no field. Favorites get `FAVORITE_SCORE_BOOST` subtracted.
 */
export function scoreModelSearch(model: SearchableModel, query: string): number | null {
  const tokens = tokenizeSearchQuery(query);
  if (tokens.length === 0) return 0;
  const fields = modelSearchFields(model);
  let total = 0;
  for (const token of tokens) {
    let best: number | null = null;
    for (let index = 0; index < fields.length; index += 1) {
      const score = scoreToken(fields[index], token, index);
      if (score !== null && (best === null || score < best)) best = score;
    }
    if (best === null) return null;
    total += best;
  }
  return model.isFavorite ? total - FAVORITE_SCORE_BOOST : total;
}

/**
 * Filters and orders `items` by search score: lower score first, then favorites, then name, then key.
 */
export function rankBySearch<T>(items: readonly T[], query: string, toSearchable: (item: T) => SearchableModel): T[] {
  const ranked: Array<{ item: T; score: number; model: SearchableModel }> = [];
  for (const item of items) {
    const model = toSearchable(item);
    const score = scoreModelSearch(model, query);
    if (score !== null) ranked.push({ item, score, model });
  }
  ranked.sort((left, right) => {
    if (left.score !== right.score) return left.score - right.score;
    if (Boolean(left.model.isFavorite) !== Boolean(right.model.isFavorite)) return left.model.isFavorite ? -1 : 1;
    const byName = left.model.name.localeCompare(right.model.name);
    if (byName !== 0) return byName;
    return left.model.key.localeCompare(right.model.key);
  });
  return ranked.map((entry) => entry.item);
}
