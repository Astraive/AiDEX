import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { call } from "@aidex/client/rpc";
import { ErrorKind, toAidexError } from "@aidex/client/errors";
import type { ModelPreferences } from "@aidex/client/dto/preferences";
import { useToast } from "../../../app/providers/toast";
import { useRpc } from "../../../hooks/useRpc";

/** Bounds enforced by the daemon (`aidex-preferences::ModelPreferences`). */
const MAX_FAVORITES = 64;
const MAX_HIDDEN = 512;
const EMPTY: ModelPreferences = { favorite_models: [], hidden_models: [] };

export type ModelPreferencesState = {
  favorites: ReadonlySet<string>;
  hidden: ReadonlySet<string>;
  toggleFavorite: (key: string) => void;
  setHidden: (key: string, hidden: boolean) => void;
  /** Why favorites and hidden models cannot be read or saved right now, if they cannot. */
  error: string | null;
  /** Read the saved lists again (for example after the vault was unlocked elsewhere). */
  reload: () => void;
};

/**
 * Model picker favorites and hidden models (`model.preferences.*`). Changes apply at once and roll
 * back with a toast when the daemon refuses them; an older daemon without the methods reads as empty.
 * `model.preferences.set` replaces a whole list, so a change is only ever built from lists the daemon
 * returned: while the first read is pending or failed, a change reads them first.
 */
export function useModelPreferences(): ModelPreferencesState {
  const notify = useToast();
  const query = useRpc("model.preferences.get");
  const [local, setLocal] = useState<ModelPreferences | null>(null);
  // A fresh answer from the daemon replaces optimistic state.
  useEffect(() => setLocal(null), [query.data]);
  const current = local ?? query.data ?? EMPTY;
  const currentRef = useRef(current);
  currentRef.current = current;
  // Whether `current` came from the daemon rather than the EMPTY placeholder.
  const known = local !== null || query.data !== undefined;
  const knownRef = useRef(known);
  knownRef.current = known;
  const baseRequest = useRef<Promise<void> | null>(null);
  const unsupported = query.error?.kind === ErrorKind.methodNotFound;
  const refetch = query.refetch;

  /** Runs `apply` once `currentRef` holds the daemon's lists, reading them first when needed. */
  const withBase = useCallback(
    (apply: () => void) => {
      if (unsupported) {
        notify("Favorites and hidden models need a newer AiDEX Core.", "danger");
        return;
      }
      if (knownRef.current) {
        apply();
        return;
      }
      // Concurrent changes share one read and then apply in order on top of each other.
      baseRequest.current ??= call("model.preferences.get")
        .then((fresh) => {
          currentRef.current = fresh;
          knownRef.current = true;
          setLocal(fresh);
          refetch();
        })
        .finally(() => {
          baseRequest.current = null;
        });
      baseRequest.current.then(apply, (caught: unknown) =>
        notify(`Model preferences could not be read: ${toAidexError(caught).message}`, "danger"),
      );
    },
    [notify, unsupported, refetch],
  );

  const save = useCallback(
    (next: ModelPreferences, patch: Partial<ModelPreferences>) => {
      const previous = currentRef.current;
      currentRef.current = next;
      setLocal(next);
      call("model.preferences.set", patch).then(
        (saved) => {
          currentRef.current = saved;
          setLocal(saved);
        },
        (caught: unknown) => {
          currentRef.current = previous;
          setLocal(previous);
          notify(`Model preferences were not saved: ${toAidexError(caught).message}`, "danger");
          refetch();
        },
      );
    },
    [notify, refetch],
  );

  const toggleFavorite = useCallback(
    (key: string) =>
      withBase(() => {
        const before = currentRef.current;
        const has = before.favorite_models.includes(key);
        const favorite_models = has
          ? before.favorite_models.filter((entry) => entry !== key)
          : [...before.favorite_models, key];
        if (favorite_models.length > MAX_FAVORITES) {
          notify(`You can keep up to ${MAX_FAVORITES} favorite models.`, "danger");
          return;
        }
        save({ ...before, favorite_models }, { favorite_models });
      }),
    [notify, save, withBase],
  );

  const setHidden = useCallback(
    (key: string, hide: boolean) =>
      withBase(() => {
        const before = currentRef.current;
        if (before.hidden_models.includes(key) === hide) return;
        const hidden_models = hide
          ? [...before.hidden_models, key]
          : before.hidden_models.filter((entry) => entry !== key);
        if (hidden_models.length > MAX_HIDDEN) {
          notify(`You can hide up to ${MAX_HIDDEN} models.`, "danger");
          return;
        }
        save({ ...before, hidden_models }, { hidden_models });
      }),
    [notify, save, withBase],
  );

  const favorites = useMemo(() => new Set(current.favorite_models), [current.favorite_models]);
  const hidden = useMemo(() => new Set(current.hidden_models), [current.hidden_models]);
  // Lists read by a later change replace a stale read error.
  const error = unsupported || local !== null ? null : (query.error?.message ?? null);
  return useMemo(
    () => ({ favorites, hidden, toggleFavorite, setHidden, error, reload: refetch }),
    [favorites, hidden, toggleFavorite, setHidden, error, refetch],
  );
}
