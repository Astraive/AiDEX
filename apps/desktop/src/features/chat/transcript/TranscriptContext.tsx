import { createContext, useContext } from "react";
import type { ArtifactMeta, PlanStatus } from "@aidex/client/dto/artifacts";
import type { HistoryScope } from "@aidex/client/dto/sessions";
import type { SkillSummary } from "@aidex/client/dto/skills";

/** An artifact a transcript row refers to; `meta` when the row already has it. */
export type ArtifactRef = { id: string; name: string; meta?: ArtifactMeta };

/** Plan card actions, offered only in a live conversation. */
export type PlanActions = {
  /** Switches the conversation to Agent mode and sends "Implement this plan:" with the plan. */
  implement: (markdown: string) => void;
  /** Starts a new Agent chat in the same project with that message. */
  implementInNewChat: (markdown: string) => void;
  disabled: boolean;
};

export type TranscriptEnvironment = {
  /** Where the messages come from, for paging long content through `history.read`. */
  scope: HistoryScope;
  openLink: (url: string) => void;
  openArtifact: (artifact: ArtifactRef) => void;
  downloadArtifact: (id: string) => void;
  /**
   * A plan's current status (a newer plan supersedes it); undefined while unknown. When this is set,
   * plan cards offer Implement only for plans it reports as active.
   */
  planStatus?: (id: string) => PlanStatus | undefined;
  planActions?: PlanActions;
  /** Details of an invoked skill id, for chip tooltips. */
  skill?: (id: string) => SkillSummary | undefined;
};

const TranscriptContext = createContext<TranscriptEnvironment | null>(null);

export const TranscriptProvider = TranscriptContext.Provider;

export function useTranscript(): TranscriptEnvironment {
  const environment = useContext(TranscriptContext);
  if (!environment) throw new Error("useTranscript must be used inside TranscriptProvider");
  return environment;
}
