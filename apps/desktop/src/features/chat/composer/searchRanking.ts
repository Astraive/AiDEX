// Tiered text matching for the composer menus, after t3code's `searchRanking` (MIT): an exact match
// beats a prefix, which beats a word-boundary match, then a substring, then a fuzzy subsequence.
// Lower scores rank first. Pure functions, so they run during prerendering.

export type MatchTiers = {
  exact: number;
  prefix?: number;
  boundary?: number;
  includes?: number;
  fuzzy?: number;
  /** Characters that start a word for the boundary tier (default space, `-`, `_`, `/`). */
  boundaryMarkers?: readonly string[];
};

/** Trimmed, lower-cased query without the leading trigger characters (`$`, `/`). */
export function normalizeQuery(input: string, leading?: RegExp): string {
  const trimmed = input.trim();
  return (leading ? trimmed.replace(leading, "") : trimmed).toLowerCase();
}

function lengthPenalty(value: string, query: string): number {
  return Math.min(64, Math.max(0, value.length - query.length));
}

/** Score of `query` as an in-order subsequence of `value`; tight, early matches score lower. */
function subsequenceScore(value: string, query: string): number | null {
  let matched = 0;
  let first = -1;
  let previous = -1;
  let gaps = 0;
  for (let index = 0; index < value.length && matched < query.length; index += 1) {
    if (value[index] !== query[matched]) continue;
    if (first === -1) first = index;
    if (previous !== -1) gaps += index - previous - 1;
    previous = index;
    matched += 1;
  }
  if (matched < query.length) return null;
  const span = previous - first + 1 - query.length;
  return first * 2 + gaps * 3 + span + Math.min(64, value.length - query.length);
}

/** How well `value` matches `query` (both already lower-cased), or null when it does not. */
export function scoreMatch(value: string, query: string, tiers: MatchTiers): number | null {
  if (!value || !query) return null;
  if (value === query) return tiers.exact;
  if (tiers.prefix !== undefined && value.startsWith(query)) return tiers.prefix + lengthPenalty(value, query);
  if (tiers.boundary !== undefined) {
    let best: number | null = null;
    for (const marker of tiers.boundaryMarkers ?? [" ", "-", "_", "/"]) {
      const index = value.indexOf(`${marker}${query}`);
      if (index !== -1 && (best === null || index + marker.length < best)) best = index + marker.length;
    }
    if (best !== null) return tiers.boundary + best * 2 + lengthPenalty(value, query);
  }
  if (tiers.includes !== undefined) {
    const index = value.indexOf(query);
    if (index !== -1) return tiers.includes + index * 2 + lengthPenalty(value, query);
  }
  if (tiers.fuzzy !== undefined) {
    const fuzzy = subsequenceScore(value, query);
    if (fuzzy !== null) return tiers.fuzzy + fuzzy;
  }
  return null;
}

/** The best (lowest) of several scores, or null when none matched. */
export function bestScore(scores: readonly (number | null)[]): number | null {
  let best: number | null = null;
  for (const score of scores) if (score !== null && (best === null || score < best)) best = score;
  return best;
}

/** Items with a score, lowest first; ties sort by `tieBreaker`. Items scoring null are dropped. */
export function rankBy<T>(
  items: readonly T[],
  score: (item: T) => number | null,
  tieBreaker: (item: T) => string,
): T[] {
  const ranked: { item: T; score: number; tie: string }[] = [];
  for (const item of items) {
    const value = score(item);
    if (value !== null) ranked.push({ item, score: value, tie: tieBreaker(item) });
  }
  ranked.sort((left, right) => left.score - right.score || left.tie.localeCompare(right.tie));
  return ranked.map((entry) => entry.item);
}
