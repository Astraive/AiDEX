import { useEffect, useRef, useState } from "react";
import type { EventEnvelope, SubscribeParams } from "@aidex/client/dto/daemon";
import { toAidexError } from "@aidex/client/errors";
import { call, subscribe } from "@aidex/client/rpc";
import { useAidexStatus } from "./useAidexStatus";
import { cursorUnusable, reconnectDelay, sessionStreamStep } from "./sessionEvents";

export type SessionUpdates = {
  /** The stream is subscribed and has not ended. */
  active: boolean;
  /** The stream dropped and a reconnect is scheduled; live-only output sent meanwhile is lost. */
  reconnecting: boolean;
  /** Why the stream dropped, when the daemon or bridge said. */
  error: string | undefined;
};

const IDLE: SessionUpdates = { active: false, reconnecting: false, error: undefined };
/** Durable events arrive in bursts (a tool call is several); one snapshot read covers a burst. */
const REFRESH_DELAY_MS = 150;
/** A stream that stayed up this long resets the reconnect backoff. */
const STABLE_STREAM_MS = 10_000;

/**
 * Follow one session's events (`events.subscribe` filtered to `sessionId`) while the daemon answers.
 * The saved session stays authoritative: `refresh` re-reads it after (re)connecting and after
 * durable events, and `onEvent` receives every event of the session in order, ephemeral output
 * (`agent.delta`, `agent.run_delta`, `process.output`) included.
 *
 * The stream starts at `events.latest` and resumes after the last durable event or watermark it
 * processed, so a reconnect replays missed durable events without duplicates. When that history
 * expired it re-reads the snapshot and starts again from the latest cursor. Reconnects back off
 * from 1 s to 30 s; a new drive or daemon starts over.
 */
export function useSessionUpdates(
  sessionId: string | undefined,
  refresh: () => void,
  onEvent?: (envelope: EventEnvelope) => void,
): SessionUpdates {
  const { connectionKey, daemonRunning } = useAidexStatus();
  const refreshRef = useRef(refresh);
  const onEventRef = useRef(onEvent);
  refreshRef.current = refresh;
  onEventRef.current = onEvent;
  const [state, setState] = useState<SessionUpdates>(IDLE);

  useEffect(() => {
    if (!sessionId || !daemonRunning) return;
    let cancelled = false;
    let cursor: number | undefined;
    let failures = 0;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;
    let release: (() => Promise<void>) | undefined;

    const changed = () => {
      if (cancelled || refreshTimer) return;
      refreshTimer = setTimeout(() => {
        refreshTimer = undefined;
        if (!cancelled) refreshRef.current();
      }, REFRESH_DELAY_MS);
    };
    const reconnect = (error?: string) => {
      if (cancelled) return;
      release = undefined;
      setState((previous) => ({ active: false, reconnecting: true, error: error ?? previous.error }));
      if (retry) return;
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
        if (cursor === undefined) {
          cursor = (await call("events.latest")).latest_cursor;
          // Anything before the new starting point is only in the snapshot.
          changed();
        }
        if (cancelled) return;
        const params: SubscribeParams = { after: cursor, filter: { session_id: sessionId } };
        const subscription = await subscribe("events.subscribe", params, {
          onData: (data, method) => {
            if (cancelled || cursor === undefined) return;
            const step = sessionStreamStep(method, data, sessionId, cursor);
            switch (step.kind) {
              case "event":
                cursor = step.cursor;
                onEventRef.current?.(step.envelope);
                if (step.durable) changed();
                break;
              case "watermark":
                cursor = step.cursor;
                break;
              case "error":
                if (step.expired) {
                  cursor = undefined;
                  changed();
                }
                setState({
                  active: false,
                  reconnecting: true,
                  error: step.expired
                    ? "Missed session events are no longer kept. Reloading the saved session."
                    : step.message,
                });
                break;
            }
          },
          onEnd: () => {
            ended = true;
            if (openedAt && Date.now() - openedAt >= STABLE_STREAM_MS) failures = 0;
            reconnect();
          },
        });
        if (cancelled) {
          await subscription.unsubscribe();
          return;
        }
        if (ended) return;
        openedAt = Date.now();
        release = subscription.unsubscribe;
        setState({ active: true, reconnecting: false, error: undefined });
        // Live-only output sent while disconnected is gone; the snapshot has what was saved.
        changed();
      } catch (caught) {
        if (cancelled) return;
        const error = toAidexError(caught);
        if (cursorUnusable(error.data)) cursor = undefined;
        reconnect(error.message);
      }
    };

    setState(IDLE);
    void connect();
    return () => {
      cancelled = true;
      clearTimeout(retry);
      clearTimeout(refreshTimer);
      void release?.().catch(() => undefined);
    };
  }, [sessionId, daemonRunning, connectionKey]);

  return sessionId && daemonRunning ? state : IDLE;
}
