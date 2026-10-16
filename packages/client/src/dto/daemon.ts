// Mirrors the `daemon.*` and `events.*` method DTOs in crates/aidexd/src/methods/{daemon,events}.rs and
// `aidex_events::{EventEnvelope, EventFilter, ReplayPage}` (crates/events/src/). Hand-written until
// schema generation exists.

/** Result of `daemon.status`. */
export type DaemonStatus = {
  state: "ready" | "stopping";
  pid: number;
  contract_version: number;
  version: string;
  /** As the daemon resolved it (on Windows with a `\\?\` prefix). */
  drive_root: string;
  uptime_ms: number;
};

/** Which events a replay or subscription returns; omitted fields match everything. */
export type EventFilter = {
  workspace_id?: string;
  session_id?: string;
  /** Event types such as `session.started`; empty matches all. */
  kinds?: string[];
};

/** Params of `events.replay`. */
export type ReplayParams = {
  /** Last acknowledged global cursor; `0` (default) replays from the start. */
  after?: number;
  filter?: EventFilter;
  /** Page size (default and maximum set by the daemon). */
  limit?: number;
};

/** Params of `events.subscribe`. */
export type SubscribeParams = { after?: number; filter?: EventFilter };

/** Initial `data` of an `events.subscribe` response. */
export type EventsSubscribed = { after: number; latest_cursor: number };

/** Payload of an `events.error` notification (the stream ends after it). */
export type EventStreamError = { reason: "cursor_expired" | "internal" | string; message: string; oldest_available?: number };

/**
 * Payload of an `events.watermark` notification (filtered subscriptions): every durable event
 * through `cursor` was sent in an earlier `events.event` notification or excluded by the filter.
 * After processing the preceding notifications, resume after `cursor`. Watermarks are not events
 * and arrive only on durable progress, never as heartbeats.
 */
export type StreamWatermark = { cursor: number };

/** The notifications of an `events.subscribe` stream (`onData`'s `data` and `method`). */
export type EventStreamNotification =
  | { method: "events.event"; data: EventEnvelope }
  | { method: "events.watermark"; data: StreamWatermark }
  | { method: "events.error"; data: EventStreamError };

export type EventScope = { workspace_id?: string; session_id?: string };

/**
 * One event: adjacently tagged (`type` + `data`). The catalog (crates/events/src/catalog) uses dotted
 * types such as `session.started` and keeps growing, so screens must tolerate types they do not know.
 */
export type EventKind = { type: string; data?: Record<string, unknown> };

/** `aidex_events::EventEnvelope` (envelope version 2). */
export type EventEnvelope = {
  version: number;
  /** Global, monotonic cursor: the replay and resume position. */
  cursor: number;
  scope: EventScope;
  occurred_at: string;
  event: EventKind;
};

/**
 * `data` of the ephemeral `agent.delta` event: live output text of the event conversation's root
 * runs only (a child run's output arrives only as `agent.run_delta`, which carries every run).
 * Ephemeral events are never stored or replayed and carry the latest durable cursor, so never
 * acknowledge or deduplicate by their cursor; the saved transcript is authoritative.
 */
export type AgentDelta = { text: string };

/** `data` of the ephemeral `agent.run_delta` event: live output of run `agent_id`, for every run, root or child. */
export type AgentRunDelta = { agent_id: string; text: string };

/** Result of `events.replay`, ascending by cursor. */
export type ReplayPage = { events: EventEnvelope[]; next_cursor: number; has_more: boolean };

/** Result of `events.latest`. */
export type LatestCursor = { latest_cursor: number; oldest_cursor: number | null };
