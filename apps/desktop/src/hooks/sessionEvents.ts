// The resume-cursor rules of an `events.subscribe` stream (protocol/events/README.md), as pure
// functions so they can be tested without a daemon. `useSessionUpdates` applies them to one
// session's stream, and `VaultLockProvider` (`../services`) to the drive-wide `vault.locked` events.
import type { EventEnvelope, EventStreamError, StreamWatermark } from "@aidex/client/dto/daemon";

/** Live-only types: never stored or replayed, and their cursor is only the latest durable one. */
export const EPHEMERAL_EVENTS: ReadonlySet<string> = new Set(["agent.delta", "agent.run_delta", "process.output"]);

/**
 * What one notification means to a follower of one session that has processed every durable event
 * through `cursor`. `cursor` in a step is where to resume after handling it.
 */
export type SessionStreamStep =
  /** Deliver `envelope`. Only a durable event moves the cursor; ephemeral ones never do. */
  | { kind: "event"; envelope: EventEnvelope; durable: boolean; cursor: number }
  /** Every durable event through the watermark was delivered or filtered out; nothing to deliver. */
  | { kind: "watermark"; cursor: number }
  /** The daemon ended the stream. `expired`: retention removed history this follower never saw. */
  | { kind: "error"; reason: string; message: string; expired: boolean }
  /** A duplicate, another session's event, a malformed notification, or an unknown method. */
  | { kind: "ignored" };

const IGNORED: SessionStreamStep = { kind: "ignored" };

const isCursor = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;

function isEnvelope(data: unknown): data is EventEnvelope {
  if (!data || typeof data !== "object") return false;
  const envelope = data as Partial<EventEnvelope>;
  return (
    isCursor(envelope.cursor) &&
    !!envelope.scope &&
    typeof envelope.scope === "object" &&
    !!envelope.event &&
    typeof envelope.event.type === "string"
  );
}

/** Classify one `events.subscribe` notification for the follower of `sessionId` at `cursor`. */
export function sessionStreamStep(method: string, data: unknown, sessionId: string, cursor: number): SessionStreamStep {
  return streamStep(method, data, cursor, (envelope) => envelope.scope.session_id === sessionId);
}

/**
 * Classify one `events.subscribe` notification for a follower, at `cursor`, of the events `follows`
 * accepts (its subscription filter's events; anything else is ignored, never delivered).
 */
export function streamStep(
  method: string,
  data: unknown,
  cursor: number,
  follows: (envelope: EventEnvelope) => boolean,
): SessionStreamStep {
  switch (method) {
    case "events.event": {
      if (!isEnvelope(data) || !follows(data)) return IGNORED;
      // Ephemeral events share the latest durable cursor: never deduplicate or acknowledge by it.
      if (EPHEMERAL_EVENTS.has(data.event.type)) return { kind: "event", envelope: data, durable: false, cursor };
      if (data.cursor <= cursor) return IGNORED;
      return { kind: "event", envelope: data, durable: true, cursor: data.cursor };
    }
    case "events.watermark": {
      const watermark = (data as Partial<StreamWatermark> | null)?.cursor;
      if (!isCursor(watermark)) return IGNORED;
      return { kind: "watermark", cursor: Math.max(cursor, watermark) };
    }
    case "events.error": {
      const error = (data ?? {}) as Partial<EventStreamError>;
      const reason = typeof error.reason === "string" ? error.reason : "internal";
      return {
        kind: "error",
        reason,
        message: typeof error.message === "string" ? error.message : "The event stream stopped.",
        expired: reason === "cursor_expired",
      };
    }
    default:
      return IGNORED;
  }
}

/** The run an `agent.run_delta` belongs to (root or child) and its text; undefined for anything else. */
export function runDelta(envelope: EventEnvelope): { runId: string; text: string } | undefined {
  if (envelope.event.type !== "agent.run_delta") return undefined;
  const { agent_id: runId, text } = envelope.event.data ?? {};
  if (typeof runId !== "string" || !runId || typeof text !== "string") return undefined;
  return { runId, text };
}

/** Subscribe errors after which the cursor cannot be resumed: start again from `events.latest`. */
export function cursorUnusable(errorData: unknown): boolean {
  const reason = (errorData as { reason?: unknown } | null | undefined)?.reason;
  return reason === "cursor_expired" || reason === "cursor_ahead";
}

/** Delay before reconnect attempt `failures` (0-based): 1, 2, 4, 8 and 16 s, then 30 s. */
export function reconnectDelay(failures: number): number {
  return Math.min(30_000, 1000 * 2 ** Math.min(Math.max(failures, 0), 5));
}
