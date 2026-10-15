import { useEffect, useRef } from "react";
import { isApplePlatform } from "../../lib/platform";
import { zoomKeyAction, type ZoomAction } from "./appearanceState";

/**
 * Calls `onZoom` for Mod+= / Mod++ (in), Mod+- (out) and Mod+0 (reset) anywhere in the window, and
 * stops the webview's own handling of those keys. Matching is by the produced key (see
 * `zoomKeyAction`), so it works on layouts where `+` needs no Shift. `useKeyChord` cannot express
 * these chords ("Mod++" splits on "+"), and they must not be added to `AppPreferences.shortcuts`.
 * The handler may change between renders without rebinding.
 */
export function useZoomKeys(onZoom: (action: ZoomAction) => void, options: { enabled?: boolean } = {}) {
  const handlerRef = useRef(onZoom);
  handlerRef.current = onZoom;
  const enabled = options.enabled ?? true;
  useEffect(() => {
    if (!enabled) return;
    const apple = isApplePlatform();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.defaultPrevented) return;
      const action = zoomKeyAction(event, apple);
      if (!action) return;
      event.preventDefault();
      handlerRef.current(action);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [enabled]);
}
