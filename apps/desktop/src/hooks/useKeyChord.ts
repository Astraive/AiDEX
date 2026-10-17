import { useEffect, useRef } from "react";
import { hasModalLayer } from "@aidex/ui/primitives";
import { matchesKeyChord, parseKeyChord } from "../lib/keyChord";
import { isApplePlatform } from "../lib/platform";

/**
 * Runs `handler` when a chord such as "Mod+Shift+M" is pressed anywhere in the window. Malformed or
 * empty chords bind nothing. The handler may change between renders without rebinding. While a modal
 * layer (a dialog, the command palette, the image viewer) is open the chord is swallowed and the
 * handler does not run, so nothing acts behind the modal or pulls focus out of it.
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
      if (hasModalLayer()) return;
      handlerRef.current();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [chord, enabled]);
}
