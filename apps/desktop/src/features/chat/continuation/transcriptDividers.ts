// Where a conversation changed course, as dividers in its transcript: between root runs whose mode
// differs ("Continued as Agent · <project> · <permission>", "Switched to Chat", "Switched to Plan"),
// after the messages a fork copied ("Continued from <title>"), and at the end when the conversation
// was switched but has not run in its new mode yet. Pure functions, so they run during prerendering.
import type {
  ConversationMode,
  PermissionMode,
  Run,
  SessionDetail,
} from "@aidex/client/dto/sessions";
import { PERMISSION_LABELS } from "./permissions";

export type TranscriptDividerSpec = {
  key: string;
  /**
   * History index (`Message.original_index`) the divider precedes; at the end of the transcript when
   * no shown message has this index or a later one.
   */
  index: number;
  kind: "mode" | "fork";
  label: string;
  details: string[];
};

/** A conversation's mode, project, and permissions at some point. */
type ModeState = {
  mode: ConversationMode;
  workspace_id?: string | null;
  permission_mode?: PermissionMode;
};

export type DividerInput = {
  runs: readonly Run[];
  session: Pick<
    SessionDetail,
    "mode" | "permission_mode" | "workspace_id" | "total_messages" | "forked_from"
  >;
  /** A project's display name, or undefined when it is no longer registered. */
  projectName: (workspaceId: string) => string | undefined;
  /** Title of the conversation a fork came from, when it still exists. */
  sourceTitle?: string;
};

const MODE_NAMES: Record<ConversationMode, string> = {
  chat: "Chat",
  plan: "Plan",
  agent: "Agent",
};

function project(state: ModeState, name: DividerInput["projectName"]): string[] {
  const named = state.workspace_id ? name(state.workspace_id) : undefined;
  return named ? [named] : [];
}

/** What a switch into `state` reads as, and the facts that matter for that mode. */
function describeSwitch(state: ModeState, name: DividerInput["projectName"]): { label: string; details: string[] } {
  if (state.mode === "agent")
    return {
      label: "Continued as Agent",
      details: [
        ...project(state, name),
        ...(state.permission_mode ? [PERMISSION_LABELS[state.permission_mode]] : []),
      ],
    };
  if (state.mode === "plan") return { label: "Switched to Plan", details: project(state, name) };
  return { label: "Switched to Chat", details: [] };
}

/** Root runs that recorded their mode and user message, in history order. */
function rootRuns(runs: readonly Run[]): Array<Run & { message_index: number }> {
  return runs
    .filter((run): run is Run & { message_index: number } => !run.parent_run_id && run.message_index !== undefined)
    .sort((a, b) => a.message_index - b.message_index);
}

/**
 * The dividers of a conversation, in history order. Runs recorded before modes were stored (no
 * `mode`) and child runs never produce a divider.
 */
export function transcriptDividers(input: DividerInput): TranscriptDividerSpec[] {
  const { session, projectName } = input;
  const roots = rootRuns(input.runs);
  const dividers: TranscriptDividerSpec[] = [];
  const fork = session.forked_from;
  if (fork) {
    const first = roots.find((run) => run.message_index >= fork.message_count && run.mode);
    const state: ModeState = first?.mode
      ? { mode: first.mode, workspace_id: first.workspace_id, permission_mode: first.permission_mode }
      : session;
    const facts = state.mode === "chat" ? [] : describeSwitch(state, projectName).details;
    dividers.push({
      key: `fork:${fork.session_id}`,
      index: fork.message_count,
      kind: "fork",
      label: input.sourceTitle ? `Continued from ${input.sourceTitle}` : "Continued from another conversation",
      details: [MODE_NAMES[state.mode], ...facts],
    });
  }
  roots.forEach((run, position) => {
    const previous = roots[position - 1];
    if (!previous?.mode || !run.mode || previous.mode === run.mode) return;
    dividers.push({
      key: `run:${run.id}`,
      index: run.message_index,
      kind: "mode",
      ...describeSwitch({ mode: run.mode, workspace_id: run.workspace_id, permission_mode: run.permission_mode }, projectName),
    });
  });
  // Switched after the last run: say so at the end until a run in the new mode takes its place.
  const last = roots.at(-1);
  if (last?.mode && last.mode !== session.mode)
    dividers.push({
      key: "pending",
      index: Math.max(session.total_messages, last.message_index + 1),
      kind: "mode",
      ...describeSwitch(session, projectName),
    });
  return dividers.sort((a, b) => a.index - b.index);
}
