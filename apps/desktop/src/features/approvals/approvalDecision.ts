// The params of `approval.respond` for one decision. Native, gateway, and external-runtime approvals all
// bind a decision to the exact approval id and fingerprint the daemon issued, plus a command id that
// makes a retried click idempotent.
import type { Approval } from "@aidex/client/dto/sessions";
import type { RpcParams } from "@aidex/client/rpc";

/** Whether `approval` can be approved as shown: pending, with complete arguments and preview. */
export function approvable(approval: Approval): boolean {
  return approval.status === "pending" && !approval.arguments_truncated && !approval.preview_truncated;
}

/**
 * Builds the `approval.respond` params. Throws when the approval is no longer pending, lacks its
 * identity, or would be approved while its arguments or preview are truncated (a deny is still allowed).
 */
export function approvalDecision(approval: Approval, approved: boolean, commandId: string): RpcParams<"approval.respond"> {
  if (approval.status !== "pending") throw new Error("This request is no longer waiting for a decision.");
  if (!approval.id || !approval.fingerprint || !commandId)
    throw new Error("The request's exact id and fingerprint are required to answer it.");
  if (approved && !approvable(approval)) throw new Error("This request is incomplete. Deny it, or ask for a smaller action.");
  return { id: approval.id, fingerprint: approval.fingerprint, approved, command_id: commandId };
}
