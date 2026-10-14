// Bounded live excerpts for the execution page. `process.output` and `agent.run_delta` are
// ephemeral (never stored or replayed), so these only cover what arrived while the page was
// connected; final tool output and run transcripts stay in the encrypted session history.
import type { EventEnvelope } from "@aidex/client/dto/daemon";
import { ErrorKind } from "@aidex/client/errors";

/** Characters of each process stream kept (the latest 64 KiB of ASCII output). */
export const STREAM_LIMIT = 64 * 1024;
/** Processes kept; ended ones are dropped first, then the least recently active. */
export const PROCESS_LIMIT = 8;
/** Characters of live text kept per run. */
export const RUN_TEXT_LIMIT = 16 * 1024;
/** Runs whose live text is kept, the most recently active first. */
export const RUN_LIMIT = 16;

export type ProcessTrace = {
  id: string;
  /** The command summary from `process.started`, or the process id until it arrives. */
  label: string;
  stdout: string;
  stderr: string;
  ended: boolean;
  exitCode: number | null;
  signal: number | null;
  /** Disconnected while running: whether it is still running is unknown until its next event. */
  uncertain: boolean;
  /** Output sent while the page was disconnected is missing from the excerpts. */
  gap: boolean;
};

/** Traces in order of last activity, oldest first. */
export type ProcessTraces = readonly ProcessTrace[];

/** Keep the last `limit` characters without starting inside a surrogate pair. */
export function tail(text: string, limit: number): string {
  if (text.length <= limit) return text;
  const cut = text.slice(text.length - limit);
  const first = cut.charCodeAt(0);
  return first >= 0xdc00 && first <= 0xdfff ? cut.slice(1) : cut;
}

function withProcessLimit(traces: ProcessTrace[]): ProcessTrace[] {
  while (traces.length > PROCESS_LIMIT) {
    const ended = traces.findIndex((trace) => trace.ended);
    traces.splice(ended >= 0 ? ended : 0, 1);
  }
  return traces;
}

/** Apply one `process.*` event; anything else returns `previous` unchanged. */
export function captureProcessEvent(previous: ProcessTraces, envelope: EventEnvelope): ProcessTraces {
  const { type, data } = envelope.event;
  if (!data || !type.startsWith("process.") || typeof data.process_id !== "string" || !data.process_id) return previous;
  const id = data.process_id;
  const current = previous.find((trace) => trace.id === id) ?? {
    id,
    label: id,
    stdout: "",
    stderr: "",
    ended: false,
    exitCode: null,
    signal: null,
    uncertain: false,
    gap: false,
  };
  let next: ProcessTrace;
  switch (type) {
    case "process.started":
      next = { ...current, label: typeof data.command_summary === "string" ? data.command_summary : current.label };
      break;
    case "process.output": {
      const stream = data.stream;
      if ((stream !== "stdout" && stream !== "stderr") || typeof data.chunk !== "string") return previous;
      next = { ...current, [stream]: tail(current[stream] + data.chunk, STREAM_LIMIT) };
      break;
    }
    case "process.exited":
      next = {
        ...current,
        ended: true,
        exitCode: typeof data.exit_code === "number" ? data.exit_code : null,
        signal: typeof data.signal === "number" ? data.signal : null,
      };
      break;
    default:
      return previous;
  }
  next.uncertain = false;
  return withProcessLimit([...previous.filter((trace) => trace.id !== id), next]);
}

/** The stream dropped: running processes become uncertain and their excerpts have a gap. */
export function markDisconnected(previous: ProcessTraces): ProcessTraces {
  if (!previous.some((trace) => !trace.ended)) return previous;
  return previous.map((trace) => (trace.ended ? trace : { ...trace, uncertain: true, gap: true }));
}

export type ProcessStatus = { label: string; tone: "success" | "warning" | "danger" | "neutral" };

/** How a trace's state reads on screen. */
export function processStatus(trace: ProcessTrace): ProcessStatus {
  if (trace.ended) {
    if (trace.exitCode === 0) return { label: "Exited with code 0", tone: "success" };
    if (trace.exitCode !== null) return { label: `Exited with code ${trace.exitCode}`, tone: "danger" };
    if (trace.signal !== null) return { label: `Stopped by signal ${trace.signal}`, tone: "danger" };
    return { label: "Exited without a status code", tone: "neutral" };
  }
  if (trace.uncertain) return { label: "State unknown after reconnecting", tone: "warning" };
  return { label: "Running", tone: "neutral" };
}

/**
 * Live text of one run. `gap`: the stream dropped while the run was shown, and `agent.run_delta`
 * is never replayed, so text sent meanwhile is missing from the excerpt.
 */
export type RunText = { runId: string; text: string; gap?: true };

/** Live text by run id, oldest activity first. */
export type RunTexts = readonly RunText[];

/** Append live text to its run, keeping the latest excerpt of the most recently active runs. */
export function appendRunText(previous: RunTexts, runId: string, text: string): RunTexts {
  const current = previous.find((entry) => entry.runId === runId);
  const others = previous.filter((entry) => entry.runId !== runId);
  const next: RunText = { runId, text: tail((current?.text ?? "") + text, RUN_TEXT_LIMIT) };
  if (current?.gap) next.gap = true;
  return [...others, next].slice(-RUN_LIMIT);
}

/** The stream dropped: every excerpt kept so far misses whatever is sent until it is back. */
export function markRunTextsDisconnected(previous: RunTexts): RunTexts {
  if (previous.every((entry) => entry.gap)) return previous;
  return previous.map((entry) => (entry.gap ? entry : { ...entry, gap: true }));
}

/**
 * Drop the excerpts of runs a snapshot reports as no longer active (completed, failed, cancelled,
 * interrupted), so a run resumed under the same id starts a new excerpt instead of extending its
 * old one. Runs the snapshot does not list (just started, or beyond a truncated list) keep theirs.
 */
export function pruneRunTexts<R extends { id: string }>(
  previous: RunTexts,
  runs: readonly R[],
  active: (run: R) => boolean,
): RunTexts {
  const ended = new Set(runs.filter((run) => !active(run)).map((run) => run.id));
  if (!previous.some((entry) => ended.has(entry.runId))) return previous;
  return previous.filter((entry) => !ended.has(entry.runId));
}

/**
 * Whether a failed session read means the live excerpts must go: after the vault locked, decrypted
 * command output and run text must not stay on screen (as `useBridgeQuery` drops the snapshot).
 */
export function dropsLiveOutput(error: { kind: string } | null | undefined): boolean {
  return error?.kind === ErrorKind.vaultLocked;
}

/** The excerpts to keep after a session read: none once it failed because the vault locked. */
export function liveOutputAfterRead<T>(error: { kind: string } | null | undefined, kept: readonly T[]): readonly T[] {
  return dropsLiveOutput(error) && kept.length ? [] : kept;
}
