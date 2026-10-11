import type { EventEnvelope } from "@aidex/client/dto/daemon";
export const auditKinds = [
  "vault.locked",
  "vault.unlocked",
  "secret.stored",
  "secret.deleted",
  "secret.granted",
  "secret.revoked",
  "policy.decision",
  "policy.changed",
  "approval.created",
  "approval.approved",
  "approval.denied",
  "approval.expired",
  "approval.revoked",
  "approval.cancelled",
  "sandbox.negotiated",
  "sandbox.violation",
  "process.started",
  "process.exited",
  "task.failed",
  "agent.error",
];
export type AuditMetadata = {
  cursor: number;
  time: string;
  type: string;
  workspace: string;
  session: string;
  result: string;
};
/** Deliberately omit every event body string, command, summary, secret name, and payload. */
export function auditMetadata(event: EventEnvelope): AuditMetadata | undefined {
  if (
    !event?.event ||
    !event.scope ||
    !auditKinds.includes(event.event.type) ||
    !Number.isSafeInteger(event.cursor)
  )
    return;
  const data = event.event.data;
  let result = "Recorded";
  const decisions = [
    "allow",
    "deny",
    "sandbox",
    "require_approval",
    "rate_limit",
  ];
  if (typeof data?.decision === "string" && decisions.includes(data.decision))
    result = data.decision.replaceAll("_", " ");
  if (typeof data?.exit_code === "number" && Number.isInteger(data.exit_code))
    result = `Exit ${data.exit_code}`;
  if (event.event.type === "sandbox.negotiated")
    result =
      data?.enforced === true
        ? "Requested baseline met"
        : "Requested baseline unavailable";
  return {
    cursor: event.cursor,
    time: event.occurred_at,
    type: event.event.type,
    workspace: event.scope.workspace_id ?? "Drive",
    session: event.scope.session_id ?? "—",
    result,
  };
}
