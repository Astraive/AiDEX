// Plan markdown helpers for `PlanCard`. Pure functions, so they run during prerendering and in tests.

import { markdownInlineText, parseMarkdown } from "../content/parseMarkdown";

/** Characters per line assumed when estimating how tall a long source line renders. */
const WRAP_COLUMNS = 100;

export type PlanParts = {
  /** Plain text of the plan's leading heading, or null when it does not start with one. */
  title: string | null;
  /** The markdown shown under the title: without that heading or a "Summary" heading right after it. */
  body: string;
};

function headingAt(lines: readonly string[], start: number): { index: number; text: string } | null {
  let index = start;
  while (index < lines.length && lines[index].trim() === "") index += 1;
  if (index >= lines.length) return null;
  const block = parseMarkdown(lines[index])[0];
  if (!block || block.type !== "heading") return null;
  return { index, text: markdownInlineText(block.children).trim() };
}

/** Splits a plan into its title and the body to render under it (as t3code's plan cards do). */
export function splitPlanMarkdown(markdown: string): PlanParts {
  const lines = markdown.replace(/\r\n?/g, "\n").split("\n");
  const heading = headingAt(lines, 0);
  if (!heading || !heading.text) return { title: null, body: trimBlankLines(lines).join("\n") };
  let rest = lines.slice(heading.index + 1);
  const summary = headingAt(rest, 0);
  if (summary && summary.text.toLowerCase() === "summary") rest = rest.slice(summary.index + 1);
  return { title: heading.text, body: trimBlankLines(rest).join("\n") };
}

function trimBlankLines(lines: readonly string[]): string[] {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === "") start += 1;
  while (end > start && lines[end - 1].trim() === "") end -= 1;
  return lines.slice(start, end);
}

/** Estimated rendered lines: each source line counts once per `WRAP_COLUMNS` characters. */
export function planLineCount(markdown: string): number {
  if (!markdown) return 0;
  let count = 0;
  for (const line of markdown.split("\n")) count += Math.max(1, Math.ceil(line.length / WRAP_COLUMNS));
  return count;
}

/**
 * The first `maxLines` estimated lines of a plan. A cut inside a code fence still renders as code,
 * because the parser treats an unclosed fence as code.
 */
export function planPreview(markdown: string, maxLines: number): string {
  const kept: string[] = [];
  let used = 0;
  for (const line of markdown.split("\n")) {
    const cost = Math.max(1, Math.ceil(line.length / WRAP_COLUMNS));
    if (kept.length > 0 && used + cost > maxLines) break;
    kept.push(line);
    used += cost;
  }
  return trimBlankLines(kept).join("\n");
}
