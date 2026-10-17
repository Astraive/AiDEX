/**
 * Sidebar sizing for `AppShell`: when the sidebar docks beside the main region and how far it can be
 * resized. Pure functions over CSS px, mirroring the shell tokens in ../tokens/tokens.css (media
 * queries and inline styles cannot read custom properties, so the numbers live here too).
 */

/** `--aidex-rail-width`. */
export const RAIL_WIDTH = 52;
/** `--aidex-shell-pad` and `--aidex-shell-gap`; constant at every density. */
export const SHELL_PAD = 8;
export const SHELL_GAP = 8;
/** Everything across the window that is neither sidebar nor main: the rail, two pads and two gaps. */
export const SHELL_CHROME = RAIL_WIDTH + 2 * SHELL_PAD + 2 * SHELL_GAP;
/** `--aidex-main-min-width`: the narrowest main region the docked sidebar may leave. */
export const MAIN_MIN_WIDTH = 640;
/** `--aidex-sidebar-width` and its range (`--aidex-sidebar-min-width` / `-max-width`). */
export const SIDEBAR_DEFAULT_WIDTH = 256;
export const SIDEBAR_MIN_WIDTH = 208;
export const SIDEBAR_MAX_WIDTH = 480;
/** Separator key steps: an arrow moves 8px, Shift+arrow 32px. */
export const RESIZE_STEP = 8;
export const RESIZE_STEP_LARGE = 32;

/** The narrowest viewport at which a sidebar of `width` docks and main keeps `MAIN_MIN_WIDTH`: 980 at 256. */
export function dockMinViewport(width: number): number {
  return SHELL_CHROME + width + MAIN_MIN_WIDTH;
}

/**
 * The width a docked sidebar may take in `viewport`: within 208-480 and narrow enough that main keeps
 * 640px, but never below 208 (a narrower window overlays the sidebar instead).
 */
export function clampSidebarWidth(width: number, viewport: number): number {
  const max = Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, viewport - SHELL_CHROME - MAIN_MIN_WIDTH));
  const value = Number.isFinite(width) ? Math.round(width) : SIDEBAR_DEFAULT_WIDTH;
  return Math.min(max, Math.max(SIDEBAR_MIN_WIDTH, value));
}

/**
 * The width after a separator key: ArrowLeft/ArrowRight (and ArrowDown/ArrowUp) step by 8px, or 32px
 * with Shift; Home goes to the minimum and End to `max`. Null for any other key.
 */
export function resizeStep(key: string, shift: boolean, current: number, max = SIDEBAR_MAX_WIDTH): number | null {
  const upper = Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, max));
  const step = shift ? RESIZE_STEP_LARGE : RESIZE_STEP;
  let next: number;
  switch (key) {
    case "ArrowRight":
    case "ArrowUp":
      next = current + step;
      break;
    case "ArrowLeft":
    case "ArrowDown":
      next = current - step;
      break;
    case "Home":
      next = SIDEBAR_MIN_WIDTH;
      break;
    case "End":
      next = upper;
      break;
    default:
      return null;
  }
  return Math.min(upper, Math.max(SIDEBAR_MIN_WIDTH, next));
}
