// The composer's `/` menu: built-in commands, then skills as `/skill:<name>` rows, ranked like
// t3code's slash command search. Pure functions, so they run during prerendering.
import type { SkillSummary } from "@aidex/client/dto/skills";
import { bestScore, normalizeQuery, rankBy, scoreMatch } from "./searchRanking";
import { scoreSkill } from "./skillSearch";

export type SlashCommandId = "chat" | "plan" | "agent" | "web" | "new";

export type SlashCommand = {
  id: SlashCommandId;
  command: string;
  description: string;
  /** Can't run in this conversation; listed only while the query is empty or starts its name. */
  disabled?: boolean;
};

export const slashCommands: readonly SlashCommand[] = [
  { id: "chat", command: "chat", description: "Switch to Chat: answers without agent tools" },
  { id: "plan", command: "plan", description: "Switch to Plan: proposes a plan with project context" },
  { id: "agent", command: "agent", description: "Switch to Agent: works in the project with coding tools" },
  { id: "web", command: "web", description: "Turn web search on or off for this conversation" },
  { id: "new", command: "new", description: "Start a new chat" },
];

export type SlashMatch = { type: "command"; command: SlashCommand } | { type: "skill"; skill: SkillSummary };

function scoreSlashMatch(match: SlashMatch, query: string, namesOnly: boolean): number | null {
  if (match.type === "skill") {
    if (query === "skill") return 0;
    const skillQuery = query.startsWith("skill:") ? query.slice("skill:".length) : query;
    const score = skillQuery ? scoreSkill(match.skill, skillQuery) : 0;
    if (score !== null) return score;
    // A partly typed "/ski" still lists every skill, after real matches.
    return "skill".startsWith(query) ? Number.MAX_SAFE_INTEGER : null;
  }
  // The menu highlights its first row even when that row can't run, and Enter then does nothing. A
  // command that can't run is therefore listed only while the query starts its name (where it ranks
  // first, as the command being typed): text that merely contains it or mentions it in a description,
  // such as `/context` for Plan, never lists a row that would hold Enter back from sending.
  if (match.command.disabled && !match.command.command.startsWith(query)) return null;
  const byName = scoreMatch(match.command.command, query, {
    exact: 0,
    prefix: 2,
    boundary: 4,
    includes: 6,
    fuzzy: 100,
    boundaryMarkers: ["-", "_", "/"],
  });
  if (namesOnly) return byName;
  return bestScore([
    byName,
    scoreMatch(match.command.description.toLowerCase(), query, {
      exact: 20,
      prefix: 22,
      boundary: 24,
      includes: 26,
    }),
  ]);
}

/**
 * Commands and skills matching the text typed after `/`; an empty query lists everything in order.
 * Once the query starts a command's name, other commands no longer match through their descriptions,
 * so `/chat` never puts `/new` ("Start a new chat") ahead of a `/chat` that cannot run. The menu shows
 * commands above skills, so a typed command that can't run is its first row (see `scoreSlashMatch`).
 */
export function searchSlashMatches(
  commands: readonly SlashCommand[],
  skills: readonly SkillSummary[],
  rawQuery: string,
): SlashMatch[] {
  const matches: SlashMatch[] = [
    ...commands.map((command) => ({ type: "command" as const, command })),
    ...skills.map((skill) => ({ type: "skill" as const, skill })),
  ];
  const query = normalizeQuery(rawQuery, /^\/+/);
  if (!query) return matches;
  const namesOnly = commands.some((command) => command.command.startsWith(query));
  return rankBy(
    matches,
    (match) => scoreSlashMatch(match, query, namesOnly),
    (match) => (match.type === "command" ? `0\u0000${match.command.command}` : `1\u0000${match.skill.name}`),
  );
}
