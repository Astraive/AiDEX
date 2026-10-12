// Turns a session's messages into transcript items: chat messages, harness tool calls paired with
// their results (rendered as work-log rows and cards), and runs of web searches grouped together.
// Pure functions, so they run during prerendering.
import { MCP_TOOL_PREFIX, NATIVE_TOOL_NAMES, type Message, type ToolCall } from "@aidex/client/dto/sessions";

export type ToolKind =
  | "web_search"
  | "web_fetch"
  | "skill_load"
  | "artifact_create"
  | "artifact_update"
  | "plan_propose"
  | "mcp";

const nativeKinds: Record<string, ToolKind> = {
  [NATIVE_TOOL_NAMES.webSearch]: "web_search",
  [NATIVE_TOOL_NAMES.webFetch]: "web_fetch",
  [NATIVE_TOOL_NAMES.skillLoad]: "skill_load",
  [NATIVE_TOOL_NAMES.artifactCreate]: "artifact_create",
  [NATIVE_TOOL_NAMES.artifactUpdate]: "artifact_update",
  [NATIVE_TOOL_NAMES.planPropose]: "plan_propose",
};

/** The harness tool a model-facing tool name refers to; null for other tools (shown as before). */
export function toolKindOf(name: string): ToolKind | null {
  if (nativeKinds[name]) return nativeKinds[name];
  return parseMcpToolName(name) ? "mcp" : null;
}

/** `mcp__<server>__<tool>` split into its server and tool parts. */
export function parseMcpToolName(name: string): { server: string; tool: string } | null {
  if (!name.startsWith(MCP_TOOL_PREFIX)) return null;
  const rest = name.slice(MCP_TOOL_PREFIX.length);
  const split = rest.indexOf("__");
  if (split <= 0 || split + 2 >= rest.length) return null;
  return { server: rest.slice(0, split), tool: rest.slice(split + 2) };
}

/** What a tool call without a result means here. */
export type PendingState =
  /** The run is still working on it. */
  | "running"
  /** The run ended without a result (interrupted or cancelled). */
  | "missing"
  /** The result may be on another page of history. */
  | "unknown";

export type ToolEntry = {
  key: string;
  kind: ToolKind;
  call: ToolCall;
  /** Position of the call in its message's `tool_calls`. */
  callIndex: number;
  /** The assistant message that made the call. */
  owner: Message;
  result?: Message;
  /** Meaningful only without a result. */
  pending: PendingState;
};

export type TranscriptItem =
  | {
      type: "message";
      key: string;
      message: Message;
      /** Indexes of calls in `message.tool_calls` rendered as their own items. */
      hiddenCalls: ReadonlySet<number>;
    }
  | { type: "tool"; key: string; entry: ToolEntry }
  | { type: "web-searches"; key: string; entries: ToolEntry[] };

export type TranscriptOptions = {
  /** A run is active: calls after the last user message without a result are still running. */
  running: boolean;
  /** Every result of a listed call is listed too (a live conversation tail, not a history page). */
  complete: boolean;
};

/** Messages in display order as transcript items; system messages are left out. */
export function buildTranscript(messages: readonly Message[], options: TranscriptOptions): TranscriptItem[] {
  const results = new Map<string, Message>();
  for (const message of messages)
    if (message.role === "tool" && message.tool_call_id && !results.has(message.tool_call_id))
      results.set(message.tool_call_id, message);
  let lastUser = -1;
  messages.forEach((message, index) => {
    if (message.role === "user") lastUser = index;
  });

  const consumed = new Set<Message>();
  const items: TranscriptItem[] = [];
  messages.forEach((message, index) => {
    if (message.role === "system" || consumed.has(message)) return;
    if (message.role !== "assistant") {
      items.push({ type: "message", key: `m${message.original_index}`, message, hiddenCalls: new Set() });
      return;
    }
    const hiddenCalls = new Set<number>();
    const entries: ToolEntry[] = [];
    message.tool_calls.forEach((call, callIndex) => {
      const kind = toolKindOf(call.name);
      if (!kind) return;
      hiddenCalls.add(callIndex);
      const result = results.get(call.id);
      if (result) consumed.add(result);
      entries.push({
        key: `t${message.original_index}:${callIndex}`,
        kind,
        call,
        callIndex,
        owner: message,
        result,
        pending: !options.complete ? "unknown" : options.running && index > lastUser ? "running" : "missing",
      });
    });
    const shown =
      !!message.content || message.tool_calls_truncated || hiddenCalls.size < message.tool_calls.length;
    if (shown) items.push({ type: "message", key: `m${message.original_index}`, message, hiddenCalls });
    for (const entry of entries) items.push({ type: "tool", key: entry.key, entry });
  });
  return groupWebSearches(items);
}

/** Consecutive web searches become one group ("Searched the web N times"). */
function groupWebSearches(items: TranscriptItem[]): TranscriptItem[] {
  const grouped: TranscriptItem[] = [];
  let run: ToolEntry[] = [];
  const flush = () => {
    if (run.length === 1) grouped.push({ type: "tool", key: run[0].key, entry: run[0] });
    else if (run.length > 1) grouped.push({ type: "web-searches", key: `g${run[0].key}`, entries: run });
    run = [];
  };
  for (const item of items) {
    if (item.type === "tool" && item.entry.kind === "web_search") run.push(item.entry);
    else {
      flush();
      grouped.push(item);
    }
  }
  flush();
  return grouped;
}

export type ToolOutcome = { status: "running" | "completed" | "failed"; statusText?: string };

const failurePrefixes: readonly [string, string][] = [
  ["Tool execution denied by user.", "Denied"],
  ["Tool refused:", "Refused"],
  ["Tool failed:", "Failed"],
  ["The MCP tool reported an error:", "Error"],
];

/** Status of a call from its result text (the harness's failure prefixes) or its pending state. */
export function toolOutcome(entry: ToolEntry): ToolOutcome {
  if (!entry.result) {
    if (entry.pending === "running") return { status: "running" };
    if (entry.pending === "missing") return { status: "failed", statusText: "No result" };
    return { status: "completed" };
  }
  const text = entry.result.content.trimStart();
  for (const [prefix, statusText] of failurePrefixes)
    if (text.startsWith(prefix)) return { status: "failed", statusText };
  return { status: "completed" };
}

/** Aggregate status of a group: running while any member runs, failed when one failed. */
export function groupOutcome(entries: readonly ToolEntry[]): ToolOutcome {
  const outcomes = entries.map(toolOutcome);
  if (outcomes.some((outcome) => outcome.status === "running")) return { status: "running" };
  if (outcomes.some((outcome) => outcome.status === "failed"))
    return { status: "failed", statusText: "Some failed" };
  return { status: "completed" };
}

/** Parses JSON, or null when it is not valid (for example a cut-off excerpt). */
export function parseJson<T>(text: string | undefined): T | null {
  if (!text) return null;
  try {
    return JSON.parse(text) as T;
  } catch {
    return null;
  }
}

/**
 * A top-level string field from JSON text that may be cut off (tool arguments and results arrive as
 * bounded excerpts). The full object is preferred when it parses.
 */
export function jsonStringField(text: string | undefined, field: string): string | null {
  if (!text) return null;
  const whole = parseJson<Record<string, unknown>>(text);
  if (whole && typeof whole === "object") {
    const value = whole[field];
    return typeof value === "string" ? value : null;
  }
  const match = new RegExp(`"${field}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)"`).exec(text);
  if (!match) return null;
  try {
    return JSON.parse(`"${match[1]}"`) as string;
  } catch {
    return null;
  }
}
