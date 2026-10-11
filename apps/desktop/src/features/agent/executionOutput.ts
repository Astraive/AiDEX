import type { EventEnvelope } from "@aidex/client/dto/daemon";
export type ProcessTrace = {
  label: string;
  stdout: string;
  stderr: string;
  ended: boolean;
  exitCode: number | null;
  signal: number | null;
  uncertain?: boolean;
};
export type ProcessTraces = Record<string, ProcessTrace>;
/** Bounded live excerpts; final tool output remains in encrypted run history. */
export function captureProcessEvent(
  previous: ProcessTraces,
  envelope: EventEnvelope,
): ProcessTraces {
  const data = envelope.event.data;
  if (
    !data ||
    !["process.started", "process.output", "process.exited"].includes(
      envelope.event.type,
    ) ||
    typeof data.process_id !== "string"
  )
    return previous;
  const id = data.process_id;
  const current = previous[id] ?? {
    label: id,
    stdout: "",
    stderr: "",
    ended: false,
    exitCode: null,
    signal: null,
  };
  let next = { ...current, uncertain: false };
  if (
    envelope.event.type === "process.started" &&
    typeof data.command_summary === "string"
  )
    next = { ...next, label: data.command_summary };
  if (
    envelope.event.type === "process.output" &&
    (data.stream === "stdout" || data.stream === "stderr") &&
    typeof data.chunk === "string"
  )
    next = {
      ...next,
      [data.stream]: `${current[data.stream]}${data.chunk}`.slice(-65536),
    };
  if (envelope.event.type === "process.exited")
    next = {
      ...next,
      ended: true,
      exitCode: typeof data.exit_code === "number" ? data.exit_code : null,
      signal: typeof data.signal === "number" ? data.signal : null,
    };
  return Object.fromEntries(
    [
      ...Object.entries(previous).filter(([key]) => key !== id),
      [id, next],
    ].slice(-8),
  );
}
