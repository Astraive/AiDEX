import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { setUiScale } from "@aidex/client/desktop";
import { APPEARANCE_DEFAULTS, type Appearance, type AppearancePatch } from "@aidex/client/dto/preferences";
import { ErrorKind, toAidexError } from "@aidex/client/errors";
import { call } from "@aidex/client/rpc";
import { applyAppearance } from "@aidex/ui/themes";
import { useRpc } from "../../hooks/useRpc";
import { normalizeAppearance, toAppearanceTokens, zoomScale, type ZoomAction } from "./appearanceState";
import { usePreferences } from "./preferences";
import { useZoomKeys } from "./zoomKeys";

export type AppearanceState = {
  /** The appearance in effect: the saved record with changes that are still being saved on top. */
  appearance: Appearance;
  /**
   * True once the saved appearance is known, or Core is too old to have one. Until then nothing is
   * applied to `<html>` and the legacy `data-density` from the base preferences stays in charge.
   */
  ready: boolean;
  /** Core has no `appearance.*`: only density can change (compact or comfortable, via `settings.set`). */
  unsupported: boolean;
  /** True while a change is being saved. */
  saving: boolean;
  /** Why the appearance could not be read or saved, if it could not. */
  error: string | null;
  /**
   * Applies a change at once and saves it. A refused change is rolled back and sets `error`; the
   * promise never rejects. `{ sidebar_width: null }` returns the sidebar to the shell default.
   */
  update: (patch: AppearancePatch) => Promise<void>;
  /** Restores every appearance default. Accent, theme, and the motion and hint switches are kept. */
  reset: () => Promise<void>;
};

/** Zoom keys apply at once and are saved this long after the last press. */
const ZOOM_SAVE_DELAY_MS = 500;

const AppearanceContext = createContext<AppearanceState | null>(null);

/**
 * Reads and saves the appearance record (`appearance.get|set`), applies it to `<html>` through
 * `applyAppearance` and the interface scale through the host's `set_ui_scale`, and binds the zoom
 * keys. Mount it inside `PreferencesProvider`.
 */
export function AppearanceProvider({ children }: { children: ReactNode }) {
  const { settings, updateSettings } = usePreferences();
  const query = useRpc("appearance.get");
  const unsupported = query.error?.kind === ErrorKind.methodNotFound;
  const refetch = query.refetch;
  // Optimistic record while changes are saved; a fresh answer from Core replaces it.
  const [local, setLocal] = useState<Appearance | null>(null);
  useEffect(() => setLocal(null), [query.data]);
  // Scale chosen with the zoom keys and not saved yet.
  const [zoom, setZoom] = useState<number | null>(null);
  const [pending, setPending] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const saved = useMemo(
    () => normalizeAppearance(unsupported ? null : query.data, settings.density),
    [unsupported, query.data, settings.density],
  );
  const base = !unsupported && local ? local : saved;
  const appearance = useMemo(() => (zoom === null ? base : { ...base, ui_scale: zoom }), [base, zoom]);
  const ready = unsupported || query.data !== undefined || local !== null;

  const baseRef = useRef(base);
  baseRef.current = base;
  const appearanceRef = useRef(appearance);
  appearanceRef.current = appearance;
  const latestRequest = useRef(0);
  const zoomTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const zoomRef = useRef<number | null>(null);

  const cancelZoom = useCallback(() => {
    if (zoomTimer.current !== null) clearTimeout(zoomTimer.current);
    zoomTimer.current = null;
    zoomRef.current = null;
    setZoom(null);
  }, []);

  const update = useCallback(
    async (patch: AppearancePatch) => {
      if ("ui_scale" in patch) cancelZoom();
      setError(null);
      if (unsupported) {
        // An older Core keeps density in the base preferences only, and has no spacious.
        if (patch.density === undefined || patch.density === "spacious") return;
        setPending((count) => count + 1);
        try {
          await updateSettings({ density: patch.density });
        } catch (caught) {
          setError(`Density was not saved: ${toAidexError(caught).message}`);
        } finally {
          setPending((count) => count - 1);
        }
        return;
      }
      const request = ++latestRequest.current;
      const previous = baseRef.current;
      const next = { ...previous, ...patch };
      baseRef.current = next;
      setLocal(next);
      setPending((count) => count + 1);
      try {
        const result = normalizeAppearance(await call("appearance.set", patch));
        if (request === latestRequest.current) {
          baseRef.current = result;
          setLocal(result);
        }
      } catch (caught) {
        setError(`Appearance was not saved: ${toAidexError(caught).message}`);
        if (request === latestRequest.current) {
          baseRef.current = previous;
          setLocal(previous);
        }
        refetch();
      } finally {
        setPending((count) => count - 1);
      }
    },
    [cancelZoom, unsupported, updateSettings, refetch],
  );

  const reset = useCallback(
    () => update(unsupported ? { density: APPEARANCE_DEFAULTS.density } : { ...APPEARANCE_DEFAULTS }),
    [unsupported, update],
  );

  const onZoom = useCallback(
    (action: ZoomAction) => {
      const next = zoomScale(appearanceRef.current.ui_scale, action);
      zoomRef.current = next;
      setZoom(next);
      if (zoomTimer.current !== null) clearTimeout(zoomTimer.current);
      zoomTimer.current = setTimeout(() => {
        zoomTimer.current = null;
        void update({ ui_scale: next });
      }, ZOOM_SAVE_DELAY_MS);
    },
    [update],
  );
  useZoomKeys(onZoom, { enabled: ready && !unsupported });

  // A zoom still waiting to be saved is saved when the provider goes away.
  useEffect(
    () => () => {
      if (zoomTimer.current === null || zoomRef.current === null) return;
      clearTimeout(zoomTimer.current);
      void call("appearance.set", { ui_scale: zoomRef.current }).catch(() => undefined);
    },
    [],
  );

  // `applyAppearance` writes only its own attributes and properties, so re-applying is cheap and safe.
  useLayoutEffect(() => {
    if (ready) applyAppearance(toAppearanceTokens(appearance));
  }, [ready, appearance]);
  const uiScale = appearance.ui_scale;
  useEffect(() => {
    // Outside the app (a browser build) there is no host to zoom; the scale then stays at 100%.
    if (ready) setUiScale(uiScale).catch(() => undefined);
  }, [ready, uiScale]);

  const readError = unsupported ? null : (query.error?.message ?? null);
  const value = useMemo<AppearanceState>(
    () => ({
      appearance,
      ready,
      unsupported,
      saving: pending > 0,
      error: error ?? readError,
      update,
      reset,
    }),
    [appearance, ready, unsupported, pending, error, readError, update, reset],
  );
  return <AppearanceContext.Provider value={value}>{children}</AppearanceContext.Provider>;
}

export function useAppearance(): AppearanceState {
  const appearance = useContext(AppearanceContext);
  if (!appearance) throw new Error("useAppearance must be used inside AppearanceProvider");
  return appearance;
}
