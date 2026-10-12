// Dependency-free markdown parser for model output, skill bodies, plans, and markdown artifacts.
// Pure functions only (no DOM, no React), so it runs during prerendering and in plain unit tests.
//
// Supported: ATX and setext headings, paragraphs, emphasis, strong, ~~strikethrough~~, code spans,
// fenced code (an unclosed fence runs to the end, so streaming text renders as code), nested ordered,
// unordered and task lists, blockquotes, GFM tables, thematic breaks, inline and reference links,
// <autolinks>, bare http(s)/www links, images (also inside link text, e.g. badges), hard breaks,
// backslash escapes, and common entities.
//
// Safety: raw HTML is never interpreted; it stays literal text. Link and image URLs are reduced to
// absolute http(s) (and mailto for links) by `safeExternalUrl`; anything else becomes `url: null`, so a
// renderer can show the text without making it actionable. Nesting depth and delimiter runs are
// bounded so hostile input cannot exhaust the stack, and parsing stays linear on long whitespace runs,
// deep bracket nests, and unclosed link titles, so it cannot freeze the UI either.

import { safeExternalUrl } from "../../utils/url";

export type MarkdownInline =
  | { type: "text"; text: string }
  | { type: "code"; code: string }
  | { type: "emphasis"; children: MarkdownInline[] }
  | { type: "strong"; children: MarkdownInline[] }
  | { type: "strikethrough"; children: MarkdownInline[] }
  /** `url` is null when the destination is not a safe absolute http(s) or mailto URL. */
  | { type: "link"; url: string | null; title?: string; children: MarkdownInline[] }
  /** Images are never loaded; `url` is null unless it is a safe absolute http(s) URL. */
  | { type: "image"; url: string | null; alt: string; title?: string }
  /** Hard line break: two trailing spaces or a backslash before a newline. */
  | { type: "break" }
  /** Plain newline inside a paragraph. */
  | { type: "softbreak" };

export type MarkdownTableAlign = "left" | "center" | "right" | null;

export type MarkdownListItem = {
  /** `null` for a plain item; `true`/`false` for a `- [x]` / `- [ ]` task item. */
  checked: boolean | null;
  children: MarkdownBlock[];
};

export type MarkdownBlock =
  | { type: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; children: MarkdownInline[] }
  | { type: "paragraph"; children: MarkdownInline[] }
  /** `closed` is false when the fence never closed, e.g. while a response is still streaming. */
  | { type: "code"; code: string; language: string | null; closed: boolean }
  /** `tight` lists render their paragraphs without paragraph spacing. */
  | { type: "list"; ordered: boolean; start: number; tight: boolean; items: MarkdownListItem[] }
  | { type: "blockquote"; children: MarkdownBlock[] }
  | { type: "table"; align: MarkdownTableAlign[]; header: MarkdownInline[][]; rows: MarkdownInline[][][] }
  | { type: "rule" };

/** Containers (lists, quotes) nested deeper than this are read as plain paragraph text. */
const MAX_BLOCK_DEPTH = 12;
/** Emphasis nested deeper than this stays literal. */
const MAX_INLINE_DEPTH = 24;
/** Longer `*`/`_` runs are literal text rather than emphasis delimiters. */
const MAX_DELIMITER_RUN = 12;
/** Longer link labels are never looked up as references, as in CommonMark; nested brackets stay linear. */
const MAX_LABEL_LENGTH = 999;

// ---------------------------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------------------------

/** Parses markdown source into a block tree. Never throws; any input yields some tree. */
export function parseMarkdown(source: string): MarkdownBlock[] {
  const text = typeof source === "string" ? source : "";
  const lines = text.replace(/\r\n?/g, "\n").replace(/\u0000/g, "�").split("\n");
  const definitions: Definitions = new Map();
  const raw = parseBlocks(lines, 0, definitions);
  return raw.map((block) => finishBlock(block, definitions));
}

/** Plain text of inline nodes, e.g. for an image's alt text or an accessible label. */
export function markdownInlineText(nodes: readonly MarkdownInline[]): string {
  let text = "";
  for (const node of nodes) {
    switch (node.type) {
      case "text":
        text += node.text;
        break;
      case "code":
        text += node.code;
        break;
      case "image":
        text += node.alt;
        break;
      case "break":
      case "softbreak":
        text += " ";
        break;
      default:
        text += markdownInlineText(node.children);
    }
  }
  return text;
}

// ---------------------------------------------------------------------------------------------
// Block structure
// ---------------------------------------------------------------------------------------------

type LinkDefinition = { url: string; title?: string };
type Definitions = Map<string, LinkDefinition>;

type RawListItem = { checked: boolean | null; children: RawBlock[] };

type RawBlock =
  | { type: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; text: string }
  | { type: "paragraph"; text: string }
  | { type: "code"; code: string; language: string | null; closed: boolean }
  | { type: "list"; ordered: boolean; start: number; tight: boolean; items: RawListItem[] }
  | { type: "blockquote"; children: RawBlock[] }
  | { type: "table"; align: MarkdownTableAlign[]; header: string[]; rows: string[][] }
  | { type: "rule" };

// Line patterns are anchored at the start and take the rest of a line with `[\s\S]*`, so each test is
// linear in the line: `.*$` backtracks over the whole line when it holds U+2028, and `(.*?)[ \t]*$`
// rescans a run of spaces from each of its positions. For the same reason trailing blanks are cut
// with `trimTrailingBlanks` rather than `/[ \t]+$/`.
const BLANK = /^[ \t]*$/;
const ATX_HEADING = /^[ \t]*(#{1,6})(?:[ \t]+([\s\S]*))?$/;
const THEMATIC_BREAK = /^[ \t]*([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const SETEXT_UNDERLINE = /^[ \t]*(?:=+|-+)[ \t]*$/;
const QUOTE = /^[ \t]*>/;
const QUOTE_MARKER = /^[ \t]*> ?/;
const FENCE_OPEN = /^([ \t]*)(`{3,}|~{3,})([\s\S]*)$/;
const FENCE_CLOSE = /^[ \t]*(`{3,}|~{3,})[ \t]*$/;
const BULLET_ITEM = /^([ \t]*)([-+*])(?:([ \t]+)([\s\S]*))?$/;
const ORDERED_ITEM = /^([ \t]*)(\d{1,9})([.)])(?:([ \t]+)([\s\S]*))?$/;
const TASK_MARKER = /^\[([ xX])\](?:[ \t]+|$)/;
const LANGUAGE = /^[A-Za-z0-9_+#.-]{1,40}$/;
const DEFINITION =
  /^[ \t]{0,3}\[((?:[^\\[\]]|\\.){1,999})\]:[ \t]*(?:<([^<>\n]*)>|(\S+))(?:[ \t]+(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|\(((?:[^()\\]|\\.)*)\)))?[ \t]*$/;

function isBlank(line: string): boolean {
  return BLANK.test(line);
}

/** Drops trailing spaces and tabs in one backward scan. */
function trimTrailingBlanks(text: string): string {
  let end = text.length;
  while (end > 0 && (text[end - 1] === " " || text[end - 1] === "\t")) end -= 1;
  return end === text.length ? text : text.slice(0, end);
}

function trailingSpaceCount(text: string): number {
  let count = 0;
  while (count < text.length && text[text.length - 1 - count] === " ") count += 1;
  return count;
}

/** Drops an ATX heading's closing `#` run when it stands alone or follows a space or tab. */
function stripClosingSequence(text: string): string {
  let end = text.length;
  while (end > 0 && text[end - 1] === "#") end -= 1;
  if (end === text.length) return text;
  if (end === 0) return "";
  const before = text[end - 1];
  return before === " " || before === "\t" ? text.slice(0, end) : text;
}

/** Column width of the leading whitespace (tabs advance to the next multiple of 4). */
function indentOf(line: string): number {
  let column = 0;
  for (let index = 0; index < line.length; index += 1) {
    const char = line[index];
    if (char === " ") column += 1;
    else if (char === "\t") column += 4 - (column % 4);
    else break;
  }
  return column;
}

/** Removes up to `columns` columns of leading whitespace, splitting a tab that straddles the limit. */
function stripIndent(line: string, columns: number): string {
  let column = 0;
  let index = 0;
  while (index < line.length && column < columns) {
    const char = line[index];
    if (char === " ") {
      column += 1;
      index += 1;
    } else if (char === "\t") {
      const width = 4 - (column % 4);
      if (column + width > columns) return " ".repeat(column + width - columns) + line.slice(index + 1);
      column += width;
      index += 1;
    } else {
      break;
    }
  }
  return line.slice(index);
}

type Fence = { indent: number; char: string; length: number; language: string | null };

function matchFenceOpen(line: string): Fence | null {
  const match = FENCE_OPEN.exec(line);
  if (!match) return null;
  const marker = match[2];
  const info = match[3];
  // A backtick "fence" whose info string holds a backtick is an inline code span instead.
  if (marker[0] === "`" && info.indexOf("`") !== -1) return null;
  const word = info.trim().split(/[ \t{]/)[0] ?? "";
  return {
    indent: indentOf(match[1]),
    char: marker[0],
    length: marker.length,
    language: LANGUAGE.test(word) ? word : null,
  };
}

function isFenceClose(line: string, fence: Fence): boolean {
  const match = FENCE_CLOSE.exec(line);
  return match !== null && match[1][0] === fence.char && match[1].length >= fence.length;
}

type ListMarker = {
  ordered: boolean;
  /** Bullet character or ordered delimiter; items of one list share it. */
  symbol: string;
  start: number;
  contentIndent: number;
  content: string;
  empty: boolean;
};

function columnsOf(spacing: string, startColumn: number): number {
  let column = startColumn;
  for (const char of spacing) column += char === "\t" ? 4 - (column % 4) : 1;
  return column - startColumn;
}

function matchListItem(line: string): ListMarker | null {
  let lead: string;
  let symbol: string;
  let markerWidth: number;
  let spacing: string;
  let rest: string;
  let ordered = false;
  let start = 1;
  const bullet = BULLET_ITEM.exec(line);
  if (bullet) {
    [, lead, symbol] = bullet;
    markerWidth = 1;
    spacing = bullet[3] ?? "";
    rest = bullet[4] ?? "";
  } else {
    const numbered = ORDERED_ITEM.exec(line);
    if (!numbered) return null;
    ordered = true;
    lead = numbered[1];
    start = Number(numbered[2]);
    symbol = numbered[3];
    markerWidth = numbered[2].length + 1;
    spacing = numbered[4] ?? "";
    rest = numbered[5] ?? "";
  }
  const indent = indentOf(lead);
  const empty = rest.trim() === "";
  const spaceWidth = columnsOf(spacing, indent + markerWidth);
  if (empty) {
    return { ordered, symbol, start, contentIndent: indent + markerWidth + 1, content: "", empty };
  }
  if (spaceWidth > 4) {
    // Wide gaps keep their extra spaces as content rather than pushing the content column out.
    return {
      ordered,
      symbol,
      start,
      contentIndent: indent + markerWidth + 1,
      content: " ".repeat(spaceWidth - 1) + rest,
      empty,
    };
  }
  return { ordered, symbol, start, contentIndent: indent + markerWidth + spaceWidth, content: rest, empty };
}

/** Lines that begin a new block even without a blank line before them. */
function startsBlock(line: string): boolean {
  return (
    matchFenceOpen(line) !== null ||
    ATX_HEADING.test(line) ||
    THEMATIC_BREAK.test(line) ||
    QUOTE.test(line) ||
    matchListItem(line) !== null
  );
}

/** Lines that end a paragraph: like `startsBlock`, but only lists that start at 1 with content. */
function interruptsParagraph(line: string): boolean {
  if (matchFenceOpen(line) || ATX_HEADING.test(line) || THEMATIC_BREAK.test(line) || QUOTE.test(line)) return true;
  const item = matchListItem(line);
  return item !== null && !item.empty && (!item.ordered || item.start === 1);
}

function splitTableRow(line: string): string[] {
  let text = line.trim();
  if (text.startsWith("|")) text = text.slice(1);
  if (text.endsWith("|") && !text.endsWith("\\|")) text = text.slice(0, -1);
  const cells: string[] = [];
  let current = "";
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === "\\" && text[index + 1] === "|") {
      current += "|";
      index += 1;
    } else if (char === "|") {
      cells.push(current.trim());
      current = "";
    } else {
      current += char;
    }
  }
  cells.push(current.trim());
  return cells;
}

function parseDelimiterRow(line: string): MarkdownTableAlign[] | null {
  if (line.indexOf("|") === -1 && line.indexOf("-") === -1) return null;
  const align: MarkdownTableAlign[] = [];
  for (const cell of splitTableRow(line)) {
    if (!/^:?-+:?$/.test(cell)) return null;
    const left = cell.startsWith(":");
    const right = cell.endsWith(":");
    align.push(left && right ? "center" : right ? "right" : left ? "left" : null);
  }
  return align.length > 0 ? align : null;
}

/** A GFM table starts at `index` when that line and the next form a header and a matching delimiter row. */
function tableAt(lines: readonly string[], index: number): { header: string[]; align: MarkdownTableAlign[] } | null {
  const headerLine = lines[index];
  const delimiterLine = lines[index + 1];
  if (delimiterLine === undefined || headerLine.indexOf("|") === -1 || delimiterLine.indexOf("|") === -1) return null;
  const align = parseDelimiterRow(delimiterLine);
  if (!align) return null;
  const header = splitTableRow(headerLine);
  return header.length === align.length ? { header, align } : null;
}

function normalizeLabel(label: string): string {
  return label.trim().replace(/\s+/g, " ").toLowerCase();
}

function matchDefinition(line: string): { label: string; definition: LinkDefinition } | null {
  const match = DEFINITION.exec(line);
  if (!match || match[1].trim() === "") return null;
  const title = match[4] ?? match[5] ?? match[6];
  return {
    label: normalizeLabel(match[1]),
    definition: { url: unescapeString(match[2] ?? match[3] ?? ""), title: title ? unescapeString(title) : undefined },
  };
}

/** True when a blank line separates two direct children of a list item (outside fenced code). */
function hasBlankBetweenChildren(itemLines: readonly string[]): boolean {
  let fence: Fence | null = null;
  let sawBlank = false;
  let sawContent = false;
  for (const line of itemLines) {
    if (fence) {
      if (isFenceClose(line, fence)) fence = null;
      continue;
    }
    if (isBlank(line)) {
      if (sawContent) sawBlank = true;
      continue;
    }
    if (sawBlank && indentOf(line) === 0) return true;
    sawContent = true;
    sawBlank = false;
    fence = matchFenceOpen(line);
  }
  return false;
}

function parseBlocks(lines: readonly string[], depth: number, definitions: Definitions): RawBlock[] {
  const blocks: RawBlock[] = [];
  const containers = depth < MAX_BLOCK_DEPTH;
  let index = 0;

  while (index < lines.length) {
    const line = lines[index];
    if (isBlank(line)) {
      index += 1;
      continue;
    }

    const fence = matchFenceOpen(line);
    if (fence) {
      const code: string[] = [];
      let closed = false;
      index += 1;
      while (index < lines.length) {
        if (isFenceClose(lines[index], fence)) {
          closed = true;
          index += 1;
          break;
        }
        code.push(stripIndent(lines[index], fence.indent));
        index += 1;
      }
      blocks.push({ type: "code", code: code.join("\n"), language: fence.language, closed });
      continue;
    }

    const heading = ATX_HEADING.exec(line);
    if (heading) {
      const text = stripClosingSequence(trimTrailingBlanks(heading[2] ?? "")).trim();
      blocks.push({ type: "heading", level: heading[1].length as 1 | 2 | 3 | 4 | 5 | 6, text });
      index += 1;
      continue;
    }

    if (THEMATIC_BREAK.test(line)) {
      blocks.push({ type: "rule" });
      index += 1;
      continue;
    }

    if (containers && QUOTE.test(line)) {
      const quoted: string[] = [];
      while (index < lines.length) {
        const current = lines[index];
        if (QUOTE.test(current)) {
          quoted.push(current.replace(QUOTE_MARKER, ""));
          index += 1;
          continue;
        }
        // Lazy continuation: an unmarked line continues the quoted paragraph.
        const last = quoted[quoted.length - 1];
        if (!isBlank(current) && last !== undefined && !isBlank(last) && !startsBlock(current)) {
          quoted.push(current);
          index += 1;
          continue;
        }
        break;
      }
      blocks.push({ type: "blockquote", children: parseBlocks(quoted, depth + 1, definitions) });
      continue;
    }

    const marker = containers ? matchListItem(line) : null;
    if (marker) {
      const list = parseList(lines, index, marker, depth, definitions);
      blocks.push(list.block);
      index = list.next;
      continue;
    }

    const table = tableAt(lines, index);
    if (table) {
      const rows: string[][] = [];
      index += 2;
      while (index < lines.length) {
        const current = lines[index];
        if (isBlank(current) || current.indexOf("|") === -1 || startsBlock(current)) break;
        const cells = splitTableRow(current).slice(0, table.header.length);
        while (cells.length < table.header.length) cells.push("");
        rows.push(cells);
        index += 1;
      }
      blocks.push({ type: "table", align: table.align, header: table.header, rows });
      continue;
    }

    const definition = matchDefinition(line);
    if (definition) {
      if (!definitions.has(definition.label)) definitions.set(definition.label, definition.definition);
      index += 1;
      continue;
    }

    // Paragraph, possibly turned into a setext heading by an underline.
    const paragraph: string[] = [line.replace(/^[ \t]+/, "")];
    let setextLevel: 0 | 1 | 2 = 0;
    index += 1;
    while (index < lines.length) {
      const current = lines[index];
      if (isBlank(current)) break;
      if (SETEXT_UNDERLINE.test(current)) {
        setextLevel = current.indexOf("=") !== -1 ? 1 : 2;
        index += 1;
        break;
      }
      if (interruptsParagraph(current) || tableAt(lines, index)) break;
      paragraph.push(current.replace(/^[ \t]+/, ""));
      index += 1;
    }
    // Trailing spaces on the last line are not a hard break.
    const text = trimTrailingBlanks(paragraph.join("\n"));
    blocks.push(setextLevel ? { type: "heading", level: setextLevel, text } : { type: "paragraph", text });
  }

  return blocks;
}

function parseList(
  lines: readonly string[],
  startIndex: number,
  first: ListMarker,
  depth: number,
  definitions: Definitions,
): { block: RawBlock; next: number } {
  const items: RawListItem[] = [];
  let index = startIndex;
  let loose = false;
  let blankBeforeItem = false;

  while (index < lines.length) {
    const line = lines[index];
    if (THEMATIC_BREAK.test(line)) break;
    const marker = matchListItem(line);
    if (!marker || marker.ordered !== first.ordered || marker.symbol !== first.symbol) break;
    if (items.length > 0 && blankBeforeItem) loose = true;

    const itemLines: string[] = [marker.content];
    let contentStarted = !marker.empty;
    let previousBlank = false;
    index += 1;
    while (index < lines.length) {
      const current = lines[index];
      if (isBlank(current)) {
        // An item that opens with a blank line is empty when another blank line follows.
        if (!contentStarted) break;
        itemLines.push("");
        previousBlank = true;
        index += 1;
        continue;
      }
      if (indentOf(current) >= marker.contentIndent) {
        itemLines.push(stripIndent(current, marker.contentIndent));
        contentStarted = true;
        previousBlank = false;
        index += 1;
        continue;
      }
      // Lazy continuation of the item's paragraph.
      if (contentStarted && !previousBlank && !startsBlock(current)) {
        itemLines.push(current.replace(/^[ \t]+/, ""));
        index += 1;
        continue;
      }
      break;
    }

    let trailingBlank = false;
    while (itemLines.length > 1 && itemLines[itemLines.length - 1] === "") {
      itemLines.pop();
      trailingBlank = true;
    }
    blankBeforeItem = trailingBlank;
    if (!loose && hasBlankBetweenChildren(itemLines)) loose = true;

    const children = parseBlocks(itemLines, depth + 1, definitions);
    let checked: boolean | null = null;
    const head = children[0];
    if (head && head.type === "paragraph") {
      const task = TASK_MARKER.exec(head.text);
      if (task) {
        checked = task[1] !== " ";
        const rest = head.text.slice(task[0].length);
        if (rest.trim() === "") children.shift();
        else children[0] = { type: "paragraph", text: rest };
      }
    }
    items.push({ checked, children });
  }

  return {
    block: { type: "list", ordered: first.ordered, start: first.start, tight: !loose, items },
    next: index,
  };
}

function finishBlock(block: RawBlock, definitions: Definitions): MarkdownBlock {
  switch (block.type) {
    case "heading":
      return { type: "heading", level: block.level, children: parseInline(block.text, definitions, "top") };
    case "paragraph":
      return { type: "paragraph", children: parseInline(block.text, definitions, "top") };
    case "code":
    case "rule":
      return block;
    case "blockquote":
      return { type: "blockquote", children: block.children.map((child) => finishBlock(child, definitions)) };
    case "list":
      return {
        type: "list",
        ordered: block.ordered,
        start: block.start,
        tight: block.tight,
        items: block.items.map((item) => ({
          checked: item.checked,
          children: item.children.map((child) => finishBlock(child, definitions)),
        })),
      };
    case "table":
      return {
        type: "table",
        align: block.align,
        header: block.header.map((cell) => parseInline(cell, definitions, "top")),
        rows: block.rows.map((row) => row.map((cell) => parseInline(cell, definitions, "top"))),
      };
  }
}

// ---------------------------------------------------------------------------------------------
// Inline content
// ---------------------------------------------------------------------------------------------

const ESCAPABLE = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";
const PUNCTUATION =
  /[!-/:-@[-`{-~¡-¿‐-‧‰-⁞　-〿！-／：-＠［-｀｛-･]/;
const WHITESPACE = /\s/;
const AUTOLINK_URI = /^<([A-Za-z][A-Za-z0-9+.-]{1,31}:[^\s<>]*)>/;
const AUTOLINK_EMAIL =
  /^<([A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?(?:\.[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)*)>/;
const AUTOLINK_LITERAL = /^(?:https?:\/\/|www\.)[A-Za-z0-9][^\s<]*/i;
const ENTITY = /^&(?:#[xX]([0-9a-fA-F]{1,6})|#([0-9]{1,7})|([A-Za-z][A-Za-z0-9]{1,31}));/;

const NAMED_ENTITIES = new Map<string, string>([
  ["amp", "&"],
  ["lt", "<"],
  ["gt", ">"],
  ["quot", '"'],
  ["apos", "'"],
  ["nbsp", " "],
  ["copy", "©"],
  ["reg", "®"],
  ["trade", "™"],
  ["hellip", "…"],
  ["mdash", "—"],
  ["ndash", "–"],
  ["lsquo", "‘"],
  ["rsquo", "’"],
  ["ldquo", "“"],
  ["rdquo", "”"],
  ["laquo", "«"],
  ["raquo", "»"],
  ["bull", "•"],
  ["middot", "·"],
  ["times", "×"],
  ["divide", "÷"],
  ["deg", "°"],
  ["plusmn", "±"],
  ["para", "¶"],
  ["sect", "§"],
  ["euro", "€"],
  ["pound", "£"],
  ["yen", "¥"],
  ["cent", "¢"],
  ["larr", "←"],
  ["rarr", "→"],
  ["uarr", "↑"],
  ["darr", "↓"],
  ["harr", "↔"],
  ["check", "✓"],
]);

function isWhitespace(char: string | undefined): boolean {
  return char === undefined || char === "" || WHITESPACE.test(char);
}

function isPunctuation(char: string | undefined): boolean {
  return char !== undefined && char !== "" && PUNCTUATION.test(char);
}

function matchEntity(text: string, index: number): { value: string; end: number } | null {
  const match = ENTITY.exec(text.slice(index, index + 40));
  if (!match) return null;
  if (match[3] !== undefined) {
    const value = NAMED_ENTITIES.get(match[3]);
    return value === undefined ? null : { value, end: index + match[0].length };
  }
  const code = match[1] !== undefined ? parseInt(match[1], 16) : parseInt(match[2], 10);
  const valid = code > 0 && code <= 0x10ffff && (code < 0xd800 || code > 0xdfff);
  return { value: valid ? String.fromCodePoint(code) : "�", end: index + match[0].length };
}

/** Resolves backslash escapes and entities in a link destination or title. */
function unescapeString(value: string): string {
  let result = "";
  for (let index = 0; index < value.length; index += 1) {
    const char = value[index];
    if (char === "\\" && index + 1 < value.length && ESCAPABLE.indexOf(value[index + 1]) !== -1) {
      result += value[index + 1];
      index += 1;
    } else if (char === "&") {
      const entity = matchEntity(value, index);
      if (entity) {
        result += entity.value;
        index = entity.end - 1;
      } else {
        result += char;
      }
    } else {
      result += char;
    }
  }
  return result;
}

type CodeSpan = { end: number; contentStart: number; contentEnd: number };

/** Pairs backtick runs into code spans in one linear pass; keys are the opening positions. */
function findCodeSpans(text: string): Map<number, CodeSpan> {
  const runs: Array<{ start: number; length: number; escaped: boolean }> = [];
  for (let index = 0; index < text.length; ) {
    if (text[index] !== "`") {
      index += 1;
      continue;
    }
    let backslashes = 0;
    while (text[index - 1 - backslashes] === "\\") backslashes += 1;
    let end = index;
    while (text[end] === "`") end += 1;
    runs.push({ start: index, length: end - index, escaped: backslashes % 2 === 1 });
    index = end;
  }
  const byLength = new Map<number, number[]>();
  runs.forEach((run, runIndex) => {
    const list = byLength.get(run.length);
    if (list) list.push(runIndex);
    else byLength.set(run.length, [runIndex]);
  });
  const cursors = new Map<number, number>();
  const nextRunOfLength = (length: number, after: number): number => {
    const list = byLength.get(length);
    if (!list) return -1;
    let cursor = cursors.get(length) ?? 0;
    while (cursor < list.length && list[cursor] <= after) cursor += 1;
    cursors.set(length, cursor);
    return cursor < list.length ? list[cursor] : -1;
  };

  const spans = new Map<number, CodeSpan>();
  let runIndex = 0;
  while (runIndex < runs.length) {
    const run = runs[runIndex];
    // An escaped first backtick is literal; the rest of the run may still open a span.
    const start = run.escaped ? run.start + 1 : run.start;
    const length = run.escaped ? run.length - 1 : run.length;
    if (length === 0) {
      runIndex += 1;
      continue;
    }
    const closer = nextRunOfLength(length, runIndex);
    if (closer === -1) {
      runIndex += 1;
      continue;
    }
    const closing = runs[closer];
    spans.set(start, { end: closing.start + closing.length, contentStart: start + length, contentEnd: closing.start });
    runIndex = closer + 1;
  }
  return spans;
}

/** Pairs `[` with `]` (skipping escapes and code spans) in one pass; keys are the `[` positions. */
function findBracketPairs(text: string, spans: Map<number, CodeSpan>): Map<number, number> {
  const pairs = new Map<number, number>();
  const stack: number[] = [];
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === "\\") {
      index += 1;
      continue;
    }
    const span = spans.get(index);
    if (span) {
      index = span.end - 1;
      continue;
    }
    if (char === "[") stack.push(index);
    else if (char === "]" && stack.length > 0) pairs.set(stack.pop() as number, index);
  }
  return pairs;
}

function skipSpace(text: string, index: number): number {
  let position = index;
  while (position < text.length && (text[position] === " " || text[position] === "\t" || text[position] === "\n")) {
    position += 1;
  }
  return position;
}

/** Where the link title opening at `open` closes, and the first non-space position after that. */
type TitleFinder = (open: number) => { end: number; after: number } | null;

/**
 * A title scan skips each backslash-escaped character. A title only opens after whitespace, so its
 * scan steps on the same characters as one scan from the start of the text, and that one scan's
 * closer positions answer every title. Scanning per link was quadratic on a run of unclosed titles
 * such as `[a](x (` repeated.
 */
function titleFinder(text: string): TitleFinder {
  const closers = new Map<string, number[]>();
  const afterCloser = new Map<number, number>();
  return (open) => {
    const closing = text[open] === "(" ? ")" : text[open];
    let positions = closers.get(closing);
    if (!positions) {
      positions = [];
      for (let index = 0; index < text.length; index += 1) {
        if (text[index] === "\\") index += 1;
        else if (text[index] === closing) positions.push(index);
      }
      closers.set(closing, positions);
    }
    // Binary search for the first closer after the opening character.
    let low = 0;
    let high = positions.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (positions[middle] <= open) low = middle + 1;
      else high = middle;
    }
    if (low === positions.length) return null;
    const end = positions[low];
    let after = afterCloser.get(end);
    if (after === undefined) {
      after = skipSpace(text, end + 1);
      afterCloser.set(end, after);
    }
    return { end, after };
  };
}

/** Parses `(destination "title")` starting just after the `(`. */
function parseInlineDestination(
  text: string,
  open: number,
  findTitle: TitleFinder,
): { url: string; title?: string; end: number } | null {
  let index = skipSpace(text, open);
  let destination: string;
  if (text[index] === "<") {
    let end = index + 1;
    while (end < text.length && text[end] !== ">" && text[end] !== "\n" && text[end] !== "<") {
      if (text[end] === "\\") end += 1;
      end += 1;
    }
    if (text[end] !== ">") return null;
    destination = text.slice(index + 1, end);
    index = end + 1;
  } else {
    let depth = 0;
    let end = index;
    while (end < text.length) {
      const char = text[end];
      if (char === "\\" && end + 1 < text.length && ESCAPABLE.indexOf(text[end + 1]) !== -1) {
        end += 2;
        continue;
      }
      if (char === "(") {
        depth += 1;
        if (depth > 32) return null;
      } else if (char === ")") {
        if (depth === 0) break;
        depth -= 1;
      } else if (char <= " ") {
        break;
      }
      end += 1;
    }
    if (depth !== 0) return null;
    destination = text.slice(index, end);
    index = end;
  }
  const beforeTitle = index;
  index = skipSpace(text, index);
  let title: string | undefined;
  const quote = text[index];
  if (index > beforeTitle && (quote === '"' || quote === "'" || quote === "(")) {
    const close = findTitle(index);
    if (!close || text[close.after] !== ")") return null;
    title = unescapeString(text.slice(index + 1, close.end));
    index = close.after;
  }
  if (text[index] !== ")") return null;
  return { url: unescapeString(destination), title, end: index + 1 };
}

function matchAutolinkLiteral(text: string, index: number): { url: string; label: string; end: number } | null {
  const match = AUTOLINK_LITERAL.exec(text.slice(index, index + 4096));
  if (!match) return null;
  let candidate = match[0];
  // Counted once rather than per trimmed character: of the parentheses, trimming only drops `)`.
  let opens = 0;
  let closes = 0;
  for (const char of candidate) {
    if (char === "(") opens += 1;
    else if (char === ")") closes += 1;
  }
  for (;;) {
    const last = candidate[candidate.length - 1];
    if (last === undefined) return null;
    if ("?!.,:*_~'\"".indexOf(last) !== -1) {
      candidate = candidate.slice(0, -1);
      continue;
    }
    if (last === ")" && closes > opens) {
      candidate = candidate.slice(0, -1);
      closes -= 1;
      continue;
    }
    if (last === ";") {
      // A trailing entity reference can only start at the last `&`.
      const amp = candidate.lastIndexOf("&");
      if (amp !== -1 && /^&[A-Za-z0-9]+;$/.test(candidate.slice(amp))) {
        candidate = candidate.slice(0, amp);
        continue;
      }
    }
    break;
  }
  const url = /^www\./i.test(candidate) ? `https://${candidate}` : candidate;
  if (!/^(?:https?:\/\/|www\.)[A-Za-z0-9]/i.test(candidate)) return null;
  return { url, label: candidate, end: index + candidate.length };
}

// Inline items form a doubly linked list so emphasis can wrap ranges without re-indexing; the
// delimiters form a second list threaded through the same items (CommonMark's delimiter stack).

type Delimiter = {
  char: string;
  count: number;
  original: number;
  canOpen: boolean;
  canClose: boolean;
  item: Item;
  previous: Delimiter | null;
  next: Delimiter | null;
};

type Item = {
  node: MarkdownInline | null;
  delimiter: Delimiter | null;
  depth: number;
  previous: Item | null;
  next: Item | null;
};

type ItemList = { head: Item | null; tail: Item | null; firstDelimiter: Delimiter | null; lastDelimiter: Delimiter | null };

function appendItem(list: ItemList, item: Item): void {
  item.previous = list.tail;
  if (list.tail) list.tail.next = item;
  else list.head = item;
  list.tail = item;
}

function unlinkItem(list: ItemList, item: Item): void {
  if (item.previous) item.previous.next = item.next;
  else list.head = item.next;
  if (item.next) item.next.previous = item.previous;
  else list.tail = item.previous;
}

function unlinkDelimiter(list: ItemList, delimiter: Delimiter): void {
  if (delimiter.previous) delimiter.previous.next = delimiter.next;
  else list.firstDelimiter = delimiter.next;
  if (delimiter.next) delimiter.next.previous = delimiter.previous;
  else list.lastDelimiter = delimiter.previous;
}

function pushText(nodes: MarkdownInline[], text: string): void {
  if (!text) return;
  const last = nodes[nodes.length - 1];
  if (last && last.type === "text") last.text += text;
  else nodes.push({ type: "text", text });
}

/** Converts items from `first` up to (not including) `stop` into nodes; leftover delimiters are literal. */
function itemsToNodes(first: Item | null, stop: Item | null): MarkdownInline[] {
  const nodes: MarkdownInline[] = [];
  for (let item = first; item && item !== stop; item = item.next) {
    if (item.delimiter) pushText(nodes, item.delimiter.char.repeat(item.delimiter.count));
    else if (item.node && item.node.type === "text") pushText(nodes, item.node.text);
    else if (item.node) nodes.push(item.node);
  }
  return nodes;
}

function processEmphasis(list: ItemList): void {
  const openersBottom = new Map<string, Delimiter | null>();
  let closer = list.firstDelimiter;
  while (closer) {
    if (!closer.canClose) {
      closer = closer.next;
      continue;
    }
    const key = `${closer.char}${closer.canOpen ? 1 : 0}${closer.original % 3}`;
    const bottom = openersBottom.get(key) ?? null;
    let opener = closer.previous;
    let found: Delimiter | null = null;
    while (opener && opener !== bottom) {
      if (opener.char === closer.char && opener.canOpen) {
        const oddMatch =
          closer.char !== "~" &&
          (closer.canOpen || opener.canClose) &&
          closer.original % 3 !== 0 &&
          (opener.original + closer.original) % 3 === 0;
        if (!oddMatch) {
          found = opener;
          break;
        }
      }
      opener = opener.previous;
    }

    let depth = 0;
    if (found) {
      for (let item = found.item.next; item && item !== closer.item; item = item.next) {
        if (item.depth > depth) depth = item.depth;
      }
      depth += 1;
      if (depth > MAX_INLINE_DEPTH) found = null;
    }

    if (!found) {
      const next = closer.next;
      openersBottom.set(key, closer.previous);
      if (!closer.canOpen) unlinkDelimiter(list, closer);
      closer = next;
      continue;
    }

    const use = closer.char === "~" ? 2 : closer.count >= 2 && found.count >= 2 ? 2 : 1;
    found.count -= use;
    closer.count -= use;
    const children = itemsToNodes(found.item.next, closer.item);
    const node: MarkdownInline =
      use === 1
        ? { type: "emphasis", children }
        : closer.char === "~"
          ? { type: "strikethrough", children }
          : { type: "strong", children };
    const wrapped: Item = { node, delimiter: null, depth, previous: found.item, next: closer.item };
    found.item.next = wrapped;
    closer.item.previous = wrapped;
    // Delimiters between the pair are now literal text inside the new node.
    found.next = closer;
    closer.previous = found;
    if (found.count === 0) {
      unlinkItem(list, found.item);
      unlinkDelimiter(list, found);
    }
    if (closer.count === 0) {
      const next = closer.next;
      unlinkItem(list, closer.item);
      unlinkDelimiter(list, closer);
      closer = next;
    }
  }
}

/** Where inline text sits: a link's text may hold images but not links, and alt text holds neither. */
type InlineContext = "top" | "link" | "alt";

function parseInline(text: string, definitions: Definitions, context: InlineContext): MarkdownInline[] {
  const spans = findCodeSpans(text);
  const brackets = findBracketPairs(text, spans);
  const list: ItemList = { head: null, tail: null, firstDelimiter: null, lastDelimiter: null };
  let buffer = "";

  const flush = () => {
    if (!buffer) return;
    appendItem(list, { node: { type: "text", text: buffer }, delimiter: null, depth: 0, previous: null, next: null });
    buffer = "";
  };
  const pushNode = (node: MarkdownInline, depth = 0) => {
    flush();
    appendItem(list, { node, delimiter: null, depth, previous: null, next: null });
  };

  const findTitle = titleFinder(text);
  const tryLink = (open: number): { label: string; url: string; title?: string; end: number } | null => {
    const close = brackets.get(open);
    if (close === undefined) return null;
    const label = text.slice(open + 1, close);
    if (text[close + 1] === "(") {
      const inline = parseInlineDestination(text, close + 2, findTitle);
      if (inline) return { label, url: inline.url, title: inline.title, end: inline.end };
    }
    if (definitions.size === 0) return null;
    let reference = label;
    let end = close + 1;
    if (text[close + 1] === "[") {
      const referenceClose = brackets.get(close + 1);
      if (referenceClose !== undefined) {
        const inner = text.slice(close + 2, referenceClose);
        if (inner.trim() !== "") reference = inner;
        end = referenceClose + 1;
      }
    }
    // Checked before normalizing, or each `[` of a deep bracket nest would normalize its whole label.
    if (reference.length > MAX_LABEL_LENGTH || reference.trim() === "") return null;
    const definition = definitions.get(normalizeLabel(reference));
    return definition ? { label, url: definition.url, title: definition.title, end } : null;
  };

  let index = 0;
  while (index < text.length) {
    const char = text[index];
    switch (char) {
      case "\\": {
        const next = text[index + 1];
        if (next === "\n") {
          buffer = trimTrailingBlanks(buffer);
          pushNode({ type: "break" });
          index = skipSpace(text, index + 2);
          continue;
        }
        if (next !== undefined && ESCAPABLE.indexOf(next) !== -1) {
          buffer += next;
          index += 2;
          continue;
        }
        buffer += char;
        index += 1;
        continue;
      }
      case "`": {
        const span = spans.get(index);
        if (span) {
          let code = text.slice(span.contentStart, span.contentEnd).replace(/\n/g, " ");
          if (code.length > 2 && code[0] === " " && code[code.length - 1] === " " && code.trim() !== "") {
            code = code.slice(1, -1);
          }
          pushNode({ type: "code", code });
          index = span.end;
          continue;
        }
        let end = index;
        while (text[end] === "`") end += 1;
        buffer += text.slice(index, end);
        index = end;
        continue;
      }
      case "\n": {
        const hard = trailingSpaceCount(buffer) >= 2;
        buffer = trimTrailingBlanks(buffer);
        pushNode(hard ? { type: "break" } : { type: "softbreak" });
        index += 1;
        while (text[index] === " " || text[index] === "\t") index += 1;
        continue;
      }
      case "*":
      case "_":
      case "~": {
        let end = index;
        while (text[end] === char) end += 1;
        const count = end - index;
        if ((char === "~" && count !== 2) || count > MAX_DELIMITER_RUN) {
          buffer += text.slice(index, end);
          index = end;
          continue;
        }
        const before = index > 0 ? text[index - 1] : undefined;
        const after = text[end];
        const leftFlanking = !isWhitespace(after) && (!isPunctuation(after) || isWhitespace(before) || isPunctuation(before));
        const rightFlanking =
          !isWhitespace(before) && (!isPunctuation(before) || isWhitespace(after) || isPunctuation(after));
        const canOpen = char === "_" ? leftFlanking && (!rightFlanking || isPunctuation(before)) : leftFlanking;
        const canClose = char === "_" ? rightFlanking && (!leftFlanking || isPunctuation(after)) : rightFlanking;
        flush();
        const item: Item = { node: null, delimiter: null, depth: 0, previous: null, next: null };
        const delimiter: Delimiter = {
          char,
          count,
          original: count,
          canOpen,
          canClose,
          item,
          previous: list.lastDelimiter,
          next: null,
        };
        item.delimiter = delimiter;
        appendItem(list, item);
        if (list.lastDelimiter) list.lastDelimiter.next = delimiter;
        else list.firstDelimiter = delimiter;
        list.lastDelimiter = delimiter;
        index = end;
        continue;
      }
      case "!":
      case "[": {
        const open = char === "!" ? index + 1 : index;
        const allowed = context === "top" || (context === "link" && char === "!");
        if (allowed && text[open] === "[") {
          const link = tryLink(open);
          if (link) {
            if (char === "!") {
              const alt = markdownInlineText(parseInline(link.label, definitions, "alt"));
              pushNode({ type: "image", url: safeExternalUrl(link.url, { allowMailto: false }), alt, title: link.title });
            } else {
              pushNode(
                {
                  type: "link",
                  url: safeExternalUrl(link.url),
                  title: link.title,
                  children: parseInline(link.label, definitions, "link"),
                },
                1,
              );
            }
            index = link.end;
            continue;
          }
        }
        buffer += char;
        index += 1;
        continue;
      }
      case "<": {
        if (context === "top") {
          const window = text.slice(index, index + 4096);
          const uri = AUTOLINK_URI.exec(window);
          const email = uri ? null : AUTOLINK_EMAIL.exec(window);
          const match = uri ?? email;
          if (match) {
            const label = match[1];
            const url = safeExternalUrl(email ? `mailto:${label}` : label);
            pushNode({ type: "link", url, children: [{ type: "text", text: label }] }, 1);
            index += match[0].length;
            continue;
          }
        }
        buffer += char;
        index += 1;
        continue;
      }
      case "&": {
        const entity = matchEntity(text, index);
        if (entity) {
          buffer += entity.value;
          index = entity.end;
          continue;
        }
        buffer += char;
        index += 1;
        continue;
      }
      default: {
        if (context === "top" && (char === "h" || char === "H" || char === "w" || char === "W")) {
          const before = index > 0 ? text[index - 1] : undefined;
          if (isWhitespace(before) || (before !== undefined && "*_~(".indexOf(before) !== -1)) {
            const literal = matchAutolinkLiteral(text, index);
            if (literal) {
              pushNode(
                { type: "link", url: safeExternalUrl(literal.url), children: [{ type: "text", text: literal.label }] },
                1,
              );
              index = literal.end;
              continue;
            }
          }
        }
        buffer += char;
        index += 1;
      }
    }
  }
  flush();
  processEmphasis(list);
  return itemsToNodes(list.head, null);
}
