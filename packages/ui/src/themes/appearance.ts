/**
 * Appearance options that apply to the whole document: enum options become `data-*` attributes on
 * `<html>` (matched by the blocks in ../styles.css) and numeric options become inline `--aidex-*`
 * custom properties. Interface scale is not here: it is native webview zoom, set by the host.
 *
 * This module has no dependency on @aidex/client; callers map the stored record to these tokens.
 */

export type UiDensity = "compact" | "comfortable" | "spacious";
export type ChatWidth = "narrow" | "comfortable" | "wide" | "full";
export type MessageSpacing = "compact" | "comfortable" | "relaxed";

export type AppearanceTokens = {
  density: UiDensity;
  chatWidth: ChatWidth;
  messageSpacing: MessageSpacing;
  /** Chat prompt and message text, CSS px 12-20. */
  chatFontSize: number;
  /** Code, diff and terminal text, CSS px 10-18. */
  codeFontSize: number;
  /** Sidebar width in CSS px 208-480, or null for the shell default (`--aidex-sidebar-width` in tokens.css). */
  sidebarWidth: number | null;
};

export const DEFAULT_APPEARANCE_TOKENS: AppearanceTokens = {
  density: "comfortable",
  chatWidth: "comfortable",
  messageSpacing: "comfortable",
  chatFontSize: 14,
  codeFontSize: 13,
  sidebarWidth: null,
};

export const CHAT_FONT_SIZE_RANGE = { min: 12, max: 20 } as const;
export const CODE_FONT_SIZE_RANGE = { min: 10, max: 18 } as const;
export const SIDEBAR_WIDTH_RANGE = { min: 208, max: 480 } as const;

/** `dataset` keys on `<html>`: `data-ui-density`, `data-chat-width` and `data-message-spacing`. */
export type AppearanceAttributes = {
  uiDensity: UiDensity;
  chatWidth: ChatWidth | null;
  messageSpacing: MessageSpacing | null;
};

/** The custom properties `applyAppearance` writes; a null value removes the inline property. */
export type AppearanceVariables = {
  "--aidex-text-chat": string;
  "--aidex-text-code": string;
  "--aidex-code-line-height": string;
  "--aidex-sidebar-width": string | null;
};

/** The parts of an element `applyAppearance` touches, so a plain object can stand in for `<html>` in tests. */
export type AppearanceRoot = {
  dataset: DOMStringMap;
  style: Pick<CSSStyleDeclaration, "setProperty" | "removeProperty">;
};

/** Code line height in px for a code font size: round(size * 1.54), so 13px code gets 20px lines. */
export function codeLineHeight(fontSize: number): number {
  return Math.round(fontSize * 1.54);
}

/**
 * Attribute values for an appearance. Density is always present, so it overrides the legacy
 * `data-density` fallback; the other attributes are null (absent) at their default.
 */
export function appearanceAttributes(t: AppearanceTokens): AppearanceAttributes {
  return {
    uiDensity: t.density,
    chatWidth: t.chatWidth === "comfortable" ? null : t.chatWidth,
    messageSpacing: t.messageSpacing === "comfortable" ? null : t.messageSpacing,
  };
}

/** Custom property values for an appearance. Out-of-range or non-finite sizes are clamped or defaulted. */
export function appearanceVariables(t: AppearanceTokens): AppearanceVariables {
  const chat = clampSize(t.chatFontSize, CHAT_FONT_SIZE_RANGE, DEFAULT_APPEARANCE_TOKENS.chatFontSize);
  const code = clampSize(t.codeFontSize, CODE_FONT_SIZE_RANGE, DEFAULT_APPEARANCE_TOKENS.codeFontSize);
  const sidebar =
    t.sidebarWidth === null || !Number.isFinite(t.sidebarWidth)
      ? null
      : clampSize(t.sidebarWidth, SIDEBAR_WIDTH_RANGE, SIDEBAR_WIDTH_RANGE.min);
  return {
    "--aidex-text-chat": `${chat}px`,
    "--aidex-text-code": `${code}px`,
    "--aidex-code-line-height": `${codeLineHeight(code)}px`,
    "--aidex-sidebar-width": sidebar === null ? null : `${sidebar}px`,
  };
}

/**
 * Applies an appearance to `<html>`. It writes only its own dataset keys and custom properties, one at
 * a time, and never replaces `style.cssText`, so `applyAccent`'s inline accent and the theme, legacy
 * density, motion and keyboard-hint attributes on the same element are left alone.
 */
export function applyAppearance(t: AppearanceTokens, root: AppearanceRoot = document.documentElement): void {
  for (const [key, value] of Object.entries(appearanceAttributes(t))) {
    if (value === null) delete root.dataset[key];
    else root.dataset[key] = value;
  }
  for (const [name, value] of Object.entries(appearanceVariables(t))) {
    if (value === null) root.style.removeProperty(name);
    else root.style.setProperty(name, value);
  }
}

function clampSize(value: number, range: { min: number; max: number }, fallback: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.min(range.max, Math.max(range.min, Math.round(value)));
}
