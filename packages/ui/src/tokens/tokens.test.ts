import { describe, test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const SRC = fileURLToPath(new URL("..", import.meta.url));
const TOKENS_CSS = join(SRC, "tokens", "tokens.css");
const STYLES_CSS = join(SRC, "styles.css");

/** Files whose `"--aidex-*"` string literals are written at runtime on `<html>`. */
const RUNTIME_SETTERS = [join(SRC, "themes", "appearance.ts"), join(SRC, "themes", "accents.ts")];

/**
 * Variables a component sets inline on its own element, so they have no token default: the Popover's
 * measured space and anchor size, and BrandIcon's per-glyph color.
 */
const INLINE_COMPONENT_VARIABLES = [/^--aidex-popover-(available|anchor)-[a-z]+$/, /^--aidex-brand-color$/];

/** Families that define the visual identity. They must not drift while sizing changes. */
const IDENTITY = /^--aidex-(color|chart|font|brand)-/;

/** Declarations at 1bfb684, in order: tokens.css `:root`, then the `html[data-theme="light"]` block. */
const FROZEN_ROOT = [
  "--aidex-color-bg: #09090a",
  "--aidex-color-surface: #0b0c0d",
  "--aidex-color-surface-raised: #111417",
  "--aidex-color-surface-hover: #171a1f",
  "--aidex-color-surface-strong: #21252b",
  "--aidex-color-surface-stronger: #2e343d",
  "--aidex-color-border-subtle: #1e2126",
  "--aidex-color-border: #2a2e35",
  "--aidex-color-border-strong: #3a3f47",
  "--aidex-color-text: #f5f6f8",
  "--aidex-color-text-secondary: #b7c5da",
  "--aidex-color-text-muted: #9aa6b6",
  "--aidex-color-accent: #1a8cff",
  "--aidex-color-accent-border: color-mix(in srgb, var(--aidex-color-accent) 80%, black)",
  "--aidex-color-accent-soft: color-mix(in srgb, var(--aidex-color-accent) 12%, var(--aidex-color-bg))",
  "--aidex-color-accent-subtle-border: color-mix(in srgb, var(--aidex-color-accent) 40%, var(--aidex-color-bg))",
  "--aidex-color-on-accent: #04111d",
  "--aidex-color-success: #10c77e",
  "--aidex-color-warning: #e0a33a",
  "--aidex-color-danger: #f26d78",
  "--aidex-color-focus: color-mix(in srgb, var(--aidex-color-accent) 75%, white)",
  "--aidex-color-accent-ring: color-mix(in srgb, var(--aidex-color-accent) 20%, transparent)",
  "--aidex-color-link: color-mix(in srgb, var(--aidex-color-accent) 75%, var(--aidex-color-text))",
  "--aidex-color-shadow: color-mix(in srgb, var(--aidex-color-bg) 72%, transparent)",
  "--aidex-brand-color-strength: 100%",
  "--aidex-color-diff-bg: var(--aidex-color-surface)",
  "--aidex-color-diff-addition: var(--aidex-color-success)",
  "--aidex-color-diff-deletion: var(--aidex-color-danger)",
  "--aidex-color-diff-addition-bg: color-mix(in srgb, var(--aidex-color-diff-addition) 12%, var(--aidex-color-diff-bg))",
  "--aidex-color-diff-deletion-bg: color-mix(in srgb, var(--aidex-color-diff-deletion) 12%, var(--aidex-color-diff-bg))",
  "--aidex-color-diff-addition-emphasis: color-mix(in srgb, var(--aidex-color-diff-addition) 30%, transparent)",
  "--aidex-color-diff-deletion-emphasis: color-mix(in srgb, var(--aidex-color-diff-deletion) 30%, transparent)",
  '--aidex-font-sans: "Inter", "Segoe UI Variable Text", "Segoe UI", -apple-system, BlinkMacSystemFont, "SF Pro Text", system-ui, sans-serif',
  '--aidex-font-display: "Inter Display", "Inter", "Segoe UI Variable Display", "Segoe UI", -apple-system, BlinkMacSystemFont, "SF Pro Display", system-ui, sans-serif',
  '--aidex-font-mono: "JetBrains Mono", "Cascadia Code", "SF Mono", Consolas, monospace',
  "--aidex-chart-1: #1a8cff",
  "--aidex-chart-2: #d95926",
  "--aidex-chart-3: #199e70",
  "--aidex-chart-4: #c98500",
  "--aidex-chart-5: #d55181",
  "--aidex-chart-6: #008300",
];

const FROZEN_LIGHT = [
  "--aidex-color-bg: #f7f8fa",
  "--aidex-color-surface: #ffffff",
  "--aidex-color-surface-raised: #f0f2f5",
  "--aidex-color-surface-hover: #e7ebf0",
  "--aidex-color-surface-strong: #dde3eb",
  "--aidex-color-surface-stronger: #ced6e0",
  "--aidex-color-border-subtle: #e0e5eb",
  "--aidex-color-border: #c4ccd8",
  "--aidex-color-border-strong: #a4afbf",
  "--aidex-color-text: #151a23",
  "--aidex-color-text-secondary: #354359",
  "--aidex-color-text-muted: #56657a",
  "--aidex-color-success: #08794f",
  "--aidex-color-warning: #936010",
  "--aidex-color-danger: #b82940",
  "--aidex-color-shadow: color-mix(in srgb, var(--aidex-color-text) 16%, transparent)",
  "--aidex-brand-color-strength: 78%",
];

/** The only tokens a density block may remap. The shell frame (shell-pad, shell-gap) is deliberately absent. */
const DENSITY_TOKENS = [
  "--aidex-space-3",
  "--aidex-space-4",
  "--aidex-space-5",
  "--aidex-space-6",
  "--aidex-space-7",
  "--aidex-space-8",
  "--aidex-control-xs",
  "--aidex-control-sm",
  "--aidex-control-md",
  "--aidex-control-lg",
  "--aidex-control-xl",
  "--aidex-row-sm",
  "--aidex-row-md",
  "--aidex-row-lg",
  "--aidex-control-pad-x",
  "--aidex-control-pad-x-sm",
  "--aidex-control-pad-x-xs",
  "--aidex-page-pad-top",
  "--aidex-page-gap",
];

type Block = { selector: string; body: string; declarations: [name: string, value: string][] };

function read(path: string): string {
  return readFileSync(path, "utf8").replace(/\r\n?/g, "\n");
}

function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, "");
}

/** Innermost rule blocks with their custom property declarations (enough for these flat stylesheets). */
function blocks(css: string): Block[] {
  const result: Block[] = [];
  for (const match of stripComments(css).matchAll(/([^{}]*)\{([^{}]*)\}/g)) {
    const declarations: Block["declarations"] = [];
    for (const decl of match[2].matchAll(/(--aidex-[a-z0-9-]+)\s*:\s*([^;]+);/g)) {
      declarations.push([decl[1], decl[2].trim().replace(/\s+/g, " ")]);
    }
    result.push({ selector: match[1].trim().replace(/\s+/g, " "), body: match[2], declarations });
  }
  return result;
}

function block(css: string, selector: string): Block {
  const found = blocks(css).find((b) => b.selector === selector);
  assert.ok(found, `missing block ${selector}`);
  return found;
}

function sourceFiles(dir: string): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "node_modules") files.push(...sourceFiles(path));
    } else if (/\.(css|tsx?)$/.test(entry.name)) {
      files.push(path);
    }
  }
  return files;
}

function definedTokens(): Set<string> {
  const defined = new Set<string>();
  for (const path of [TOKENS_CSS, STYLES_CSS]) {
    for (const b of blocks(read(path))) for (const [name] of b.declarations) defined.add(name);
  }
  for (const path of RUNTIME_SETTERS) {
    for (const match of read(path).matchAll(/["'`](--aidex-[a-z0-9-]+)["'`]/g)) defined.add(match[1]);
  }
  return defined;
}

describe("tokens", () => {
  test("every var(--aidex-*) used in packages/ui/src is defined", () => {
    const defined = definedTokens();
    const missing: string[] = [];
    for (const path of sourceFiles(SRC)) {
      const text = path.endsWith(".css") ? stripComments(read(path)) : read(path);
      for (const match of text.matchAll(/var\(\s*(--aidex-[a-z0-9-]+)/g)) {
        const name = match[1];
        if (defined.has(name) || INLINE_COMPONENT_VARIABLES.some((pattern) => pattern.test(name))) continue;
        missing.push(`${relative(SRC, path).replace(/\\/g, "/")}: ${name}`);
      }
    }
    assert.deepEqual(
      [...new Set(missing)],
      [],
      "Undefined --aidex-* variables. Add the token to tokens/tokens.css (tokens group), or fix the name.",
    );
  });

  test("colour, chart, font and brand declarations match 1bfb684", () => {
    const root = block(read(TOKENS_CSS), ":root").declarations.filter(([name]) => IDENTITY.test(name));
    assert.deepEqual(
      root.map(([name, value]) => `${name}: ${value}`),
      FROZEN_ROOT,
    );
    const light = block(read(STYLES_CSS), 'html[data-theme="light"]').declarations.filter(([name]) => IDENTITY.test(name));
    assert.deepEqual(
      light.map(([name, value]) => `${name}: ${value}`),
      FROZEN_LIGHT,
    );
  });

  test("identity tokens are declared only in :root and the light theme", () => {
    const stray: string[] = [];
    for (const [path, allowed] of [
      [TOKENS_CSS, ":root"],
      [STYLES_CSS, 'html[data-theme="light"]'],
    ] as const) {
      for (const b of blocks(read(path))) {
        if (b.selector === allowed) continue;
        for (const [name] of b.declarations) if (IDENTITY.test(name)) stray.push(`${b.selector}: ${name}`);
      }
    }
    assert.deepEqual(stray, []);
  });

  test("density blocks remap only spacing, controls, rows and the page rhythm", () => {
    const css = read(STYLES_CSS);
    const compact = block(css, 'html[data-ui-density="compact"], html[data-density="compact"]:not([data-ui-density])');
    const spacious = block(css, 'html[data-ui-density="spacious"]');
    for (const b of [compact, spacious]) {
      assert.deepEqual(
        b.declarations.map(([name]) => name).sort(),
        [...DENSITY_TOKENS].sort(),
        `${b.selector} must override exactly the density tokens`,
      );
    }
    const value = (b: Block, name: string) => b.declarations.find(([n]) => n === name)?.[1];
    const root = block(read(TOKENS_CSS), ":root");
    assert.equal(value(root, "--aidex-control-md"), "32px");
    assert.equal(value(compact, "--aidex-control-md"), "28px");
    assert.equal(value(spacious, "--aidex-control-md"), "36px");
    assert.equal(value(root, "--aidex-shell-pad"), "8px");
    assert.equal(value(root, "--aidex-shell-gap"), "8px");
  });

  test("body line height stays unitless", () => {
    const body = block(read(STYLES_CSS), "body").body;
    assert.match(body, /line-height:\s*var\(--aidex-leading-body\);/);
    assert.match(body, /font-size:\s*var\(--aidex-text-md\);/);
    const leading = block(read(TOKENS_CSS), ":root").declarations.find(([name]) => name === "--aidex-leading-body");
    assert.ok(leading && /^\d+(\.\d+)?$/.test(leading[1]), "--aidex-leading-body must be a plain number");
  });

  test("runtime appearance variables have :root defaults", () => {
    const root = new Set(block(read(TOKENS_CSS), ":root").declarations.map(([name]) => name));
    for (const match of read(RUNTIME_SETTERS[0]).matchAll(/["'`](--aidex-[a-z0-9-]+)["'`]/g)) {
      assert.ok(root.has(match[1]), `${match[1]} needs a default in tokens.css`);
    }
  });
});
