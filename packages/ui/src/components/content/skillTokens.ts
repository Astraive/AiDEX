// `$name` skill mentions in plain text. Pure functions only, so they run during prerendering and in tests.

export type SkillTokenSegment = { type: "text"; text: string } | { type: "skill"; name: string; raw: string };

/**
 * A `$` at the start of the text or after whitespace or an opening bracket or quote, then a name of
 * letters, digits, `_` or `-` that contains a letter and ends at whitespace, punctuation, or the end.
 * Amounts (`$20`, `$20k`, `$100M`, `$1e6`, `$1.5`) are not mentions.
 */
const SKILL_TOKEN =
  /(^|[\s(["'])\$(?![0-9][0-9_]*(?:[kKmMbBtT]|[eE][0-9]+)?(?![A-Za-z0-9_-]))(?=[A-Za-z0-9_-]*[A-Za-z])([A-Za-z0-9][A-Za-z0-9_-]{0,127})(?=$|[\s.,;:!?)\]}'"])/g;

/** Splits text into plain runs and `$name` mentions; text without mentions is one text segment. */
export function splitSkillTokens(text: string): SkillTokenSegment[] {
  const segments: SkillTokenSegment[] = [];
  let cursor = 0;
  SKILL_TOKEN.lastIndex = 0;
  for (let match = SKILL_TOKEN.exec(text); match !== null; match = SKILL_TOKEN.exec(text)) {
    const start = match.index + match[1].length;
    if (start > cursor) segments.push({ type: "text", text: text.slice(cursor, start) });
    segments.push({ type: "skill", name: match[2], raw: `$${match[2]}` });
    cursor = start + 1 + match[2].length;
  }
  if (cursor < text.length || segments.length === 0) segments.push({ type: "text", text: text.slice(cursor) });
  return segments;
}

/** Distinct `$name` mentions in order of first appearance. */
export function findSkillMentions(text: string): string[] {
  const names: string[] = [];
  for (const segment of splitSkillTokens(text)) {
    if (segment.type === "skill" && !names.includes(segment.name)) names.push(segment.name);
  }
  return names;
}
