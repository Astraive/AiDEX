import { useEffect, useRef, useState } from "react";
import { call, subscribe } from "@aidex/client/rpc";
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import { toAidexError } from "@aidex/client/errors";
import { useAidexStatus } from "./useAidexStatus";
import { sessionEvent } from "./sessionEvents";

/** Session snapshots remain authoritative; stream cursors recover missed durable updates. */
export function useSessionUpdates(
  sessionId: string | undefined,
  refresh: () => void,
  onEvent?: (event: EventEnvelope) => void,
) {
  const { connectionKey, daemonRunning } = useAidexStatus();
  const refreshRef = useRef(refresh);
  const eventRef = useRef(onEvent);
  refreshRef.current = refresh;
  eventRef.current = onEvent;
  const [state, setState] = useState({
    active: false,
    reconnecting: false,
    error: undefined as string | undefined,
  });
  useEffect(() => {
    if (!sessionId || !daemonRunning) return;
    let cancelled = false;
    let cursor: number | undefined;
    let failures = 0;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;
    let release: (() => Promise<void>) | undefined;
    const changed = () => {
      if (!refreshTimer)
        refreshTimer = setTimeout(() => {
          refreshTimer = undefined;
          if (!cancelled) refreshRef.current();
        }, 150);
    };
    const schedule = () => {
      if (cancelled || retry) return;
      setState((previous) => ({
        ...previous,
        active: false,
        reconnecting: true,
      }));
      retry = setTimeout(
        () => {
          retry = undefined;
          void connect();
        },
        Math.min(30000, 1000 * 2 ** Math.min(failures++, 5)),
      );
    };
    const connect = async () => {
      let ended = false;
      try {
        if (cursor === undefined)
          cursor = (await call("events.latest")).latest_cursor;
        if (cancelled) return;
        const subscription = await subscribe(
          "events.subscribe",
          { after: cursor, filter: { session_id: sessionId } },
          {
            onData: (data, method) => {
              if (cancelled) return;
              if (method === "events.error") {
                const reason = (data as { reason?: string })?.reason;
                if (reason === "cursor_expired") {
                  cursor = undefined;
                  changed();
                }
                setState({
                  active: false,
                  reconnecting: true,
                  error:
                    "Live event history changed. Reloading the saved session and reconnecting.",
                });
                return;
              }
              const update = sessionEvent(data, method, sessionId, cursor);
              if (!update) return;
              cursor = update.cursor;
              // Ephemeral deltas share the latest durable cursor. Never discard them by cursor equality.
              eventRef.current?.(update.envelope);
              changed();
            },
            onEnd: () => {
              ended = true;
              schedule();
            },
          },
        );
        if (cancelled) {
          await subscription.unsubscribe();
          return;
        }
        release = subscription.unsubscribe;
        if (!ended) {
          failures = 0;
          setState({ active: true, reconnecting: false, error: undefined });
          changed();
        }
      } catch (caught) {
        if (cancelled) return;
        const error = toAidexError(caught);
        const reason = (error.data as { reason?: string } | undefined)?.reason;
        if (reason === "cursor_expired" || reason === "cursor_ahead")
          cursor = undefined;
        setState({ active: false, reconnecting: true, error: error.message });
        schedule();
      }
    };
    setState({ active: false, reconnecting: false, error: undefined });
    void connect();
    return () => {
      cancelled = true;
      clearTimeout(retry);
      clearTimeout(refreshTimer);
      void release?.().catch(() => undefined);
    };
  }, [sessionId, daemonRunning, connectionKey]);
  return sessionId && daemonRunning
    ? state
    : { active: false, reconnecting: false, error: undefined };
}
