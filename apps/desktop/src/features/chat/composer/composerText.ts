// Composer text helpers: which menu the caret is in, replacing the typed trigger, and which skills a
// message invokes. Pure functions, so they run during prerendering.
import { findSkillMentions } from "@aidex/ui/components";
import type { SkillId, SkillSummary } from "@aidex/client/dto/skills";
import { MAX_INVOKED_SKILLS } from "@aidex/client/dto/skills";

export type ComposerTrigger = {
  /** `skill`: a `$name` token; `command`: a `/command` that starts its line. */
  kind: "skill" | "command";
  /** Text typed after the trigger character. */
  query: string;
  /** Range of the trigger and its query, replaced when a row is picked. */
  start: number;
  end: number;
};

const isSpace = (char: string | undefined) => char === " " || char === "\n" || char === "\t" || char === "\r";

/**
 * The menu trigger that ends at `caret`: `/query` when it is everything on the line before the caret,
 * else a `$query` token (from the previous whitespace to the caret).
 */
export function detectComposerTrigger(text: string, caret: number): ComposerTrigger | null {
  const end = Math.max(0, Math.min(text.length, Number.isFinite(caret) ? Math.floor(caret) : text.length));
  const lineStart = end === 0 ? 0 : text.lastIndexOf("\n", end - 1) + 1;
  const command = /^\/(\S*)$/.exec(text.slice(lineStart, end));
  if (command) return { kind: "command", query: command[1], start: lineStart, end };
  let start = end;
  while (start > 0 && !isSpace(text[start - 1])) start -= 1;
  const token = text.slice(start, end);
  if (token.startsWith("$")) return { kind: "skill", query: token.slice(1), start, end };
  return null;
}

/** Replaces `start..end` with `replacement`; the caret lands after it. */
export function replaceRange(
  text: string,
  start: number,
  end: number,
  replacement: string,
): { text: string; caret: number } {
  const from = Math.max(0, Math.min(text.length, start));
  const to = Math.max(from, Math.min(text.length, end));
  return { text: `${text.slice(0, from)}${replacement}${text.slice(to)}`, caret: from + replacement.length };
}

/**
 * Skill ids for the `$name` mentions in `text` that name an invocable skill, in order of first
 * mention. A skill picked from the menu keeps the id it was picked with while it is still invocable.
 */
export function invokedSkillIds(
  text: string,
  skills: readonly SkillSummary[],
  picked: ReadonlyMap<string, SkillId>,
): SkillId[] {
  const ids: SkillId[] = [];
  for (const name of findSkillMentions(text)) {
    const pickedId = picked.get(name);
    const skill =
      (pickedId ? skills.find((candidate) => candidate.id === pickedId) : undefined) ??
      skills.find((candidate) => candidate.name === name);
    if (skill && !ids.includes(skill.id)) ids.push(skill.id);
  }
  return ids;
}

/** Why `ids` cannot be sent, or null. */
export function invokedSkillsProblem(ids: readonly SkillId[]): string | null {
  return ids.length > MAX_INVOKED_SKILLS
    ? `A message can invoke at most ${MAX_INVOKED_SKILLS} skills. Remove some $skill mentions.`
    : null;
}

/** The name part of a `<scope>:<name>` skill id. */
export function skillNameOf(id: SkillId): string {
  const colon = id.indexOf(":");
  return colon === -1 ? id : id.slice(colon + 1);
}
