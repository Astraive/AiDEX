import type { AidexError } from "@aidex/client/errors";

/** What `useBridgeQuery` holds for the key it last loaded. */
export type QueryState<T> = { key: string; data?: T; error: AidexError | null; settled: boolean };

/** A load for `key` began: a refetch of the same key keeps its data on screen, a new key starts empty. */
export function queryStarted<T>(previous: QueryState<T>, key: string): QueryState<T> {
  return { key, data: previous.key === key ? previous.data : undefined, error: null, settled: false };
}

export function querySucceeded<T>(key: string, data: T): QueryState<T> {
  return { key, data, error: null, settled: true };
}

/**
 * A failed load keeps no data, even for the same key: after the vault locks, decrypted records
 * must not stay on screen next to the `vault_locked` error.
 */
export function queryFailed<T>(key: string, error: AidexError): QueryState<T> {
  return { key, error, settled: true };
}

/**
 * Coalesces refetches while a load is in flight. A refetch that arrives during a load is queued and
 * runs once that load settles, however many arrived, so polling or event-driven refreshes faster
 * than a slow read never discard every answer before it can be shown.
 */
export type RefetchScheduler = {
  /** A load started; a refetch queued for the previous load is now covered by it. */
  start(): void;
  /** Asked to refetch: true to load now, false when queued behind the load in flight. */
  request(): boolean;
  /** The load in flight settled: true when a queued refetch should load now. */
  finish(): boolean;
  /** The load was abandoned (new key, disabled, unmounted): nothing is in flight or queued. */
  reset(): void;
};

export function createRefetchScheduler(): RefetchScheduler {
  let pending = false;
  let queued = false;
  return {
    start() {
      pending = true;
      queued = false;
    },
    request() {
      if (!pending) return true;
      queued = true;
      return false;
    },
    finish() {
      const rerun = pending && queued;
      pending = false;
      queued = false;
      return rerun;
    },
    reset() {
      pending = false;
      queued = false;
    },
  };
}
