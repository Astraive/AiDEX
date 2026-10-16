// The security audit view's projection of durable events: a fixed, value-free row per event. Only the
// cursor, time, type, scope ids, and a closed result category are kept; summaries, commands, reasons,
// secret names, consumers, and every other body field are dropped before anything reaches UI state.
import type { EventEnvelope } from "@aidex/client/dto/daemon";

/**
 * The event types the audit view reads, sent as `filter.kinds`. Every entry must exist in the daemon's
 * catalog (`EVENT_NAMES` in crates/events/src/catalog/mod.rs): the daemon refuses a filter naming an
 * unknown kind, and at most 32 kinds are allowed. `auditMetadata.test.ts` checks both against the
 * catalog source.
 */
export const auditKinds = [
  "vault.unlocked",
  "vault.locked",
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
  "tool.denied",
  "sandbox.negotiated",
  "sandbox.violation",
  "process.started",
  "process.exited",
  "emergency.stopped",
  "emergency.resumed",
  "update.switched",
  "update.rolled_back",
  "update.failed",
  "task.failed",
  "agent.error",
] as const;

export type AuditKind = (typeof auditKinds)[number];

/** Display names of the audited event types. */
export const auditKindLabels: Record<AuditKind, string> = {
  "vault.unlocked": "Vault unlocked",
  "vault.locked": "Vault locked",
  "secret.granted": "Secret handle issued",
  "secret.revoked": "Secret handle revoked",
  "policy.decision": "Policy decision",
  "policy.changed": "Policy changed",
  "approval.created": "Approval requested",
  "approval.approved": "Approval granted",
  "approval.denied": "Approval denied",
  "approval.expired": "Approval expired",
  "approval.revoked": "Approval revoked",
  "approval.cancelled": "Approval cancelled",
  "tool.denied": "Tool call denied",
  "sandbox.negotiated": "Sandbox level set",
  "sandbox.violation": "Sandbox violation",
  "process.started": "Process started",
  "process.exited": "Process exited",
  "emergency.stopped": "Emergency stop",
  "emergency.resumed": "Emergency stop lifted",
  "update.switched": "Update selected",
  "update.rolled_back": "Update rolled back",
  "update.failed": "Update failed",
  "task.failed": "Task failed",
  "agent.error": "Agent error",
};

/** One audit row. Nothing else from the event is kept. */
export type AuditMetadata = {
  cursor: number;
  time: string;
  type: AuditKind;
  /** The workspace id, or "Drive" for drive-wide events. */
  workspace: string;
  /** The session id, or "—". */
  session: string;
  result: string;
};

const VERDICTS: Record<string, string> = {
  allow: "Allowed",
  deny: "Denied",
  sandbox: "Sandboxed",
  require_approval: "Needs approval",
  rate_limit: "Rate limited",
};

/** Results that depend only on the event type. */
const FIXED_RESULTS: Partial<Record<AuditKind, string>> = {
  "vault.unlocked": "Unlocked",
  "vault.locked": "Locked",
  "secret.granted": "Issued",
  "secret.revoked": "Revoked",
  "policy.changed": "Changed",
  "approval.created": "Requested",
  "approval.approved": "Approved",
  "approval.denied": "Denied",
  "approval.expired": "Expired",
  "approval.revoked": "Revoked",
  "approval.cancelled": "Cancelled",
  "tool.denied": "Denied",
  "sandbox.violation": "Violation",
  "process.started": "Started",
  "emergency.stopped": "Stopped",
  "emergency.resumed": "Resumed",
  "update.switched": "Selected",
  "update.rolled_back": "Rolled back",
  "update.failed": "Failed",
  "task.failed": "Failed",
  "agent.error": "Error",
};

const isAuditKind = (type: string): type is AuditKind => (auditKinds as readonly string[]).includes(type);

/** The closed result category: a fixed word, a known verdict, or an integer exit status. */
function resultOf(type: AuditKind, data: Record<string, unknown> | undefined): string {
  if (type === "policy.decision")
    return typeof data?.decision === "string" ? (VERDICTS[data.decision] ?? "Recorded") : "Recorded";
  if (type === "process.exited") {
    if (Number.isSafeInteger(data?.exit_code)) return `Exit ${data?.exit_code as number}`;
    if (Number.isSafeInteger(data?.signal)) return `Signal ${data?.signal as number}`;
    return "Exited";
  }
  if (type === "sandbox.negotiated") return data?.enforced === true ? "Enforced" : "Not enforced";
  return FIXED_RESULTS[type] ?? "Recorded";
}

/** Projects one event to an audit row, or `undefined` for events the audit view does not show. */
export function auditMetadata(event: EventEnvelope): AuditMetadata | undefined {
  if (!event?.event || !event.scope || !Number.isSafeInteger(event.cursor) || typeof event.occurred_at !== "string") return;
  const type = event.event.type;
  if (!isAuditKind(type)) return;
  return {
    cursor: event.cursor,
    time: event.occurred_at,
    type,
    workspace: typeof event.scope.workspace_id === "string" ? event.scope.workspace_id : "Drive",
    session: typeof event.scope.session_id === "string" ? event.scope.session_id : "—",
    result: resultOf(type, event.event.data),
  };
}

/** Adds `row` to `rows` (newest first, one row per cursor), keeping at most `limit`. */
export function mergeAuditRow(rows: readonly AuditMetadata[], row: AuditMetadata, limit: number): AuditMetadata[] {
  return [row, ...rows.filter((existing) => existing.cursor !== row.cursor)].sort((a, b) => b.cursor - a.cursor).slice(0, limit);
}
