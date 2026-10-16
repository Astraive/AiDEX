// Reads the security audit rows: "recent" follows a filtered `events.subscribe` stream that replays a
// recent window of the log and then stays live; "browse" pages `events.replay` from the oldest event
// the drive still keeps. Rows are projected by `auditMetadata` before they reach state.
import { useEffect, useState } from "react";
import { call, subscribe } from "@aidex/client/rpc";
import { toAidexError, type AidexError } from "@aidex/client/errors";
import type { EventEnvelope, StreamWatermark } from "@aidex/client/dto/daemon";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { useBridgeQuery } from "../../hooks/useBridgeQuery";
import { auditKinds, auditMetadata, mergeAuditRow, type AuditMetadata } from "./auditMetadata";

/** Rows shown at once, in either mode. */
export const AUDIT_ROWS = 50;
/** How far back (in log cursors) the recent view starts reading. */
const RECENT_WINDOW = 5000;
const MAX_BACKOFF_MS = 30_000;

const filter = { kinds: [...auditKinds] };

export type AuditStream = "connecting" | "live" | "reconnecting" | "offline";

export type SecurityAudit = {
  mode: "recent" | "browse";
  /** Newest first. */
  rows: AuditMetadata[];
  stream: AuditStream;
  /** Set when part of the requested range expired before it could be read. */
  gap: boolean;
  loading: boolean;
  error: AidexError | null;
  canOlder: boolean;
  canNewer: boolean;
  older: () => void;
  newer: () => void;
  /** Starts browsing from the oldest event the drive keeps. */
  browse: () => void;
  /** Returns to the recent, live view. */
  recent: () => void;
};

const reasonOf = (error: AidexError): string | undefined => (error.data as { reason?: string } | undefined)?.reason;

/** Where reading may start: just before the oldest kept event, and no further back than `window`. */
async function startCursor(window: number): Promise<number> {
  const latest = await call("events.latest");
  const oldest = latest.oldest_cursor ?? latest.latest_cursor + 1;
  return Math.max(0, oldest - 1, latest.latest_cursor - window);
}

export function useSecurityAudit(): SecurityAudit {
  const { daemonRunning, connectionKey } = useAidexStatus();
  const [mode, setMode] = useState<"recent" | "browse">("recent");
  const [liveRows, setLiveRows] = useState<AuditMetadata[]>([]);
  const [stream, setStream] = useState<AuditStream>("connecting");
  const [gap, setGap] = useState(false);
  // Browse position: the cursor each viewed page started after, oldest page first.
  const [pages, setPages] = useState<number[]>([]);
  const [browseError, setBrowseError] = useState<AidexError | null>(null);
  const after = pages.at(-1);

  useEffect(() => {
    setMode("recent");
    setPages([]);
    setGap(false);
  }, [connectionKey]);

  // Recent and live: one filtered stream, resumed after the last event or watermark it delivered.
  useEffect(() => {
    if (!daemonRunning || mode !== "recent") {
      setStream("offline");
      return;
    }
    let closed = false;
    let cursor: number | undefined;
    let failures = 0;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let release: (() => Promise<void>) | undefined;
    setLiveRows([]);
    setStream("connecting");

    const reconnect = () => {
      if (closed) return;
      setStream("reconnecting");
      retry = setTimeout(() => void connect(), Math.min(MAX_BACKOFF_MS, 1000 * 2 ** Math.min(failures++, 5)));
    };

    const connect = async () => {
      let ended = false;
      try {
        if (cursor === undefined) cursor = await startCursor(RECENT_WINDOW);
        if (closed) return;
        const subscription = await subscribe(
          "events.subscribe",
          { after: cursor, filter },
          {
            onData: (data, method) => {
              if (closed) return;
              if (method === "events.watermark") {
                const mark = (data as StreamWatermark).cursor;
                if (Number.isSafeInteger(mark)) cursor = Math.max(cursor ?? 0, mark);
                return;
              }
              if (method === "events.error") {
                // The stream ends after this; start again from what the drive still keeps.
                cursor = undefined;
                if ((data as { reason?: string } | null)?.reason === "cursor_expired") setGap(true);
                return;
              }
              if (method !== "events.event") return;
              const envelope = data as EventEnvelope;
              if (!Number.isSafeInteger(envelope?.cursor) || (cursor !== undefined && envelope.cursor <= cursor)) return;
              cursor = envelope.cursor;
              const row = auditMetadata(envelope);
              if (row) setLiveRows((rows) => mergeAuditRow(rows, row, AUDIT_ROWS));
            },
            onEnd: () => {
              ended = true;
              reconnect();
            },
          },
        );
        if (closed) {
          void subscription.unsubscribe().catch(() => undefined);
          return;
        }
        release = subscription.unsubscribe;
        if (!ended) {
          failures = 0;
          setStream("live");
        }
      } catch (caught) {
        const reason = reasonOf(toAidexError(caught));
        if (reason === "cursor_expired" || reason === "cursor_ahead") {
          cursor = undefined;
          setGap(reason === "cursor_expired");
        }
        reconnect();
      }
    };

    void connect();
    return () => {
      closed = true;
      if (retry) clearTimeout(retry);
      void release?.().catch(() => undefined);
    };
  }, [daemonRunning, connectionKey, mode]);

  const page = useBridgeQuery(
    async () => {
      const result = await call("events.replay", { after, filter, limit: AUDIT_ROWS });
      return {
        rows: result.events.flatMap((event) => auditMetadata(event) ?? []).reverse(),
        next: result.next_cursor,
        hasMore: result.has_more,
      };
    },
    `${connectionKey}|security-audit|${after ?? ""}`,
    daemonRunning && mode === "browse" && after !== undefined,
  );

  useEffect(() => {
    if (page.error && reasonOf(page.error) === "cursor_expired") setGap(true);
  }, [page.error]);

  const browse = () => {
    setBrowseError(null);
    setGap(false);
    call("events.latest").then(
      (latest) => {
        setPages([Math.max(0, (latest.oldest_cursor ?? latest.latest_cursor + 1) - 1)]);
        setMode("browse");
      },
      (caught: unknown) => setBrowseError(toAidexError(caught)),
    );
  };

  const browsing = mode === "browse";
  return {
    mode,
    rows: browsing ? (page.data?.rows ?? []) : liveRows,
    stream: daemonRunning ? stream : "offline",
    gap,
    loading: browsing ? page.loading : stream === "connecting",
    error: browseError ?? (browsing ? page.error : null),
    canOlder: browsing && pages.length > 1,
    canNewer: browsing && !!page.data?.hasMore,
    older: () => setPages((current) => (current.length > 1 ? current.slice(0, -1) : current)),
    newer: () => {
      const next = page.data?.next;
      if (page.data?.hasMore && next !== undefined) setPages((current) => [...current, next]);
    },
    browse,
    recent: () => {
      setBrowseError(null);
      setGap(false);
      setPages([]);
      setMode("recent");
    },
  };
}
