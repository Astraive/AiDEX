import { useCallback, useEffect, useMemo, useRef } from "react";
import { call } from "@aidex/client/rpc";
import type { PlanStatus, PlanToolResult } from "@aidex/client/dto/artifacts";
import { NATIVE_TOOL_NAMES, type Message } from "@aidex/client/dto/sessions";
import type { SkillSummary } from "@aidex/client/dto/skills";
import { useAidexStatus } from "../../hooks/useAidexStatus";
import { useBridgeQuery } from "../../hooks/useBridgeQuery";
import { useRpc } from "../../hooks/useRpc";
import { invocableSkills } from "./composer/skillSearch";
import { webSearchAvailability } from "./composer/useWebSearchToggle";
import { parseJson } from "./transcript/transcriptModel";

/** The plan artifact a `plan_propose` result names; results longer than the view's excerpt are cut off. */
function planArtifactId(message: Message): string | null {
  if (!message.content_truncated) {
    const id = parseJson<PlanToolResult>(message.content)?.artifact?.id;
    return typeof id === "string" ? id : null;
  }
  return /"artifact"\s*:\s*\{\s*"id"\s*:\s*"(art_[^"\\]+)"/.exec(message.content)?.[1] ?? null;
}

/** Ids of the plan artifacts the transcript shows a card for, in message order. */
function proposedPlanIds(messages: readonly Message[] | undefined): string[] {
  if (!messages) return [];
  const calls = new Set<string>();
  for (const message of messages)
    for (const toolCall of message.tool_calls)
      if (toolCall.name === NATIVE_TOOL_NAMES.planPropose) calls.add(toolCall.id);
  const ids: string[] = [];
  for (const message of messages) {
    if (message.role !== "tool" || !message.tool_call_id || !calls.has(message.tool_call_id)) continue;
    const id = planArtifactId(message);
    if (id && !ids.includes(id)) ids.push(id);
  }
  return ids;
}

/**
 * The current status of `ids`. One `artifact.list` page holds the newest artifacts; plans older than
 * that page are read one by one (a conversation proposes few plans). A plan that cannot be read is
 * left out, so it never counts as active.
 */
async function loadPlanStatuses(sessionId: string, ids: readonly string[]): Promise<ReadonlyMap<string, PlanStatus>> {
  const statuses = new Map<string, PlanStatus>();
  const page = await call("artifact.list", { session_id: sessionId, limit: 100 });
  for (const artifact of page.artifacts) if (artifact.plan_status) statuses.set(artifact.id, artifact.plan_status);
  const older = await Promise.all(
    ids.filter((id) => !statuses.has(id)).map((id) => call("artifact.get", { id }).catch(() => null)),
  );
  for (const artifact of older) if (artifact?.plan_status) statuses.set(artifact.id, artifact.plan_status);
  return statuses;
}

/**
 * Skills, web search, and plan state a conversation's composer and transcript use: the invocable
 * skills of its project, whether web search is set up, and the current status of proposed plans.
 */
export function useConversationExtensions({
  sessionId,
  workspaceId,
  messages,
}: {
  sessionId: string | undefined;
  /** A registered project; without one only drive and plugin skills are listed. */
  workspaceId: string | undefined;
  messages: readonly Message[] | undefined;
}) {
  const skillList = useRpc("skill.list", workspaceId ? { workspace_id: workspaceId } : undefined);
  const allSkills = skillList.data?.skills;
  const skills = useMemo(() => invocableSkills(allSkills), [allSkills]);
  const skill = useCallback(
    (id: string): SkillSummary | undefined => allSkills?.find((candidate) => candidate.id === id),
    [allSkills],
  );

  const webSettings = useRpc("web.settings.get");
  const web = webSearchAvailability(webSettings.data);

  // Plan cards show whether a newer plan superseded them, so the statuses follow new tool results.
  const { connectionKey, daemonRunning } = useAidexStatus();
  const planIds = proposedPlanIds(messages);
  const planKey = planIds.join(",");
  const plans = useBridgeQuery(
    () => loadPlanStatuses(sessionId ?? "", planIds),
    `${connectionKey}|plan-status|${sessionId ?? ""}|${planKey}`,
    daemonRunning && !!sessionId && planIds.length > 0,
  );
  const refetchPlans = plans.refetch;
  const toolResults = messages?.filter((message) => message.role === "tool").length ?? 0;
  const seen = useRef({ toolResults, planKey });
  useEffect(() => {
    const previous = seen.current;
    seen.current = { toolResults, planKey };
    // A new plan changes the query key, which reloads by itself.
    if (planKey && previous.planKey === planKey && previous.toolResults !== toolResults) refetchPlans();
  }, [toolResults, planKey, refetchPlans]);
  const statuses = plans.data;
  const planStatus = useCallback((id: string): PlanStatus | undefined => statuses?.get(id), [statuses]);

  return {
    skills,
    skillsLoading: skillList.loading,
    refetchSkills: skillList.refetch,
    skill,
    web,
    refetchWeb: webSettings.refetch,
    planStatus,
  };
}
