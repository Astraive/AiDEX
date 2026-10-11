import { useCallback, useEffect, useRef, useState } from "react";
import { toAidexError, type AidexError } from "@aidex/client/errors";

export type BridgeQuery<T> = {
  data: T | undefined;
  error: AidexError | null;
  /** True while a request for the current key is in flight. */
  loading: boolean;
  /** True once the current key has settled (with data or an error). */
  settled: boolean;
  refetch: () => void;
};

/**
 * Run a bridge command whenever `key` changes (or on `refetch`). Results for an outdated key
 * are dropped, so a slow answer never overwrites a newer one. Disabled queries keep no data.
 */
export function useBridgeQuery<T>(
  load: () => Promise<T>,
  key: string,
  enabled = true,
): BridgeQuery<T> {
  const [state, setState] = useState<{
    key: string;
    data?: T;
    error: AidexError | null;
    settled: boolean;
  }>({
    key: "",
    error: null,
    settled: false,
  });
  const [tick, setTick] = useState(0);
  const loadRef = useRef(load);
  const pendingRef = useRef(false);
  const queuedRef = useRef(false);
  loadRef.current = load;

  useEffect(() => {
    if (!enabled) return;
    let current = true;
    pendingRef.current = true;
    queuedRef.current = false;
    setState((previous) => ({
      key,
      data: previous.key === key ? previous.data : undefined,
      error: null,
      settled: false,
    }));
    loadRef
      .current()
      .then(
        (data) =>
          current && setState({ key, data, error: null, settled: true }),
        (error: unknown) =>
          current &&
          setState((previous) => ({
            key,
            data: previous.key === key ? previous.data : undefined,
            error: toAidexError(error),
            settled: true,
          })),
      )
      .finally(() => {
        if (!current) return;
        pendingRef.current = false;
        if (queuedRef.current) {
          queuedRef.current = false;
          setTick((value) => value + 1);
        }
      });
    return () => {
      current = false;
    };
  }, [key, enabled, tick]);

  const refetch = useCallback(() => {
    // Coalesce stream/poll refreshes while a slow read is in flight; repeated
    // refreshes must not invalidate every response before it can be displayed.
    if (pendingRef.current) queuedRef.current = true;
    else setTick((value) => value + 1);
  }, []);
  const fresh = enabled && state.key === key;
  return {
    data: fresh ? state.data : undefined,
    error: fresh ? state.error : null,
    loading: enabled && (!fresh || !state.settled),
    settled: fresh && state.settled,
    refetch,
  };
}
