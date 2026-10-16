// Whether a conversation offers "Resume task" for its latest run. The engine resumes only an
// interrupted run, and a run on an external runtime only when the runtime can resume and the run
// saved a protocol reference to continue from (crates/harness/src/engine.rs); replaying it instead
// could repeat its effects. Pure, so the rule is testable.
import type { RuntimeDescriptor } from "@aidex/client/dto/execution";
import type { Run } from "@aidex/client/dto/sessions";

export function canResume(
  run: Pick<Run, "status" | "has_runtime_ref"> | undefined,
  /** The conversation runs on an external runtime rather than AiDEX's own harness. */
  external: boolean,
  /** The runtime's `execution.status` descriptor; unknown means it cannot resume. */
  descriptor: Pick<RuntimeDescriptor, "supports_resume"> | undefined,
): boolean {
  if (run?.status !== "interrupted") return false;
  if (!external) return true;
  return !!descriptor?.supports_resume && run.has_runtime_ref;
}
