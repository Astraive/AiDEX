// Skills the composer offers and how `$` and `/skill:` queries rank them (t3code `providerSkillSearch`).
import type { SkillScope, SkillSummary } from "@aidex/client/dto/skills";
import { bestScore, normalizeQuery, rankBy, scoreMatch } from "./searchRanking";

export const skillScopeLabels: Record<SkillScope, string> = {
  project: "Project",
  repo: "Repo",
  drive: "Drive",
  plugin: "Plugin",
};

/** Skills a user can invoke: enabled, user-invocable, and without an error. */
export function invocableSkills(skills: readonly SkillSummary[] | undefined): SkillSummary[] {
  return (skills ?? []).filter((skill) => skill.enabled && skill.user_invocable && !skill.error);
}

/** Name, then display name, then description, then scope; null when nothing matches. */
export function scoreSkill(skill: SkillSummary, query: string): number | null {
  return bestScore([
    scoreMatch(skill.name.toLowerCase(), query, {
      exact: 0,
      prefix: 2,
      boundary: 4,
      includes: 6,
      fuzzy: 100,
      boundaryMarkers: ["-", "_", "/"],
    }),
    scoreMatch(skill.display_name.toLowerCase(), query, {
      exact: 1,
      prefix: 3,
      boundary: 5,
      includes: 7,
      fuzzy: 110,
    }),
    scoreMatch(skill.description.toLowerCase(), query, {
      exact: 30,
      prefix: 32,
      boundary: 34,
      includes: 36,
    }),
    scoreMatch(skill.scope, query, { exact: 40, prefix: 42, includes: 44 }),
  ]);
}

const skillTieBreaker = (skill: SkillSummary) => `${skill.display_name.toLowerCase()}\u0000${skill.name}`;

/** Skills matching the text typed after `$`, best first; an empty query lists them all by name. */
export function searchSkills(skills: readonly SkillSummary[], rawQuery: string): SkillSummary[] {
  const query = normalizeQuery(rawQuery, /^\$+/);
  if (!query) return [...skills].sort((left, right) => skillTieBreaker(left).localeCompare(skillTieBreaker(right)));
  return rankBy(skills, (skill) => scoreSkill(skill, query), skillTieBreaker);
}
