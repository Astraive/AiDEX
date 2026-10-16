// Accessible names of an approval request's decision buttons. Lists show many requests at once, so
// each button names what it decides: the tool and, where requests of several runs mix, the run.
import type { Approval } from "@aidex/client/dto/sessions";

export type DecisionLabels = { deny: string; approve: string };

/** The request a decision applies to, as screen readers announce it. */
export function approvalTarget(approval: Pick<Approval, "tool_name" | "run_id">, showOrigin: boolean): string {
  return showOrigin ? `${approval.tool_name} (run ${approval.run_id})` : approval.tool_name;
}

/** Each label starts with the visible button text, so speech and voice control still match it. */
export function decisionLabels(approval: Pick<Approval, "tool_name" | "run_id">, showOrigin: boolean): DecisionLabels {
  const target = approvalTarget(approval, showOrigin);
  return { deny: `Deny: ${target}`, approve: `Approve once: ${target}` };
}
