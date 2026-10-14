import { useEffect, useRef } from "react";
import { windowPresence, type PollOptions } from "./presence";

/**
 * Calls `poll` every `intervalMs` while `enabled`, but only while the user is present (`presence.ts`):
 * the window is visible and had input in the last minute. When the user comes back, it polls at once.
 * `whileAway` keeps a visible window polling without input, for reads tied to work the daemon already
 * holds the vault open for (an executing run). Use it for every timer that reads through the daemon,
 * so an unattended window never holds off the vault's idle auto-lock.
 */
export function usePoll(
  poll: () => void,
  intervalMs: number,
  options: PollOptions & { enabled?: boolean } = {},
): void {
  const { enabled = true, whileAway = false } = options;
  const pollRef = useRef(poll);
  pollRef.current = poll;
  const whileAwayRef = useRef(whileAway);
  whileAwayRef.current = whileAway;

  useEffect(() => {
    if (!enabled) return;
    const presence = windowPresence();
    const timer = setInterval(() => {
      if (presence.pollAllowed({ whileAway: whileAwayRef.current })) pollRef.current();
    }, intervalMs);
    const stop = presence.onReturn(() => pollRef.current());
    return () => {
      clearInterval(timer);
      stop();
    };
  }, [enabled, intervalMs]);
}
