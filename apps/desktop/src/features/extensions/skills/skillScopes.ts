import type { FrontMatterValue, SkillScope, SkillSummary } from "@aidex/client/dto/skills";

export type ScopeFilter = "all" | SkillScope;

/** Filter chips in the order the scopes are listed. */
export const scopeFilterOptions = [
  { value: "all", label: "All" },
  { value: "drive", label: "Drive" },
  { value: "project", label: "Project" },
  { value: "repo", label: "Repo" },
  { value: "plugin", label: "Plugin" },
] as const satisfies readonly { value: ScopeFilter; label: string }[];

export const scopeLabels: Record<SkillScope, string> = {
  drive: "Drive",
  project: "Project",
  repo: "Repo",
  plugin: "Plugin",
};

/** Where each scope's skills come from, for the detail pane. */
export const scopeDescriptions: Record<SkillScope, string> = {
  drive: "Your portable skills in the drive's skills folder.",
  project: "This workspace's .aidex/skills folder.",
  repo: "Skills this repository shares with other agents (.agents, .claude, or .codex).",
  plugin: "Contributed by an enabled plugin.",
};

const scopeOrder: Record<SkillScope, number> = { drive: 0, project: 1, repo: 2, plugin: 3 };

/** Workspace skills are someone else's instructions: they stay off until the user turns them on. */
export function startsDisabled(scope: SkillScope): boolean {
  return scope === "project" || scope === "repo";
}

/** Who can start a skill, in words. */
export function invocationText(skill: Pick<SkillSummary, "name" | "user_invocable" | "model_invocable">): string {
  if (skill.user_invocable && skill.model_invocable) return `You, with $${skill.name}, or the model when it fits the request`;
  if (skill.user_invocable) return `Only you, with $${skill.name}`;
  if (skill.model_invocable) return "Only the model, when it fits the request";
  return "Nobody: the skill turns off both ways of invoking it";
}

/** Front matter values as one line of text. */
export function frontMatterText(value: FrontMatterValue): string {
  if (Array.isArray(value)) return value.join(", ");
  return String(value);
}

/**
 * Skills matching the scope filter and search, ranked like t3code's skill search: a name that
 * starts with the query first, then a name that contains it, then a description match.
 */
export function filterSkills(skills: readonly SkillSummary[], filter: ScopeFilter, query: string): SkillSummary[] {
  const needle = query.trim().toLowerCase().replace(/^\$/, "");
  const rank = (skill: SkillSummary): number => {
    if (!needle) return 0;
    const names = [skill.name, skill.display_name.toLowerCase()];
    if (names.some((name) => name.startsWith(needle))) return 0;
    if (names.some((name) => name.includes(needle))) return 1;
    if (`${skill.description} ${skill.plugin ?? ""}`.toLowerCase().includes(needle)) return 2;
    return -1;
  };
  return skills
    .filter((skill) => filter === "all" || skill.scope === filter)
    .map((skill) => ({ skill, rank: rank(skill) }))
    .filter((entry) => entry.rank >= 0)
    .sort(
      (a, b) =>
        a.rank - b.rank ||
        scopeOrder[a.skill.scope] - scopeOrder[b.skill.scope] ||
        a.skill.display_name.localeCompare(b.skill.display_name),
    )
    .map((entry) => entry.skill);
}

/** Skill names: a lowercase slug of up to 64 characters (`aidex_skills` naming rule). */
export const SKILL_NAME_PATTERN = /^[a-z0-9][a-z0-9-]{0,63}$/;
export const MAX_DESCRIPTION_CHARS = 1024;
export const MAX_BODY_BYTES = 64 * 1024;
