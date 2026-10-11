import type { EventEnvelope } from "@aidex/client/dto/daemon";
const ephemeral = new Set(["agent.delta", "agent.run_delta", "process.output"]);
/** Only durable events advance replay. Equal-cursor live deltas remain distinct updates. */
export function sessionEvent(
  data: unknown,
  method: string,
  sessionId: string,
  cursor: number | undefined,
): { envelope: EventEnvelope; cursor: number | undefined } | undefined {
  if (method !== "events.event" || !data || typeof data !== "object") return;
  const envelope = data as EventEnvelope;
  if (
    !Number.isSafeInteger(envelope.cursor) ||
    envelope.cursor < 0 ||
    !envelope.event ||
    typeof envelope.event.type !== "string" ||
    envelope.scope?.session_id !== sessionId
  )
    return;
  if (ephemeral.has(envelope.event.type)) return { envelope, cursor };
  if (cursor !== undefined && envelope.cursor <= cursor) return;
  return { envelope, cursor: envelope.cursor };
}
