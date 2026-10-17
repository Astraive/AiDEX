/**
 * The stack of open dismissable layers (dialogs, popovers, menus, selects), so nested surfaces
 * behave like Radix / Base UI layers: only the top layer answers Escape, and a pointer press inside a
 * layer opened on top of another is not "outside" the one below.
 *
 * A surface pushes its element while it is open and removes it as it starts to close. Surfaces that
 * stay in the page without registering (for example the toast region) carry the `data-aidex-layer`
 * attribute instead; a press inside such an element is not outside any layer the element does not
 * contain.
 */

/** What the stack needs from an element; a DOM element qualifies, and tests pass stand-ins. */
export type LayerElement = {
  contains(other: unknown): boolean;
};

/** Where a press landed: an element (anything with `closest`) or a node inside one, such as text. */
export type LayerTarget = {
  closest?(selector: string): LayerElement | null;
  parentElement?: LayerTarget | null;
};

export type LayerOptions = {
  /** A modal layer blocks the shell underneath, e.g. a dialog (see `hasModalLayer`). */
  modal?: boolean;
};

type Entry = { element: LayerElement; modal: boolean };

/** Marks a dismissable layer that is not in the stack. A fixed attribute name, not a token. */
export const LAYER_ATTRIBUTE = "data-aidex-layer";
/** Marks the element popovers portal into when they open inside it: the dialog popup. */
export const PORTAL_ROOT_ATTRIBUTE = "data-aidex-portal-root";

const stack: Entry[] = [];
const pushListeners = new Set<() => void>();

/** Adds a layer on top and returns its removal. Pushing an element again moves it to the top. */
export function pushLayer(element: LayerElement, options: LayerOptions = {}): () => void {
  popLayer(element);
  const entry: Entry = { element, modal: options.modal ?? false };
  stack.push(entry);
  for (const listener of [...pushListeners]) listener();
  return () => {
    const index = stack.indexOf(entry);
    if (index >= 0) stack.splice(index, 1);
  };
}

/** Removes a layer wherever it sits in the stack. */
export function popLayer(element: LayerElement): void {
  const index = stack.findIndex((entry) => entry.element === element);
  if (index >= 0) stack.splice(index, 1);
}

/** The top-most open layer, or null when none is open. */
export function topLayer(): LayerElement | null {
  return stack.at(-1)?.element ?? null;
}

/** Whether `element` is the top-most layer, i.e. the one that answers Escape. */
export function isTopLayer(element: LayerElement): boolean {
  return topLayer() === element;
}

/** Whether a modal layer (a dialog) is open; shell shortcuts such as Mod+B stand down meanwhile. */
export function hasModalLayer(): boolean {
  return stack.some((entry) => entry.modal);
}

/** The number of open layers. */
export function layerCount(): number {
  return stack.length;
}

/**
 * Calls `listener` after every push, e.g. so a non-modal popover can close when a dialog opens over it
 * (see `isBelowModalLayer`). Returns the unsubscribe.
 */
export function onLayerPush(listener: () => void): () => void {
  pushListeners.add(listener);
  return () => {
    pushListeners.delete(listener);
  };
}

/**
 * Whether a modal layer that does not contain `element` sits above it in the stack: a dialog or the
 * command palette opened over a popover elsewhere in the page, which the popover would otherwise float
 * over. A popover opened inside a dialog (its portal root) is pushed after it and is not below it.
 */
export function isBelowModalLayer(element: LayerElement): boolean {
  const index = stack.findIndex((entry) => entry.element === element);
  if (index < 0) return false;
  return stack.slice(index + 1).some((entry) => entry.modal && !entry.element.contains(element));
}

/**
 * Whether a press on `target` landed inside a layer on top of `element`, so it is not an outside
 * press for `element`: a layer pushed after it, or a `data-aidex-layer` element that neither is
 * `element`, contains it, nor sits below it in the stack.
 */
export function isInsideDescendantLayer(target: LayerTarget | null, element: LayerElement): boolean {
  if (!target) return false;
  const index = stack.findIndex((entry) => entry.element === element);
  if (index >= 0 && stack.slice(index + 1).some((entry) => entry.element.contains(target))) return true;
  const node = typeof target.closest === "function" ? target : target.parentElement;
  const marked = node?.closest?.(`[${LAYER_ATTRIBUTE}]`) ?? null;
  if (!marked || marked === element || marked.contains(element)) return false;
  const markedIndex = stack.findIndex((entry) => entry.element === marked);
  return markedIndex < 0 || markedIndex > index;
}

/** Recently focused elements, oldest first, so focus can go back to one that is still there. */
const focusHistory: Array<WeakRef<Element>> = [];
const FOCUS_HISTORY_LIMIT = 20;
let trackingFocus = false;

/** Starts recording where focus goes (once per page). Needs a DOM. */
export function trackFocusHistory(): void {
  if (trackingFocus || typeof document === "undefined") return;
  trackingFocus = true;
  document.addEventListener(
    "focusin",
    (event) => {
      if (!(event.target instanceof Element)) return;
      focusHistory.push(new WeakRef(event.target));
      if (focusHistory.length > FOCUS_HISTORY_LIMIT) focusHistory.splice(0, focusHistory.length - FOCUS_HISTORY_LIMIT);
    },
    true,
  );
}

/** The elements focused so far, newest first, that have not been collected. Needs `trackFocusHistory`. */
export function recentlyFocused(): Element[] {
  const elements: Element[] = [];
  for (let index = focusHistory.length - 1; index >= 0; index -= 1) {
    const element = focusHistory[index]?.deref();
    if (element && !elements.includes(element)) elements.push(element);
  }
  return elements;
}

/**
 * Keys that move focus themselves: Tab, and the arrow, Home/End and Page keys that composite widgets
 * (menus, toolbars, lists) move focus with, plus F6 between regions.
 */
const NAVIGATION_KEYS = new Set([
  "Tab",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "F6",
]);
/** How long after such a key a focus move still counts as caused by it (a widget may move it a frame later). */
export const NAVIGATION_FOCUS_WINDOW_MS = 500;

/** Whether `key` (a `KeyboardEvent.key`) moves focus by itself. */
export function isNavigationKey(key: string): boolean {
  return NAVIGATION_KEYS.has(key);
}

/** The last key pressed and when (`performance.now()`), cleared by a pointer press. */
let lastKey: { key: string; at: number } | null = null;
let trackingKeys = false;

/** Starts recording the last key pressed (once per page), for `focusFollowsNavigationKey`. Needs a DOM. */
export function trackNavigationKeys(): void {
  if (trackingKeys || typeof document === "undefined") return;
  trackingKeys = true;
  document.addEventListener(
    "keydown",
    (event) => {
      lastKey = { key: event.key, at: performance.now() };
    },
    true,
  );
  document.addEventListener(
    "pointerdown",
    () => {
      lastKey = null;
    },
    true,
  );
}

/** Records a key press by hand; tests use it in place of `trackNavigationKeys`. */
export function noteKeyPress(key: string | null, at = 0): void {
  lastKey = key === null ? null : { key, at };
}

/**
 * Whether focus moving at time `now` follows the user's own navigation (Tab, an arrow key…) rather
 * than a script placing it, such as a dialog's initial focus after Enter or focus going back to an
 * opener after Escape. Needs `trackNavigationKeys` (or `noteKeyPress`).
 */
export function focusFollowsNavigationKey(now: number): boolean {
  return lastKey !== null && isNavigationKey(lastKey.key) && now - lastKey.at <= NAVIGATION_FOCUS_WINDOW_MS;
}

/** Whether focus can go back to `element`: in the page, shown, not inert, enabled. Needs a DOM. */
export function canRestoreFocus(element: Element | null | undefined): element is HTMLElement {
  return (
    element instanceof HTMLElement &&
    element.isConnected &&
    !element.closest("[hidden], [inert]") &&
    !element.matches(":disabled") &&
    element.getClientRects().length > 0
  );
}

const TABBABLE =
  'a[href], area[href], button, input, select, textarea, summary, iframe, [contenteditable]:not([contenteditable="false"]), [tabindex]';

/** Whether Tab can reach `element`: enabled, not inert, rendered and visible. Needs a DOM. */
export function isTabbable(element: HTMLElement): boolean {
  if (element.tabIndex < 0 || element.hasAttribute("disabled")) return false;
  if (element.closest("[inert], [data-focus-guard], [data-base-ui-focus-guard]")) return false;
  if (element.getClientRects().length === 0) return false;
  return getComputedStyle(element).visibility !== "hidden";
}

/** The elements Tab reaches inside `root`, in document order. Needs a DOM. */
export function tabbablesIn(root: ParentNode): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(TABBABLE)).filter(isTabbable);
}

/**
 * The element Tab reaches after `anchor` in document order, skipping anything inside `exclude` (the
 * layer being left). Inside a dialog it stays within the dialog. Used so Tab out of a layer behaves
 * as if the layer sat right after its anchor. Needs a DOM.
 */
export function tabbableAfter(anchor: HTMLElement, exclude: HTMLElement): HTMLElement | null {
  const scope: ParentNode = anchor.closest(`[${PORTAL_ROOT_ATTRIBUTE}]`) ?? document;
  return (
    tabbablesIn(scope).find(
      (element) =>
        !exclude.contains(element) &&
        !anchor.contains(element) &&
        Boolean(anchor.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING),
    ) ?? null
  );
}

/** The element Shift+Tab reaches before `anchor`, or the anchor itself when it is tabbable. Needs a DOM. */
export function tabbableBefore(anchor: HTMLElement, exclude: HTMLElement): HTMLElement | null {
  if (isTabbable(anchor)) return anchor;
  const scope: ParentNode = anchor.closest(`[${PORTAL_ROOT_ATTRIBUTE}]`) ?? document;
  const previous = tabbablesIn(scope).filter(
    (element) =>
      !exclude.contains(element) && Boolean(anchor.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_PRECEDING),
  );
  return previous.at(-1) ?? null;
}
