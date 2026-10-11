import type { Approval } from "@aidex/client/dto/sessions";
import type { RpcParams } from "@aidex/client/rpc";
/** Native, SDK-gateway, and vendor approvals all use the same service-owned binding. */
export function approvalDecision(
  approval: Approval,
  approved: boolean,
  commandId: string,
): RpcParams<"approval.respond"> {
  if (approval.status !== "pending")
    throw new Error("This approval is no longer pending.");
  if (!approval.id || !approval.fingerprint || !commandId)
    throw new Error(
      "The exact approval identity and fingerprint are required.",
    );
  if (approved && (approval.arguments_truncated || approval.preview_truncated))
    throw new Error("Review a complete request before approving it.");
  return {
    id: approval.id,
    fingerprint: approval.fingerprint,
    approved,
    command_id: commandId,
  };
}
