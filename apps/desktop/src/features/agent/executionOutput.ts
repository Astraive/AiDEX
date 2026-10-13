// Bounded live excerpts for the execution page. `process.output` and `agent.run_delta` are
// ephemeral (never stored or replayed), so these only cover what arrived while the page was
// connected; final tool output and run transcripts stay in the encrypted session history.
import type { EventEnvelope } from "@aidex/client/dto/daemon";

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

/** Live text by run id, oldest activity first. */
export type RunTexts = readonly { runId: string; text: string }[];

/** Append live text to its run, keeping the latest excerpt of the most recently active runs. */
export function appendRunText(previous: RunTexts, runId: string, text: string): RunTexts {
  const current = previous.find((entry) => entry.runId === runId)?.text ?? "";
  const others = previous.filter((entry) => entry.runId !== runId);
  return [...others, { runId, text: tail(current + text, RUN_TEXT_LIMIT) }].slice(-RUN_LIMIT);
}
