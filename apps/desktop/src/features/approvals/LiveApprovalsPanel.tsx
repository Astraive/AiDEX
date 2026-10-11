import { useEffect, useState } from "react";
import { CodeBlock, Notice } from "@aidex/ui/components";
import {
  Button,
  Card,
  Stack,
  TabPanel,
  Tabs,
  Text,
} from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import type { Approval } from "@aidex/client/dto/sessions";
import { useRpc } from "../../hooks/useRpc";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { useExtensionAction } from "../extensions/useExtensionAction";
import { approvalDecision } from "./approvalDecision";

export function LiveApprovalsPanel({ onChanged }: { onChanged: () => void }) {
  const [view, setView] = useState<"pending" | "history">("pending");
  const [offset, setOffset] = useState(0);
  const approvals = useRpc("approval.list", {
    offset,
    limit: 50,
    ...(view === "pending" ? { status: "pending" as const } : {}),
  });
  const action = useExtensionAction(() => {
    approvals.refetch();
    onChanged();
  });
  useEffect(() => {
    const timer = setInterval(approvals.refetch, 2000);
    return () => clearInterval(timer);
  }, [approvals.refetch]);
  const items =
    approvals.data?.approvals.filter((approval) =>
      view === "pending"
        ? approval.status === "pending"
        : approval.status !== "pending",
    ) ?? [];
  const decide = (approval: Approval, approved: boolean) =>
    void action.act(() =>
      call(
        "approval.respond",
        approvalDecision(approval, approved, crypto.randomUUID()),
      ),
    );
  return (
    <Stack gap={4}>
      <CoreStatusNotice requires="daemon" />
      <Tabs
        id="live-approvals"
        label="Approval views"
        variant="pill"
        value={view}
        items={[
          { value: "pending", label: "Pending" },
          { value: "history", label: "History" },
        ]}
        onValueChange={(next: typeof view) => {
          setView(next);
          setOffset(0);
        }}
      />
      {(approvals.error || action.error) && (
        <Notice tone="danger">
          {action.error ?? approvals.error?.message}
        </Notice>
      )}
      <TabPanel tabsId="live-approvals" value={view}>
        <Stack gap={4}>
          {approvals.loading && (
            <Text tone="muted">Loading broker requests…</Text>
          )}
          {items.map((approval) => (
            <Card key={approval.id} padding="md">
              <Stack gap={3}>
                <Text>
                  {approval.tool_name} · {approval.status}
                </Text>
                <Text tone="muted">
                  Run {approval.run_id}
                  {approval.runtime_id
                    ? ` · Runtime ${approval.runtime_id}`
                    : " · Core broker"}
                </Text>
                <CodeBlock
                  title="Requested arguments"
                  language="json"
                  code={JSON.stringify(approval.arguments, null, 2)}
                />
                {approval.preview != null && (
                  <CodeBlock
                    title="Change preview"
                    language="json"
                    code={JSON.stringify(approval.preview, null, 2)}
                  />
                )}
                {(approval.arguments_truncated ||
                  approval.preview_truncated) && (
                  <Notice tone="warning">
                    This request is truncated. Ask for a smaller action before
                    approving it.
                  </Notice>
                )}
                {approval.status === "pending" && (
                  <Stack direction="row" gap={2}>
                    <Button
                      disabled={
                        action.busy ||
                        approval.arguments_truncated ||
                        approval.preview_truncated
                      }
                      onClick={() => decide(approval, true)}
                    >
                      Approve once
                    </Button>
                    <Button
                      variant="outline"
                      disabled={action.busy}
                      onClick={() => decide(approval, false)}
                    >
                      Deny
                    </Button>
                  </Stack>
                )}
              </Stack>
            </Card>
          ))}
          {approvals.data && !items.length && (
            <Text tone="muted">
              {view === "pending"
                ? "No pending requests on this page."
                : "No saved decisions on this page."}
            </Text>
          )}
          <Stack direction="row" gap={2}>
            <Button
              variant="outline"
              disabled={!offset || approvals.loading}
              onClick={() => setOffset(Math.max(0, offset - 50))}
            >
              Previous page
            </Button>
            <Text>
              {offset + (approvals.data?.approvals.length ? 1 : 0)}–
              {offset + (approvals.data?.approvals.length ?? 0)} of{" "}
              {approvals.data?.total_approvals ?? 0} requests
            </Text>
            <Button
              variant="outline"
              disabled={
                approvals.loading || approvals.data?.next_offset == null
              }
              onClick={() => setOffset(approvals.data!.next_offset!)}
            >
              Next page
            </Button>
          </Stack>
        </Stack>
      </TabPanel>
    </Stack>
  );
}
