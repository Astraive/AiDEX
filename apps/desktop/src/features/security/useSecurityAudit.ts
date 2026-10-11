import { useEffect, useState } from "react";
import { call, subscribe } from "@aidex/client/rpc";
import type { EventEnvelope, LatestCursor } from "@aidex/client/dto/daemon";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { useBridgeQuery } from "../../hooks/useBridgeQuery";
import { auditKinds, auditMetadata, type AuditMetadata } from "./auditMetadata";

export function useSecurityAudit() {
  const { daemonRunning, connectionKey } = useAidexStatus();
  const [bounds, setBounds] = useState<LatestCursor>();
  const [after, setAfter] = useState<number>();
  const [history, setHistory] = useState<number[]>([]);
  const [liveRows, setLiveRows] = useState<AuditMetadata[]>([]);
  const [streamState, setStreamState] = useState("Connecting audit stream…");
  const start =
    after ??
    Math.max(
      0,
      (bounds?.latest_cursor ?? 0) - 200,
      (bounds?.oldest_cursor ?? 1) - 1,
    );
  const page = useBridgeQuery(
    async () => {
      const result = await call("events.replay", {
        after: start,
        limit: 50,
        filter: { kinds: auditKinds },
      });
      return {
        rows: result.events.flatMap((event) => {
          const row = auditMetadata(event);
          return row ? [row] : [];
        }),
        nextCursor: result.next_cursor,
        hasMore: result.has_more,
      };
    },
    `${connectionKey}|security-audit|${start}`,
    daemonRunning && !!bounds,
  );
  useEffect(() => {
    if (!daemonRunning) return;
    let closed = false,
      cursor: number | undefined,
      failures = 0;
    let retry: ReturnType<typeof setTimeout>;
    let release: (() => Promise<void>) | undefined;
    setBounds(undefined);
    setAfter(undefined);
    setHistory([]);
    setLiveRows([]);
    const reconnect = () => {
      if (!closed) {
        setStreamState(
          "Audit stream disconnected; reconnecting from the last durable cursor…",
        );
        retry = setTimeout(
          () => void connect(),
          Math.min(30000, 1000 * 2 ** Math.min(failures++, 5)),
        );
      }
    };
    const connect = async () => {
      let ended = false;
      try {
        const latest = await call("events.latest");
        if (closed) return;
        setBounds(latest);
        if (cursor === undefined) cursor = latest.latest_cursor;
        if (latest.oldest_cursor != null && cursor < latest.oldest_cursor - 1) {
          cursor = latest.latest_cursor;
          setStreamState(
            "Older audit events expired; reloading retained metadata.",
          );
        }
        const stream = await subscribe(
          "events.subscribe",
          { after: cursor, filter: { kinds: auditKinds } },
          {
            onData: (data, method) => {
              if (closed) return;
              if (method === "events.error") {
                cursor = undefined;
                setStreamState(
                  "Audit replay requires a fresh retained snapshot.",
                );
                return;
              }
              if (method !== "events.event") return;
              const row = auditMetadata(data as EventEnvelope);
              if (!row || row.cursor <= (cursor ?? -1)) return;
              cursor = row.cursor;
              setLiveRows((previous) =>
                [
                  ...previous.filter((event) => event.cursor !== row.cursor),
                  row,
                ].slice(-50),
              );
            },
            onEnd: () => {
              ended = true;
              reconnect();
            },
          },
        );
        if (closed) {
          await stream.unsubscribe();
          return;
        }
        release = stream.unsubscribe;
        if (!ended) {
          failures = 0;
          setStreamState("Live audit metadata connected");
        }
      } catch {
        if (!closed) reconnect();
      }
    };
    void connect();
    return () => {
      closed = true;
      clearTimeout(retry);
      void release?.().catch(() => undefined);
    };
  }, [daemonRunning, connectionKey]);
  const rows = [
    ...new Map(
      [
        ...(page.data?.rows ?? []),
        ...(after === undefined ? liveRows : []),
      ].map((row) => [row.cursor, row]),
    ).values(),
  ]
    .sort((a, b) => b.cursor - a.cursor)
    .slice(0, 50);
  return {
    rows,
    page,
    start,
    streamState: daemonRunning ? streamState : "Core is unavailable",
    tail: after === undefined,
    previous: () => {
      const prior = history.at(-1);
      if (prior !== undefined) {
        setHistory(history.slice(0, -1));
        setAfter(prior);
      }
    },
    canPrevious: history.length > 0,
    next: () => {
      if (page.data?.hasMore) {
        setHistory([...history, start]);
        setAfter(page.data.nextCursor);
      }
    },
    beginning: () => {
      setHistory([]);
      setAfter(Math.max(0, (bounds?.oldest_cursor ?? 1) - 1));
    },
    recent: () => {
      setHistory([]);
      setAfter(undefined);
      page.refetch();
    },
  };
}
