// Whether someone is using the window, which decides the polls that may run. Most screen polls read
// encrypted records. AiDEX Core does not count its background status, list and view reads as vault
// use, but other reads restart the vault's idle auto-lock, and an unattended window has no reason to
// keep decrypting records. So those polls stop once the window is hidden or has had no input for
// `AWAY_AFTER_MS`, and catch up as soon as the user is back. The tracker is pure (clock and visibility are injected) so the rules are tested without a DOM;
// `windowPresence` wires one to the window and `usePoll` uses it.

/** No keyboard, pointer, wheel, touch, or focus input for this long counts as away. */
export const AWAY_AFTER_MS = 60_000;

export type PollOptions = {
  /**
   * Keep polling while the user is away (never while the window is hidden). Only for reads tied to
   * work the daemon already holds the vault open for, such as an executing run; a run waiting for an
   * approval does not count.
   */
  whileAway?: boolean;
};

export type PresenceTracker = {
  /** Keyboard, pointer, wheel, touch, or focus input arrived. */
  input(): void;
  /** The window's visibility changed; becoming visible counts as input. */
  visibilityChanged(): void;
  /** The window is visible and had input within the away period. */
  present(): boolean;
  /** Whether a poll tick may read now. */
  pollAllowed(options?: PollOptions): boolean;
  /** Calls `listener` whenever the user comes back (input after being away, or the window shown again). */
  onReturn(listener: () => void): () => void;
};

export function createPresenceTracker(clock: {
  now: () => number;
  visible: () => boolean;
  awayAfterMs?: number;
}): PresenceTracker {
  const awayAfter = clock.awayAfterMs ?? AWAY_AFTER_MS;
  let lastInput = clock.now();
  let wasVisible = clock.visible();
  const returned = new Set<() => void>();
  const present = () => clock.visible() && clock.now() - lastInput < awayAfter;
  const notify = () => {
    for (const listener of [...returned]) listener();
  };
  return {
    input() {
      const away = !present();
      lastInput = clock.now();
      if (away && clock.visible()) notify();
    },
    visibilityChanged() {
      const visible = clock.visible();
      const shown = visible && !wasVisible;
      wasVisible = visible;
      if (!shown) return;
      lastInput = clock.now();
      notify();
    },
    present,
    pollAllowed(options = {}) {
      if (!clock.visible()) return false;
      return options.whileAway === true || present();
    },
    onReturn(listener) {
      returned.add(listener);
      return () => {
        returned.delete(listener);
      };
    },
  };
}

const INPUT_EVENTS = ["pointerdown", "pointermove", "keydown", "wheel", "touchstart", "focus"] as const;

let windowTracker: PresenceTracker | undefined;

const documentVisible = () => typeof document === "undefined" || document.visibilityState !== "hidden";

/**
 * The window's tracker. Its listeners are installed on first use and stay for the window's life, so
 * input on screens without polls counts too. Outside a browser (static rendering) it reports a
 * visible window.
 */
export function windowPresence(): PresenceTracker {
  if (windowTracker) return windowTracker;
  const tracker = createPresenceTracker({ now: () => Date.now(), visible: documentVisible });
  windowTracker = tracker;
  if (typeof window !== "undefined") {
    const onInput = () => tracker.input();
    for (const name of INPUT_EVENTS) window.addEventListener(name, onInput, { capture: true, passive: true });
    document.addEventListener("visibilitychange", () => tracker.visibilityChanged());
  }
  return tracker;
}
