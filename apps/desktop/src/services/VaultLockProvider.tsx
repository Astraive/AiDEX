import { createContext, useEffect, useState, type ReactNode } from "react";
import { toAidexError } from "@aidex/client/errors";
import { call, subscribe } from "@aidex/client/rpc";
import { useAidexStatus } from "../hooks/useAidexStatus";
import { cursorUnusable, reconnectDelay } from "../hooks/sessionEvents";
import { followVaultLocks, VAULT_LOCK_FILTER } from "../hooks/vaultLocks";

/** How many times the vault locked while the workspace was open; read it with `useVaultLocks()`. */
export const VaultLockContext = createContext(0);

/** A stream that stayed up this long resets the reconnect backoff. */
const STABLE_STREAM_MS = 10_000;

/**
 * Follows the drive-wide `vault.locked` event for every screen of the workspace, with one
 * `events.subscribe` stream (filtered to that kind, starting at `events.latest`). A lock from outside
 * the app (the idle auto-lock, `aidex vault lock`, an emergency stop that locks the vault) ends the
 * unlocked session while screens still show what they decrypted, and a screen that is not polling
 * (the user is away, or its session stream is up) would keep that on screen until its next read.
 * Each lock bumps the counter, and `useRpc` re-reads every query when it changes, so a read refused
 * as `vault_locked` drops its data and the pages' own lock handling clears what they kept beside it.
 * A reconnect resumes after the last event or watermark it processed, so a lock while the stream was
 * down still counts; history that expired meanwhile counts as one.
 */
export function VaultLockProvider({ children }: { children: ReactNode }) {
  const { connectionKey, daemonRunning } = useAidexStatus();
  const [locks, setLocks] = useState(0);

  useEffect(() => {
    if (!daemonRunning) return;
    let cancelled = false;
    let cursor: number | undefined;
    let failures = 0;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let release: (() => Promise<void>) | undefined;

    const locked = () => {
      if (!cancelled) setLocks((count) => count + 1);
    };
    const reconnect = () => {
      if (cancelled || retry) return;
      release = undefined;
      retry = setTimeout(() => {
        retry = undefined;
        void connect();
      }, reconnectDelay(failures));
      failures += 1;
    };
    const connect = async () => {
      let ended = false;
      let openedAt = 0;
      try {
        if (cursor === undefined) cursor = (await call("events.latest")).latest_cursor;
        if (cancelled) return;
        const subscription = await subscribe(
          "events.subscribe",
          { after: cursor, filter: VAULT_LOCK_FILTER },
          {
            onData: (data, method) => {
              if (cancelled || cursor === undefined) return;
              const update = followVaultLocks(method, data, cursor);
              cursor = update.cursor;
              if (update.locked) locked();
            },
            onEnd: () => {
              ended = true;
              if (openedAt && Date.now() - openedAt >= STABLE_STREAM_MS) failures = 0;
              reconnect();
            },
          },
        );
        if (cancelled) {
          await subscription.unsubscribe();
          return;
        }
        if (ended) return;
        openedAt = Date.now();
        release = subscription.unsubscribe;
      } catch (caught) {
        if (cancelled) return;
        if (cursorUnusable(toAidexError(caught).data)) {
          // The resume point is gone: start again from the latest cursor, counting a possible lock.
          cursor = undefined;
          locked();
        }
        reconnect();
      }
    };

    void connect();
    return () => {
      cancelled = true;
      clearTimeout(retry);
      void release?.().catch(() => undefined);
    };
  }, [daemonRunning, connectionKey]);

  return <VaultLockContext.Provider value={locks}>{children}</VaultLockContext.Provider>;
}
