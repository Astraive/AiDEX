// Pure appearance helpers for the AppearanceProvider: reading a saved record leniently, stepping the
// interface scale, mapping the record to `@aidex/ui` tokens, and recognising the zoom keys.

import {
  APPEARANCE_DEFAULTS,
  APPEARANCE_LIMITS,
  UI_SCALE_STEPS,
  type Appearance,
  type AppPreferences,
  type ChatWidth,
  type MessageSpacing,
  type UiDensity,
} from "@aidex/client/dto/preferences";
import type { AppearanceTokens } from "@aidex/ui/themes";

/** Density stored in the base preferences, which older Core versions also read and write. */
export type LegacyDensity = AppPreferences["density"];

export type ZoomAction = "in" | "out" | "reset";

/** The parts of a `KeyboardEvent` the zoom keys look at. */
export type ZoomKeyEvent = {
  key: string;
  code: string;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  /** True while an input method is composing text; those keystrokes belong to the IME. */
  isComposing?: boolean;
};

const DENSITIES: readonly UiDensity[] = ["compact", "comfortable", "spacious"];
const LEGACY_DENSITIES: readonly LegacyDensity[] = ["compact", "comfortable"];
const CHAT_WIDTHS: readonly ChatWidth[] = ["narrow", "comfortable", "wide", "full"];
const MESSAGE_SPACINGS: readonly MessageSpacing[] = ["compact", "comfortable", "relaxed"];

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value) ? (value as T) : fallback;
}

function clampWhole(value: unknown, range: { min: number; max: number }, fallback: number): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(range.max, Math.max(range.min, Math.round(value)));
}

/**
 * The appearance a Core without `appearance.*` implies: every default, with the density from the
 * base preferences (compact or comfortable).
 */
export function legacyAppearance(legacyDensity?: LegacyDensity | null): Appearance {
  return { ...APPEARANCE_DEFAULTS, density: oneOf(legacyDensity, LEGACY_DENSITIES, APPEARANCE_DEFAULTS.density) };
}

/**
 * Reads an `appearance.get` result leniently: numbers are clamped to their ranges, unknown enum
 * values and wrong types fall back to the default, and a missing record (`null`/`undefined`, i.e. the
 * method is not there) gives `legacyAppearance(legacyDensity)`. An unreadable density falls back to the
 * legacy one too.
 */
export function normalizeAppearance(raw: unknown, legacyDensity?: LegacyDensity | null): Appearance {
  const fallback = legacyAppearance(legacyDensity);
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return fallback;
  const record = raw as Record<string, unknown>;
  const width = record.sidebar_width;
  return {
    ui_scale: clampWhole(record.ui_scale, APPEARANCE_LIMITS.ui_scale, APPEARANCE_DEFAULTS.ui_scale),
    density: oneOf(record.density, DENSITIES, fallback.density),
    chat_width: oneOf(record.chat_width, CHAT_WIDTHS, APPEARANCE_DEFAULTS.chat_width),
    chat_font_size: clampWhole(record.chat_font_size, APPEARANCE_LIMITS.chat_font_size, APPEARANCE_DEFAULTS.chat_font_size),
    code_font_size: clampWhole(record.code_font_size, APPEARANCE_LIMITS.code_font_size, APPEARANCE_DEFAULTS.code_font_size),
    sidebar_width:
      typeof width === "number" && Number.isFinite(width)
        ? clampWhole(width, APPEARANCE_LIMITS.sidebar_width, APPEARANCE_LIMITS.sidebar_width.min)
        : null,
    sidebar_auto_collapse:
      typeof record.sidebar_auto_collapse === "boolean"
        ? record.sidebar_auto_collapse
        : APPEARANCE_DEFAULTS.sidebar_auto_collapse,
    message_spacing: oneOf(record.message_spacing, MESSAGE_SPACINGS, APPEARANCE_DEFAULTS.message_spacing),
  };
}

/** The interface scale step after `current` in `UI_SCALE_STEPS`; it stays at 125 and 75 at the ends. */
export function nextScaleStep(current: number, direction: "in" | "out"): number {
  if (direction === "in") return UI_SCALE_STEPS.find((step) => step > current) ?? UI_SCALE_STEPS[UI_SCALE_STEPS.length - 1];
  for (let index = UI_SCALE_STEPS.length - 1; index >= 0; index--) {
    if (UI_SCALE_STEPS[index] < current) return UI_SCALE_STEPS[index];
  }
  return UI_SCALE_STEPS[0];
}

/** The interface scale a zoom key leads to from `current`. */
export function zoomScale(current: number, action: ZoomAction): number {
  return action === "reset" ? APPEARANCE_DEFAULTS.ui_scale : nextScaleStep(current, action);
}

/** Maps the stored record to the tokens `applyAppearance` writes on `<html>`. */
export function toAppearanceTokens(appearance: Appearance): AppearanceTokens {
  return {
    density: appearance.density,
    chatWidth: appearance.chat_width,
    messageSpacing: appearance.message_spacing,
    chatFontSize: appearance.chat_font_size,
    codeFontSize: appearance.code_font_size,
    sidebarWidth: appearance.sidebar_width,
  };
}

/** Whether every field equals its default (`APPEARANCE_DEFAULTS`). */
export function isDefaultAppearance(appearance: Appearance): boolean {
  return (Object.keys(APPEARANCE_DEFAULTS) as (keyof Appearance)[]).every(
    (key) => appearance[key] === APPEARANCE_DEFAULTS[key],
  );
}

/**
 * The zoom action for a key press, or null. Mod (Ctrl, or ⌘ on Apple platforms) must be held,
 * without Alt (so AltGr characters pass through) and outside IME composition. It matches the
 * produced `key`, so layouts that type `+` without Shift (German, Nordic) work: `=` and `+` zoom in,
 * `-` and `_` zoom out, `0` resets. Numpad + and − match by `code` too; Numpad 0 matches only as
 * `0`, since with Num Lock off it is Insert and Ctrl+Insert is copy.
 */
export function zoomKeyAction(event: ZoomKeyEvent, apple: boolean): ZoomAction | null {
  if (event.isComposing || event.altKey) return null;
  const mod = apple ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  if (!mod) return null;
  if (event.code === "NumpadAdd") return "in";
  if (event.code === "NumpadSubtract") return "out";
  switch (event.key) {
    case "=":
    case "+":
      return "in";
    case "-":
    case "_":
      return "out";
    case "0":
      return "reset";
    default:
      return null;
  }
}
