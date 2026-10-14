import { CodeBlock, MediaItem, Notice } from "@aidex/ui/components";
import { CheckIcon, ClipboardCheckIcon, XIcon } from "@aidex/ui/icons";
import { Button, Card, IconBadge, Stack, StatusIndicator } from "@aidex/ui/primitives";
import type { Approval, ApprovalStatus } from "@aidex/client/dto/sessions";
import { approvable } from "./approvalDecision";
import { decisionLabels } from "./approvalLabels";

type Props = {
  approval: Approval;
  /** Disables the decision buttons while this or another decision is being sent. */
  busy?: boolean;
  /**
   * Called with the decision. The caller sends `approvalDecision(approval, approved, commandId)` to
   * `approval.respond`; nothing is decided locally.
   */
  onDecide: (approved: boolean) => void;
  /** Names the owning run and runtime, for lists that mix conversations (the Security approvals). */
  showOrigin?: boolean;
};

const statusIndicator: Record<ApprovalStatus, { label: string; tone: "success" | "warning" | "neutral" }> = {
  pending: { label: "Waiting for you", tone: "warning" },
  approved: { label: "Approved", tone: "success" },
  denied: { label: "Denied", tone: "neutral" },
  expired: { label: "Expired", tone: "neutral" },
};

const json = (value: unknown): string => JSON.stringify(value, null, 2) ?? "null";

/**
 * One broker approval request as the daemon reports it: the tool, its arguments and change preview
 * (untrusted text, shown as code), a warning when either was truncated, and Approve once / Deny while it
 * is pending. A truncated request can be denied but not approved. Used by the conversation and by the
 * Security approvals.
 */
export function ApprovalRequestCard({ approval, busy = false, onDecide, showOrigin = false }: Props) {
  const status = statusIndicator[approval.status];
  const pending = approval.status === "pending";
  const truncated = approval.arguments_truncated || approval.preview_truncated;
  const origin = approval.runtime_id ? `Runtime ${approval.runtime_id}` : "AiDEX Native";
  const labels = decisionLabels(approval, showOrigin);

  return (
    <Card padding="md">
      <Stack gap={4}>
        <MediaItem
          media={<IconBadge tone={pending ? "accent" : "neutral"} icon={<ClipboardCheckIcon />} />}
          title={`Approval: ${approval.tool_name}`}
          titleLevel={3}
          description={showOrigin ? `${origin} · Run ${approval.run_id}` : undefined}
          trailing={<StatusIndicator tone={status.tone} label={status.label} />}
        />
        <CodeBlock title="Requested arguments" language="json" code={json(approval.arguments)} />
        {approval.preview != null ? <CodeBlock title="Change preview" language="json" code={json(approval.preview)} /> : null}
        {truncated ? (
          <Notice tone="warning">This request is too large to show completely. Deny it, or ask for a smaller action.</Notice>
        ) : null}
        {pending ? (
          <Stack direction="row" gap={2} justify="end">
            <Button
              variant="outline"
              size="md"
              icon={<XIcon />}
              aria-label={labels.deny}
              disabled={busy}
              onClick={() => onDecide(false)}
            >
              Deny
            </Button>
            <Button
              variant="primary"
              size="md"
              icon={<CheckIcon />}
              aria-label={labels.approve}
              disabled={busy || !approvable(approval)}
              onClick={() => onDecide(true)}
            >
              Approve once
            </Button>
          </Stack>
        ) : null}
      </Stack>
    </Card>
  );
}
