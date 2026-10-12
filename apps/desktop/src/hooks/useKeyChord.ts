import { useEffect, useRef } from "react";
import { matchesKeyChord, parseKeyChord } from "../lib/keyChord";
import { isApplePlatform } from "../lib/platform";

/**
 * Runs `handler` when a chord such as "Mod+Shift+M" is pressed anywhere in the window. Malformed or
 * empty chords bind nothing. The handler may change between renders without rebinding.
 */
export function useKeyChord(
  chord: string | null | undefined,
  handler: () => void,
  options: { enabled?: boolean } = {},
) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;
  const enabled = options.enabled ?? true;
  useEffect(() => {
    if (!enabled || !chord) return;
    const parsed = parseKeyChord(chord);
    if (!parsed) return;
    const apple = isApplePlatform();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.repeat || event.defaultPrevented) return;
      if (!matchesKeyChord(event, parsed, apple)) return;
      event.preventDefault();
      handlerRef.current();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [chord, enabled]);
}
