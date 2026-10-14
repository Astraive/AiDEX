import { useEffect, useRef, useState } from "react";
import { EmptyState, Notice } from "@aidex/ui/components";
import { ChevronLeftIcon, ChevronRightIcon, ClipboardCheckIcon, LockOpenIcon } from "@aidex/ui/icons";
import { Button, Stack, TabPanel, Tabs, Text } from "@aidex/ui/primitives";
import { call } from "@aidex/client/rpc";
import { ErrorKind, toAidexError } from "@aidex/client/errors";
import type { Approval, ApprovalStatus } from "@aidex/client/dto/sessions";
import { useToast } from "../../app/providers/toast";
import { useRpc } from "../../hooks/useRpc";
import { CoreStatusNotice } from "../system/CoreStatusNotice";
import { ApprovalRequestCard } from "./ApprovalRequestCard";
import { approvalDecision } from "./approvalDecision";
import { approvalPageView, firstApprovalRange, type ApprovalRange } from "./approvalPage";

const TABS_ID = "approval-views";

type View = ApprovalStatus | "all";

const views: readonly { value: View; label: string; empty: string }[] = [
  { value: "pending", label: "Pending", empty: "Nothing is waiting for your decision." },
  { value: "approved", label: "Approved", empty: "No approved requests." },
  { value: "denied", label: "Denied", empty: "No denied requests." },
  { value: "expired", label: "Expired", empty: "No expired requests." },
  { value: "all", label: "All requests", empty: "No approval requests yet." },
];

/** The launch screens own unlocking the vault; reloading the window returns to them. */
const openUnlockScreen = () => window.location.reload();

type Props = {
  /** Changes on every Security refresh; the list re-reads its page when it does. */
  version: number;
  /** Called after a decision so the Security page re-reads its counts. */
  onChanged: () => void;
};

/**
 * The broker approval center: one status per view, filtered by the daemon (`approval.list` with
 * `status`). The daemon orders requests oldest first by creation time across conversations; pages
 * start at offset 0, the oldest requests, and each page lists its requests newest first
 * (`approvalPage.ts`). Decisions go to `approval.respond` with the request's exact id and
 * fingerprint and a fresh command id; nothing is decided or remembered locally.
 */
export function LiveApprovalsPanel({ version, onChanged }: Props) {
  const notify = useToast();
  const [view, setView] = useState<View>("pending");
  const [range, setRange] = useState<ApprovalRange>(firstApprovalRange);
  const [deciding, setDeciding] = useState<string>();
  const [error, setError] = useState<string>();
  const approvals = useRpc("approval.list", {
    offset: range.offset,
    limit: range.limit,
    ...(view === "all" ? {} : { status: view }),
  });

  const refetch = approvals.refetch;
  const firstVersion = useRef(version);
  useEffect(() => {
    if (version !== firstVersion.current) refetch();
  }, [version, refetch]);

  const page = approvals.data ? approvalPageView(approvals.data) : undefined;
  const truncated = approvals.data?.approvals_truncated === true;
  const locked = approvals.error?.kind === ErrorKind.vaultLocked;
  const empty = views.find((item) => item.value === view)?.empty;

  const decide = async (approval: Approval, approved: boolean) => {
    setDeciding(approval.id);
    setError(undefined);
    try {
      await call("approval.respond", approvalDecision(approval, approved, crypto.randomUUID()));
      notify(approved ? `Approved ${approval.tool_name} once.` : `Denied ${approval.tool_name}.`);
    } catch (caught) {
      setError(toAidexError(caught).message);
    } finally {
      setDeciding(undefined);
      refetch();
      onChanged();
    }
  };

  return (
    <Stack gap={5}>
      <CoreStatusNotice requires="daemon" />
      <Tabs
        id={TABS_ID}
        label="Approval requests"
        variant="pill"
        items={views.map(({ value, label }) => ({ value, label }))}
        value={view}
        onValueChange={(next: View) => {
          setView(next);
          setRange(firstApprovalRange);
          setError(undefined);
        }}
      />
      {error ? (
        <Notice tone="danger" role="alert">
          {error}
        </Notice>
      ) : null}
      {locked ? (
        <Notice tone="warning">
          <Stack gap={3} align="start">
            <Text as="span">The vault is locked, so approval requests cannot be read. Unlock it from the start screen.</Text>
            <Button variant="outline" size="md" icon={<LockOpenIcon />} onClick={openUnlockScreen}>
              Go to unlock
            </Button>
          </Stack>
        </Notice>
      ) : approvals.error ? (
        <Notice tone="danger">Cannot read approval requests: {approvals.error.message}</Notice>
      ) : null}
      <TabPanel tabsId={TABS_ID} value={view}>
        <Stack gap={4}>
          {page && page.total === 0 ? <EmptyState icon={<ClipboardCheckIcon />} title={empty ?? "No requests."} /> : null}
          {page && page.total > 0 && page.approvals.length === 0 ? (
            <Text tone="secondary">No requests are left on this page. Use Previous to see the rest.</Text>
          ) : null}
          {!page && approvals.loading ? <Text tone="secondary">Reading approval requests…</Text> : null}
          {page?.approvals.map((approval) => (
            <ApprovalRequestCard
              key={approval.id}
              approval={approval}
              showOrigin
              busy={deciding !== undefined}
              onDecide={(approved) => void decide(approval, approved)}
            />
          ))}
          {page && page.total > 0 ? (
            <Stack gap={2}>
              <Stack direction="row" gap={3} align="center" justify="between">
                <Button
                  variant="ghost"
                  size="md"
                  icon={<ChevronLeftIcon />}
                  disabled={approvals.loading || !page.previous}
                  onClick={() => page.previous && setRange(page.previous)}
                >
                  Previous
                </Button>
                <Text tone="secondary">
                  {page.approvals.length > 0 ? `${page.first + 1}–${page.end} of ${page.total}` : `${page.total} in all`}
                </Text>
                <Button
                  variant="ghost"
                  size="md"
                  trailing={<ChevronRightIcon />}
                  disabled={approvals.loading || !page.next}
                  onClick={() => page.next && setRange(page.next)}
                >
                  Next
                </Button>
              </Stack>
              {page.previous || page.next ? (
                <Text tone="muted">
                  Pages run from the oldest requests to the newest, across conversations; each page lists its
                  requests newest first.
                </Text>
              ) : null}
              {truncated ? (
                <Text tone="muted">Some requests were too large to fit on this page; Previous lists them.</Text>
              ) : null}
            </Stack>
          ) : null}
        </Stack>
      </TabPanel>
    </Stack>
  );
}
