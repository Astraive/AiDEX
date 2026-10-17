import { useCallback, useSyncExternalStore } from "react";

/**
 * Whether a CSS media query such as `(min-width: 980px)` matches, updating as the window (or its
 * zoom) changes. Prerendering has no window and reports a match, so the wide layout is the static one.
 */
export function useMediaQuery(query: string): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const list = window.matchMedia(query);
      list.addEventListener("change", onChange);
      return () => list.removeEventListener("change", onChange);
    },
    [query],
  );
  const read = useCallback(() => window.matchMedia(query).matches, [query]);
  return useSyncExternalStore(subscribe, read, matchOnServer);
}

function matchOnServer(): boolean {
  return true;
}
