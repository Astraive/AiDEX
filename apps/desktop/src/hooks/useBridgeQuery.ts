import { useCallback, useEffect, useRef, useState } from "react";
import { toAidexError, type AidexError } from "@aidex/client/errors";
import {
  createRefetchScheduler,
  queryFailed,
  queryStarted,
  querySucceeded,
  type QueryState,
} from "./bridgeQueryState";

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
 * are dropped, so a slow answer never overwrites a newer one. Refetches that arrive while a load
 * is in flight are coalesced into one load after it settles, so a read slower than its poll still
 * lands. A failed load keeps no data, and disabled queries keep none either.
 */
export function useBridgeQuery<T>(load: () => Promise<T>, key: string, enabled = true): BridgeQuery<T> {
  const [state, setState] = useState<QueryState<T>>({ key: "", error: null, settled: false });
  const [tick, setTick] = useState(0);
  const [scheduler] = useState(createRefetchScheduler);
  const loadRef = useRef(load);
  loadRef.current = load;

  useEffect(() => {
    if (!enabled) return;
    let current = true;
    scheduler.start();
    setState((previous) => queryStarted(previous, key));
    loadRef
      .current()
      .then(
        (data) => current && setState(querySucceeded(key, data)),
        (error: unknown) => current && setState(queryFailed<T>(key, toAidexError(error))),
      )
      .finally(() => {
        if (current && scheduler.finish()) setTick((value) => value + 1);
      });
    return () => {
      current = false;
      scheduler.reset();
    };
  }, [key, enabled, tick, scheduler]);

  const refetch = useCallback(() => {
    if (scheduler.request()) setTick((value) => value + 1);
  }, [scheduler]);
  const fresh = enabled && state.key === key;
  return {
    data: fresh ? state.data : undefined,
    error: fresh ? state.error : null,
    loading: enabled && (!fresh || !state.settled),
    settled: fresh && state.settled,
    refetch,
  };
}
